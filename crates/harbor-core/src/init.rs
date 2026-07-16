//! `harbor init`: generates a `harbor.toml` + `harbor.lock` skeleton in a
//! target directory (SPEC.md §13.1).
//!
//! `harbor init` MUST NOT contact the network or the registry — nothing in
//! this module performs any I/O beyond reading/writing the two skeleton
//! files under the given directory.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::lockfile;
use crate::manifest::{self, Manifest};

/// Placeholder package name used when the target directory's name doesn't
/// satisfy the naming rules (SPEC.md §19.3).
pub const PLACEHOLDER_NAME: &str = "my-package";

/// Errors that can occur while generating a package skeleton.
#[derive(Debug, Error)]
pub enum InitError {
    /// `harbor.toml` already exists in the target directory and `force` was
    /// not set. Nothing is written in this case (an existing `harbor.lock`,
    /// if any, is also left untouched).
    #[error(
        "{path} already exists; refusing to overwrite (pass --force to overwrite both harbor.toml and harbor.lock)",
        path = path.display()
    )]
    AlreadyExists { path: PathBuf },

    /// I/O failure while writing a generated file.
    #[error("I/O error writing {path}: {source}", path = path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The generated `harbor.toml`/`harbor.lock` failed to parse, validate,
    /// or serialize. This indicates a bug in the skeleton template itself
    /// (not user input) — the whole point of `harbor init` is to produce
    /// something valid to start from.
    #[error("internal error: generated package skeleton is invalid: {0}")]
    InvalidSkeleton(String),
}

/// Paths and metadata produced by [`init_package`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOutcome {
    pub manifest_path: PathBuf,
    pub lockfile_path: PathBuf,
    /// The `name` written into the generated `harbor.toml`.
    pub name: String,
    /// Whether `name` is the generic placeholder (i.e. the directory name
    /// didn't satisfy §19.3) rather than derived from the directory.
    pub name_is_placeholder: bool,
}

/// Whether `name` satisfies the package naming rules (SPEC.md §19.3):
/// lowercase ASCII letters, digits, `_`, `-`; 3-64 characters.
fn is_valid_package_name(name: &str) -> bool {
    let valid_charset = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    valid_charset && name.len() >= 3 && name.len() <= 64
}

/// Renders the `harbor.toml` skeleton contents (SPEC.md §13.1, §6.1).
///
/// All required fields (§6.1) get valid defaults; fields with no sensible
/// default (`name` when unusable, `description`, `[author]`) are marked
/// with a `# TODO` comment so a user editing the file finds them immediately.
fn render_manifest_toml(name: &str, name_is_placeholder: bool) -> String {
    let name_line = if name_is_placeholder {
        format!(
            "# TODO: choose a package name (lowercase letters/digits/_/-, 3-64 chars)\nname = \"{name}\""
        )
    } else {
        format!("name = \"{name}\"")
    };

    format!(
        r#"spec-version = 1
{name_line}
version = "0.1.0"
# TODO: describe your package
description = "TODO: describe your package"
package-type = "agent"
language = "python"
runtime = ">=3.11,<4"
entrypoint = "src/main.py"
license = "MIT"
permissions = []
features = []

[author]
# TODO: fill in your name and contact email
name = "TODO: Your Name"
email = "you@example.com"

[dependencies]
manifest = "pyproject.toml"
"#
    )
}

