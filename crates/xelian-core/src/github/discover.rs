//! Monorepo subpackage discovery for `xelian add` (SPEC.md §12.2).
//!
//! Pointing `xelian add` at a repository *root* that is really a monorepo
//! (`upstash/context7`, `supabase/mcp`, `crewAIInc/crewAI`) previously failed:
//! the root has no runnable entrypoint of its own, only a `workspaces` list or
//! a `packages/` directory. That is a dead end the user has to resolve by
//! hand, even though the answer — which subdirectory holds the package — is
//! sitting right there in the repository.
//!
//! This module scans a root checkout for runnable subpackages, so `xelian add`
//! can either descend into the single obvious one automatically, or tell the
//! user the exact subdirectory URLs to choose between.

use std::path::Path;

use crate::manifest::{Language, PackageType};

use super::{detect, entrypoint, pkgtype};

/// A runnable package found inside a monorepo root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subpackage {
    /// Path relative to the repository root, e.g. `packages/mcp`.
    pub subdir: String,
    pub language: Language,
    pub package_type: PackageType,
}

/// Directories a monorepo conventionally keeps its member packages in, checked
/// in addition to any `workspaces`/`packages` globs declared in the root
/// `package.json`.
const CONVENTIONAL_PARENTS: &[&str] = &["packages", "apps", "lib", "src", "servers", "mcp-servers"];

/// Find every runnable subpackage under a repository root.
///
/// A subdirectory qualifies when a language is detectable in it *and* an
/// entrypoint is inferable — i.e. it is a package `xelian add` could actually
/// run. Results are sorted so ordering is deterministic across runs.
pub fn find_subpackages(root: &Path) -> Vec<Subpackage> {
    let mut candidates: Vec<String> = Vec::new();

    // Workspace globs declared by the root package.json (`packages/*`).
    for glob in workspace_globs(root) {
        candidates.extend(expand_single_star_glob(root, &glob));
    }

    // Conventional parent directories, whether or not they were declared.
    for parent in CONVENTIONAL_PARENTS {
        let dir = root.join(parent);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        candidates.push(format!("{parent}/{name}"));
                    }
                }
            }
        }
    }

    candidates.sort();
    candidates.dedup();

    let mut found: Vec<Subpackage> = Vec::new();
    for subdir in candidates {
        let dir = root.join(&subdir);
        let Ok(language) = detect::detect_language(&dir) else {
            continue;
        };
        // A subpackage's own name is the best hint for its entrypoint.
        let hint = subdir.rsplit('/').next().unwrap_or(&subdir);
        if entrypoint::infer(&dir, language, hint).is_some() {
            let package_type = pkgtype::infer(&dir, language);
            found.push(Subpackage {
                subdir,
                language,
                package_type,
            });
        }
    }
    found
}

/// Choose the single subpackage to descend into automatically, if the choice
/// is unambiguous.
///
/// "Unambiguous" means exactly one runnable subpackage, or — when several
/// exist — exactly one that is an MCP server (`xelian add` of a monorepo is
/// overwhelmingly someone wanting *the* server it hosts). Anything else is
/// genuinely the user's choice and returns `None`.
pub fn pick_unambiguous(subpackages: &[Subpackage]) -> Option<&Subpackage> {
    match subpackages.len() {
        0 => None,
        1 => Some(&subpackages[0]),
        _ => {
            let mcp: Vec<&Subpackage> = subpackages
                .iter()
                .filter(|s| s.package_type == PackageType::Mcp)
                .collect();
            if mcp.len() == 1 {
                Some(mcp[0])
            } else {
                None
            }
        }
    }
}

/// The `workspaces` globs from a root `package.json`, if any.
fn workspace_globs(root: &Path) -> Vec<String> {
    let Ok(contents) = std::fs::read_to_string(root.join("package.json")) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return Vec::new();
    };
    match v.get("workspaces") {
        // `"workspaces": ["packages/*"]`
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        // `"workspaces": { "packages": ["packages/*"] }`
        Some(serde_json::Value::Object(o)) => o
            .get("packages")
            .and_then(|p| p.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Expand a `parent/*` glob into its immediate child directories. Only the
/// single-trailing-star form is supported — the only shape npm workspaces use
/// in practice — and anything else is ignored rather than mishandled.
fn expand_single_star_glob(root: &Path, glob: &str) -> Vec<String> {
    let Some(parent) = glob.strip_suffix("/*") else {
        // A concrete path (no glob) is itself a candidate.
        return if glob.contains('*') {
            Vec::new()
        } else {
            vec![glob.to_string()]
        };
    };
    if parent.contains('*') {
        return Vec::new();
    }

    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root.join(parent)) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    out.push(format!("{parent}/{name}"));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(dir: &Path, name: &str, contents: &str) {
        let full = dir.join(name);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, contents).unwrap();
    }

    #[test]
    fn finds_a_runnable_subpackage_under_a_workspace_glob() {
        // context7 shape: root declares packages/*, the mcp member is runnable.
        let d = tempdir().unwrap();
        write(d.path(), "package.json", r#"{"workspaces":["packages/*"]}"#);
        write(
            d.path(),
            "packages/mcp/package.json",
            r#"{"name":"@x/mcp","bin":{"x":"dist/index.js"},"scripts":{"build":"tsc"},"dependencies":{"@modelcontextprotocol/sdk":"^1"}}"#,
        );
        write(d.path(), "packages/mcp/src/index.ts", "export {}\n");
        write(
            d.path(),
            "packages/sdk/package.json",
            r#"{"name":"@x/sdk"}"#,
        );

        let found = find_subpackages(d.path());
        assert_eq!(found.len(), 1, "only the runnable member: {found:?}");
        assert_eq!(found[0].subdir, "packages/mcp");
        assert_eq!(found[0].package_type, PackageType::Mcp);
    }

    #[test]
    fn a_single_runnable_subpackage_is_picked_automatically() {
        let subs = vec![Subpackage {
            subdir: "packages/mcp".into(),
            language: Language::Node,
            package_type: PackageType::Mcp,
        }];
        assert_eq!(pick_unambiguous(&subs).unwrap().subdir, "packages/mcp");
    }

    #[test]
    fn the_single_mcp_server_wins_among_several_packages() {
        let subs = vec![
            Subpackage {
                subdir: "packages/cli".into(),
                language: Language::Node,
                package_type: PackageType::Agent,
            },
            Subpackage {
                subdir: "packages/mcp".into(),
                language: Language::Node,
                package_type: PackageType::Mcp,
            },
            Subpackage {
                subdir: "packages/sdk".into(),
                language: Language::Node,
                package_type: PackageType::Agent,
            },
        ];
        assert_eq!(pick_unambiguous(&subs).unwrap().subdir, "packages/mcp");
    }

    #[test]
    fn several_mcp_servers_stay_the_users_choice() {
        // supabase shape: two mcp-server-* members. Xelian must not guess.
        let subs = vec![
            Subpackage {
                subdir: "packages/mcp-server-postgrest".into(),
                language: Language::Node,
                package_type: PackageType::Mcp,
            },
            Subpackage {
                subdir: "packages/mcp-server-supabase".into(),
                language: Language::Node,
                package_type: PackageType::Mcp,
            },
        ];
        assert_eq!(pick_unambiguous(&subs), None);
    }

    #[test]
    fn a_flat_repo_with_no_subpackages_finds_nothing() {
        let d = tempdir().unwrap();
        write(d.path(), "package.json", r#"{"name":"solo"}"#);
        assert!(find_subpackages(d.path()).is_empty());
    }
}
