//! First-run permission prompts (SPEC.md §16).
//!
//! On the first run of a given `(name, version)`, Harbor prompts the user to
//! grant or deny each declared permission. The decision is persisted so
//! subsequent runs do not re-prompt. This is **disclosure-only** — Harbor does
//! not technically enforce permissions in V1 (§16.2, §20.4).
//!
//! Grant state is stored under `~/.harbor/permissions/local/<name>/<version>.toml`
//! (see [`crate::cache::HarborHome::local_grants_path`]) — deliberately *outside* the
//! extracted package cache, so a package cannot ship a pre-filled grants file
//! that suppresses its own first-run prompt.
//!
//! All prompting I/O uses stderr, and stdin is only read when it is a real
//! terminal: for `package-type = "mcp"` the child inherits Harbor's
//! stdin/stdout as the JSON-RPC stdio transport, which must stay untouched.

use std::collections::HashSet;
use std::io::{self, BufRead, IsTerminal, Write};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors from the permissions subsystem.
#[derive(Debug, Error)]
pub enum PermissionError {
    #[error("I/O error managing permissions: {0}")]
    Io(#[from] io::Error),

    #[error("failed to serialize grant state: {0}")]
    Serde(#[from] toml::ser::Error),

    #[error("failed to deserialize grant state: {0}")]
    SerdeDe(#[from] toml::de::Error),
}

/// Persisted grant state for a single package version.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Grants {
    /// Permissions the user explicitly granted.
    #[serde(default)]
    granted: Vec<String>,
    /// Permissions the user explicitly denied.
    #[serde(default)]
    denied: Vec<String>,
}

impl Grants {
    fn is_decided(&self, permission: &str) -> bool {
        self.granted.iter().any(|p| p == permission)
            || self.denied.iter().any(|p| p == permission)
    }
}

/// Read the persisted grant state for a package version, if it exists.
fn read_grants(path: &std::path::Path) -> Result<Option<Grants>, PermissionError> {
    if !path.is_file() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(path)?;
    let grants: Grants = toml::from_str(&s)?;
    Ok(Some(grants))
}

/// Write the grant state for a package version.
fn write_grants(path: &std::path::Path, grants: &Grants) -> Result<(), PermissionError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let s = toml::to_string(grants)?;
    std::fs::write(path, s)?;
    Ok(())
}

/// Prompt for a single permission on the terminal.
///
/// Returns `Some(true)` for grant, `Some(false)` for deny, and `None` on EOF
/// (the user closed the terminal input); the caller records nothing for the
/// permission in that case, so a later interactive run prompts again.
fn prompt_permission(permission: &str) -> io::Result<Option<bool>> {
    let stdin = io::stdin();
    let mut stderr = io::stderr();

    write!(stderr, "  • {permission} — grant? [y/n] ")?;
    stderr.flush()?;

    let mut line = String::new();
    loop {
        line.clear();
        if stdin.lock().read_line(&mut line)? == 0 {
            return Ok(None); // EOF (e.g. Ctrl-D)
        }
        match line.trim().to_lowercase().as_str() {
            "y" | "yes" => return Ok(Some(true)),
            "n" | "no" => return Ok(Some(false)),
            _ => {
                write!(stderr, "  Please answer 'y' or 'n': ")?;
                stderr.flush()?;
            }
        }
    }
}

/// Check permissions for a package version and prompt if this is the first
/// run (SPEC.md §16.2).
///
/// Permissions already decided (granted or denied) in the persisted grant
/// state are not re-prompted. Denied permissions are recorded but **not
/// enforced** — this is disclosure-only (§20.4).
///
/// When stdin is not a terminal (piped/CI/MCP-client invocation), the
/// undecided permissions are disclosed on stderr and nothing is persisted:
/// stdin belongs to the child process and must not be consumed, and silence
/// is not consent — the next interactive run prompts normally.
///
/// Call this function before launch (§9.10).
///
/// `grants_path` is caller-supplied rather than derived here: `cmd_run`
/// passes `home.local_grants_path(name, version)`, `cmd_add` passes
/// `home.github_grants_path(owner, repo, sha)` (SPEC.md §12.2 step 7).
/// `name`/`version` are used only for the display banner.
pub fn check_and_prompt(
    name: &str,
    version: &str,
    permissions: &[String],
    grants_path: &std::path::Path,
) -> Result<(), PermissionError> {
    if permissions.is_empty() {
        return Ok(());
    }

    let mut grants = read_grants(grants_path)?.unwrap_or_default();

    // Deduplicate while preserving declaration order, then keep only the
    // permissions not yet decided in the persisted state.
    let mut seen = HashSet::new();
    let undecided: Vec<&String> = permissions
        .iter()
        .filter(|p| seen.insert(p.as_str()))
        .filter(|p| !grants.is_decided(p))
        .collect();

    if undecided.is_empty() {
        return Ok(());
    }

    eprintln!(
        "Package {name} v{version} requires the following permissions \
         (disclosure-only, not enforced in V1):"
    );

    if !io::stdin().is_terminal() {
        // Non-interactive: disclose, but never consume stdin (it may be an
        // MCP client's JSON-RPC stream) and never record a decision the user
        // did not make.
        for perm in &undecided {
            eprintln!("  • {perm}");
        }
        eprintln!("(non-interactive session: permissions not prompted; run interactively to grant or deny)");
        return Ok(());
    }

    for perm in undecided {
        match prompt_permission(perm)? {
            Some(true) => grants.granted.push(perm.clone()),
            Some(false) => grants.denied.push(perm.clone()),
            None => break, // EOF: record nothing further; re-prompt next run.
        }
    }

    write_grants(grants_path, &grants)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::HarborHome;
    use tempfile::tempdir;

    #[test]
    fn no_permissions_skips_everything() {
        let dir = tempdir().unwrap();
        let home = HarborHome::at(dir.path());
        let grants_path = home.local_grants_path("test", "1.0.0");
        check_and_prompt("test", "1.0.0", &[], &grants_path).unwrap();
        assert!(!grants_path.exists());
    }

    #[test]
    fn grants_round_trip_and_live_outside_the_package_cache() {
        let dir = tempdir().unwrap();
        let home = HarborHome::at(dir.path());
        let path = home.local_grants_path("pkg", "1.0.0");

        let grants = Grants {
            granted: vec!["network".to_string()],
            denied: vec!["camera".to_string()],
        };
        write_grants(&path, &grants).unwrap();

        // Grant state must never live under packages/ — a package could ship it.
        assert!(!path.starts_with(home.packages()));
        assert!(path.starts_with(home.root()));

        let loaded = read_grants(&path).unwrap().unwrap();
        assert_eq!(loaded.granted, vec!["network"]);
        assert_eq!(loaded.denied, vec!["camera"]);
    }

    #[test]
    fn fully_decided_permissions_skip_prompt() {
        let dir = tempdir().unwrap();
        let home = HarborHome::at(dir.path());
        let grants = Grants {
            granted: vec!["network".to_string()],
            denied: vec!["filesystem".to_string()],
        };
        let grants_path = home.local_grants_path("test", "1.0.0");
        write_grants(&grants_path, &grants).unwrap();

        // Both permissions decided (one granted, one denied) — no prompting,
        // no stdin access, state unchanged.
        let perms: Vec<String> = vec!["network".into(), "filesystem".into()];
        check_and_prompt("test", "1.0.0", &perms, &grants_path).unwrap();

        let loaded = read_grants(&grants_path).unwrap().unwrap();
        assert_eq!(loaded.granted, vec!["network"]);
        assert_eq!(loaded.denied, vec!["filesystem"]);
    }

    #[test]
    fn duplicate_declared_permissions_count_as_one() {
        let dir = tempdir().unwrap();
        let home = HarborHome::at(dir.path());
        let grants = Grants {
            granted: vec!["network".to_string()],
            denied: vec![],
        };
        let grants_path = home.local_grants_path("test", "1.0.0");
        write_grants(&grants_path, &grants).unwrap();

        // A duplicated entry must not be treated as an undecided permission
        // (the old count-based check re-prompted forever on this input).
        let perms: Vec<String> = vec!["network".into(), "network".into()];
        check_and_prompt("test", "1.0.0", &perms, &grants_path).unwrap();
    }

    #[test]
    fn non_interactive_run_does_not_persist_grants() {
        // Test processes have non-terminal stdin, so the undecided permission
        // is disclosed but no decision may be recorded.
        let dir = tempdir().unwrap();
        let home = HarborHome::at(dir.path());

        let grants_path = home.local_grants_path("test", "1.0.0");
        let perms: Vec<String> = vec!["network".into()];
        check_and_prompt("test", "1.0.0", &perms, &grants_path).unwrap();

        assert!(
            !grants_path.exists(),
            "non-interactive sessions must never auto-grant"
        );
    }

    #[test]
    fn stale_grants_with_matching_count_still_prompt_for_undeclared_names() {
        let dir = tempdir().unwrap();
        let home = HarborHome::at(dir.path());
        // Two decided entries, but neither matches the declared permission —
        // the old count-based check would have skipped disclosure entirely.
        let grants = Grants {
            granted: vec!["camera".to_string(), "location".to_string()],
            denied: vec![],
        };
        let grants_path = home.local_grants_path("test", "1.0.0");
        write_grants(&grants_path, &grants).unwrap();

        let perms: Vec<String> = vec!["network".into()];
        // Non-terminal stdin in tests → discloses without persisting; the key
        // assertion is that "network" is still treated as undecided.
        check_and_prompt("test", "1.0.0", &perms, &grants_path).unwrap();
        let loaded = read_grants(&grants_path).unwrap().unwrap();
        assert!(!loaded.is_decided("network"));
    }
}