/// Generate a `harbor.toml` + `harbor.lock` package skeleton in `dir`
/// (SPEC.md §13.1). Performs no network activity.
///
/// The package `name` is derived from `dir`'s final path component when it
/// satisfies the naming rules (§19.3); otherwise the generated manifest gets
/// the placeholder name `"my-package"` with a `# TODO` comment, and
/// [`InitOutcome::name_is_placeholder`] is `true`.
///
/// If `harbor.toml` already exists in `dir` and `force` is `false`, returns
/// [`InitError::AlreadyExists`] and changes nothing on disk — an existing
/// `harbor.lock` is left untouched too, even though only `harbor.toml`'s
/// presence is checked. With `force: true`, both files are (re)written
/// unconditionally.
pub fn init_package(dir: &Path, force: bool) -> Result<InitOutcome, InitError> {
    let manifest_path = dir.join("harbor.toml");
    let lockfile_path = dir.join("harbor.lock");

    if manifest_path.exists() && !force {
        return Err(InitError::AlreadyExists { path: manifest_path });
    }

    let (name, name_is_placeholder) = match dir.file_name().and_then(|s| s.to_str()) {
        Some(candidate) if is_valid_package_name(candidate) => (candidate.to_string(), false),
        _ => (PLACEHOLDER_NAME.to_string(), true),
    };

    let manifest_toml = render_manifest_toml(&name, name_is_placeholder);

    // Sanity-check our own template before writing anything: the generated
    // harbor.toml MUST parse and validate cleanly (this is also asserted by
    // tests below), so a failure here means a bug in this function, not in
    // the user's input.
    let manifest = Manifest::from_toml_str(&manifest_toml)
        .map_err(|e| InitError::InvalidSkeleton(e.to_string()))?;
    manifest::validate_manifest(&manifest).map_err(|e| InitError::InvalidSkeleton(e.to_string()))?;

    let lock = lockfile::skeleton(&manifest);
    let lock_toml = lock
        .to_toml_string()
        .map_err(|e| InitError::InvalidSkeleton(e.to_string()))?;

    std::fs::write(&manifest_path, &manifest_toml).map_err(|e| InitError::Io {
        path: manifest_path.clone(),
        source: e,
    })?;
    std::fs::write(&lockfile_path, &lock_toml).map_err(|e| InitError::Io {
        path: lockfile_path.clone(),
        source: e,
    })?;

    Ok(InitOutcome {
        manifest_path,
        lockfile_path,
        name,
        name_is_placeholder,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Creates a subdirectory with an exact chosen name inside a fresh
    /// tempdir (plain `tempfile::tempdir()` names are random, but these
    /// tests need to control the directory name that `init_package` derives
    /// `name` from). The returned `TempDir` must be kept alive for the
    /// duration of the test — dropping it removes the subdirectory too.
    fn named_dir(name: &str) -> (tempfile::TempDir, PathBuf) {
        let base = tempdir().expect("create base tempdir");
        let dir = base.path().join(name);
        std::fs::create_dir(&dir).expect("create named subdir");
        (base, dir)
    }

    #[test]
    fn valid_dir_name_becomes_package_name_and_files_validate() {
        let (_base, dir) = named_dir("weather");

        let outcome = init_package(&dir, false).expect("init should succeed");

        assert_eq!(outcome.name, "weather");
        assert!(!outcome.name_is_placeholder);
        assert!(outcome.manifest_path.is_file());
        assert!(outcome.lockfile_path.is_file());

        let manifest_str = std::fs::read_to_string(&outcome.manifest_path).unwrap();
        let manifest = Manifest::from_toml_str(&manifest_str).expect("generated manifest must parse");
        let warnings =
            manifest::validate_manifest(&manifest).expect("generated manifest must validate with 0 errors");
        assert!(warnings.is_empty());
        assert_eq!(manifest.name, "weather");
    }

    #[test]
    fn invalid_dir_name_falls_back_to_placeholder() {
        let (_base, dir) = named_dir("My Project!");

        let outcome = init_package(&dir, false).expect("init should succeed");

        assert!(outcome.name_is_placeholder);
        assert_eq!(outcome.name, PLACEHOLDER_NAME);

        let manifest_str = std::fs::read_to_string(&outcome.manifest_path).unwrap();
        let manifest = Manifest::from_toml_str(&manifest_str).expect("generated manifest must parse");
        manifest::validate_manifest(&manifest).expect("generated manifest must validate with 0 errors");
        assert_eq!(manifest.name, "my-package");
    }

    #[test]
    fn too_short_dir_name_falls_back_to_placeholder() {
        let (_base, dir) = named_dir("ab");
        let outcome = init_package(&dir, false).expect("init should succeed");
        assert!(outcome.name_is_placeholder);
        assert_eq!(outcome.name, PLACEHOLDER_NAME);
    }

    #[test]
    fn existing_manifest_without_force_errors_and_changes_nothing() {
        let (_base, dir) = named_dir("weather");
        let manifest_path = dir.join("harbor.toml");
        std::fs::write(&manifest_path, "custom content").unwrap();

        let err = init_package(&dir, false).expect_err("should refuse to overwrite");
        assert!(matches!(err, InitError::AlreadyExists { .. }));

        let contents = std::fs::read_to_string(&manifest_path).unwrap();
        assert_eq!(contents, "custom content", "existing harbor.toml must be untouched");
        assert!(
            !dir.join("harbor.lock").exists(),
            "harbor.lock must not be created when the guard trips"
        );
    }

    #[test]
    fn existing_files_with_force_are_overwritten() {
        let (_base, dir) = named_dir("weather");
        std::fs::write(dir.join("harbor.toml"), "custom content").unwrap();
        std::fs::write(dir.join("harbor.lock"), "custom lock").unwrap();

        let outcome = init_package(&dir, true).expect("force init should succeed");

        let manifest_contents = std::fs::read_to_string(&outcome.manifest_path).unwrap();
        assert_ne!(manifest_contents, "custom content");
        assert!(manifest_contents.contains("name = \"weather\""));

        let lock_contents = std::fs::read_to_string(&outcome.lockfile_path).unwrap();
        assert_ne!(lock_contents, "custom lock");
    }

    #[test]
    fn already_exists_error_message_mentions_force() {
        let (_base, dir) = named_dir("weather");
        std::fs::write(dir.join("harbor.toml"), "x").unwrap();

        let err = init_package(&dir, false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("--force"), "got: {msg}");
        assert!(msg.contains("harbor.toml"), "got: {msg}");
    }
}
