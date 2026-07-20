//! `xelian init`: generates a `xelian.toml` + `xelian.lock` skeleton plus the
//! starter files a runnable package needs (entrypoint, native dependency
//! manifest, README, LICENSE) in a target directory (SPEC.md §13.1), so
//! `init` → `push` → `run` works with zero edits.
//!
//! `xelian init` MUST NOT contact the network or the registry — all I/O in
//! this module is confined to writing skeleton files under the given
//! directory, and it never overwrites a file a user already has (starter
//! files are written only when absent; only `xelian.toml`/`xelian.lock` are
//! regenerated under `--force`).

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
    /// `xelian.toml` already exists in the target directory and `force` was
    /// not set. Nothing is written in this case (an existing `xelian.lock`,
    /// if any, is also left untouched).
    #[error(
        "{path} already exists; refusing to overwrite (pass --force to overwrite both xelian.toml and xelian.lock)",
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

    /// The generated `xelian.toml`/`xelian.lock` failed to parse, validate,
    /// or serialize. This indicates a bug in the skeleton template itself
    /// (not user input) — the whole point of `xelian init` is to produce
    /// something valid to start from.
    #[error("internal error: generated package skeleton is invalid: {0}")]
    InvalidSkeleton(String),
}

/// Paths and metadata produced by [`init_package`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOutcome {
    pub manifest_path: PathBuf,
    pub lockfile_path: PathBuf,
    /// The `name` written into the generated `xelian.toml`.
    pub name: String,
    /// Whether `name` is the generic placeholder (i.e. the directory name
    /// didn't satisfy §19.3) rather than derived from the directory.
    pub name_is_placeholder: bool,
    /// Starter files created alongside the manifest (entrypoint, native
    /// dependency manifest, README, LICENSE). Only files that did not already
    /// exist are created and listed here — existing user files are never
    /// touched, even under `--force`.
    pub scaffolded: Vec<PathBuf>,
}

/// Whether `name` satisfies the package naming rules (SPEC.md §19.3):
/// lowercase ASCII letters, digits, `_`, `-`; 3-64 characters.
///
/// `pub(crate)`: reused by `github.rs` (§12.2 step 3) to validate a package
/// name slugified from a repository name, rather than duplicating this rule
/// a third time.
pub(crate) fn is_valid_package_name(name: &str) -> bool {
    let valid_charset = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    valid_charset && name.len() >= 3 && name.len() <= 64
}

/// Renders the `xelian.toml` skeleton contents (SPEC.md §13.1, §6.1).
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

/// Renders a minimal, immediately-runnable agent entrypoint (SPEC.md §9.10.1).
///
/// The generated `xelian init` package must `push` and `run` with zero edits,
/// so this is a working echo agent: it inherits the terminal's stdin/stdout
/// and replies to each line. A user replaces the body with real logic.
fn render_entrypoint_py(name: &str) -> String {
    format!(
        r#"# Starter Xelian agent for `{name}`.
#
# `xelian run <you>/{name}` connects your terminal straight to this program's
# stdin/stdout. Replace the echo below with your agent's real logic.
import sys


def main() -> None:
    # Xelian prints the readiness banner and the `> ` prompt before handing
    # over the terminal, so an agent should not print its own — two "ready"
    # lines read as a glitch. Just print a `> ` before each subsequent turn.
    for line in sys.stdin:
        message = line.rstrip("\n")
        if not message:
            continue
        print(f"you said: {{message}}", flush=True)
        print("> ", end="", flush=True)


if __name__ == "__main__":
    main()
"#
    )
}

/// Renders the native Python dependency manifest referenced by
/// `[dependencies] manifest = "pyproject.toml"` in the generated `xelian.toml`.
fn render_pyproject_toml(name: &str) -> String {
    format!(
        r#"[project]
name = "{name}"
version = "0.1.0"
requires-python = ">=3.11"
dependencies = []
"#
    )
}

/// Renders the required `README.md` (SPEC.md §5.3).
fn render_readme_md(name: &str) -> String {
    format!(
        r#"# {name}

TODO: describe your package.

## Run it

```bash
xelian run <you>/{name}
```
"#
    )
}

/// Renders the required `LICENSE` (SPEC.md §5.3) — MIT, matching the manifest's
/// default `license = "MIT"`.
fn render_license() -> String {
    r#"MIT License

Copyright (c) TODO: Your Name

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"#
    .to_string()
}

/// Writes `contents` to `path` only if `path` does not already exist, creating
/// parent directories as needed. Returns `Ok(true)` if the file was created,
/// `Ok(false)` if it already existed and was left untouched. Existing user
/// files are never overwritten — not even under `xelian init --force`, which
/// regenerates only `xelian.toml`/`xelian.lock`.
fn write_if_absent(path: &Path, contents: &str) -> Result<bool, InitError> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| InitError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    std::fs::write(path, contents).map_err(|e| InitError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    Ok(true)
}

/// Generate a `xelian.toml` + `xelian.lock` package skeleton in `dir`
/// (SPEC.md §13.1). Performs no network activity.
///
/// The package `name` is derived from `dir`'s final path component when it
/// satisfies the naming rules (§19.3); otherwise the generated manifest gets
/// the placeholder name `"my-package"` with a `# TODO` comment, and
/// [`InitOutcome::name_is_placeholder`] is `true`.
///
/// If `xelian.toml` or `xelian.lock` already exists in `dir` and `force` is
/// `false`, returns [`InitError::AlreadyExists`] and changes nothing on
/// disk. With `force: true`, both files are (re)written unconditionally.
pub fn init_package(dir: &Path, force: bool) -> Result<InitOutcome, InitError> {
    let manifest_path = dir.join("xelian.toml");
    let lockfile_path = dir.join("xelian.lock");

    if !force {
        if manifest_path.exists() {
            return Err(InitError::AlreadyExists {
                path: manifest_path,
            });
        }
        if lockfile_path.exists() {
            return Err(InitError::AlreadyExists {
                path: lockfile_path,
            });
        }
    }

    let (name, name_is_placeholder) = match dir.file_name().and_then(|s| s.to_str()) {
        Some(candidate) if is_valid_package_name(candidate) => (candidate.to_string(), false),
        _ => (PLACEHOLDER_NAME.to_string(), true),
    };

    let manifest_toml = render_manifest_toml(&name, name_is_placeholder);

    // Sanity-check our own template before writing anything: the generated
    // xelian.toml MUST parse and validate cleanly (this is also asserted by
    // tests below), so a failure here means a bug in this function, not in
    // the user's input.
    let manifest = Manifest::from_toml_str(&manifest_toml)
        .map_err(|e| InitError::InvalidSkeleton(e.to_string()))?;
    manifest::validate_manifest(&manifest)
        .map_err(|e| InitError::InvalidSkeleton(e.to_string()))?;

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

    // Scaffold the starter files the manifest references (entrypoint + native
    // manifest) plus the two required root files (§5.3), so `init` → `push` →
    // `run` works with zero edits. Never clobber files a user already has.
    let scaffold: [(PathBuf, String); 4] = [
        (dir.join("src").join("main.py"), render_entrypoint_py(&name)),
        (dir.join("pyproject.toml"), render_pyproject_toml(&name)),
        (dir.join("README.md"), render_readme_md(&name)),
        (dir.join("LICENSE"), render_license()),
    ];
    let mut scaffolded = Vec::new();
    for (path, contents) in &scaffold {
        if write_if_absent(path, contents)? {
            scaffolded.push(path.clone());
        }
    }

    Ok(InitOutcome {
        manifest_path,
        lockfile_path,
        name,
        name_is_placeholder,
        scaffolded,
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
        let manifest =
            Manifest::from_toml_str(&manifest_str).expect("generated manifest must parse");
        let warnings = manifest::validate_manifest(&manifest)
            .expect("generated manifest must validate with 0 errors");
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
        let manifest =
            Manifest::from_toml_str(&manifest_str).expect("generated manifest must parse");
        manifest::validate_manifest(&manifest)
            .expect("generated manifest must validate with 0 errors");
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
        let manifest_path = dir.join("xelian.toml");
        std::fs::write(&manifest_path, "custom content").unwrap();

        let err = init_package(&dir, false).expect_err("should refuse to overwrite");
        assert!(matches!(err, InitError::AlreadyExists { .. }));

        let contents = std::fs::read_to_string(&manifest_path).unwrap();
        assert_eq!(
            contents, "custom content",
            "existing xelian.toml must be untouched"
        );
        assert!(
            !dir.join("xelian.lock").exists(),
            "xelian.lock must not be created when the guard trips"
        );
    }

    #[test]
    fn existing_files_with_force_are_overwritten() {
        let (_base, dir) = named_dir("weather");
        std::fs::write(dir.join("xelian.toml"), "custom content").unwrap();
        std::fs::write(dir.join("xelian.lock"), "custom lock").unwrap();

        let outcome = init_package(&dir, true).expect("force init should succeed");

        let manifest_contents = std::fs::read_to_string(&outcome.manifest_path).unwrap();
        assert_ne!(manifest_contents, "custom content");
        assert!(manifest_contents.contains("name = \"weather\""));

        let lock_contents = std::fs::read_to_string(&outcome.lockfile_path).unwrap();
        assert_ne!(lock_contents, "custom lock");
    }

    #[test]
    fn scaffolds_runnable_starter_files() {
        let (_base, dir) = named_dir("weather");

        let outcome = init_package(&dir, false).expect("init should succeed");

        // The four starter files the manifest references / §5.3 requires.
        for rel in ["src/main.py", "pyproject.toml", "README.md", "LICENSE"] {
            assert!(
                dir.join(rel).is_file(),
                "init should scaffold {rel}, but it is missing"
            );
        }
        // All four are reported as freshly created.
        assert_eq!(outcome.scaffolded.len(), 4);

        // The generated package validates and builds with no edits — the whole
        // point of a runnable skeleton.
        crate::validate::validate_and_build(&dir, None)
            .expect("a freshly-init'd package must validate and build");

        // The entrypoint the manifest points at actually exists on disk.
        let manifest_str = std::fs::read_to_string(&outcome.manifest_path).unwrap();
        let manifest = Manifest::from_toml_str(&manifest_str).unwrap();
        let entrypoint = dir.join(&manifest.entrypoint);
        assert!(entrypoint.is_file());

        // If a Python interpreter is present, the generated entrypoint must
        // compile — validation alone never executes it, so only this guards
        // against a template that emits syntactically-broken Python (e.g. a
        // raw-string delimiter eating a docstring quote).
        if let Ok(out) = std::process::Command::new("python3")
            .arg("-c")
            .arg("import py_compile,sys; py_compile.compile(sys.argv[1], doraise=True)")
            .arg(&entrypoint)
            .output()
        {
            assert!(
                out.status.success(),
                "generated entrypoint does not compile:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn scaffolding_never_clobbers_existing_user_files() {
        let (_base, dir) = named_dir("weather");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("main.py"), "print('mine')\n").unwrap();
        std::fs::write(dir.join("README.md"), "# my real readme\n").unwrap();

        let outcome = init_package(&dir, false).expect("init should succeed");

        // Pre-existing files are left byte-for-byte intact...
        assert_eq!(
            std::fs::read_to_string(dir.join("src").join("main.py")).unwrap(),
            "print('mine')\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("README.md")).unwrap(),
            "# my real readme\n"
        );
        // ...and only the genuinely-missing ones are reported as scaffolded.
        assert_eq!(outcome.scaffolded.len(), 2); // pyproject.toml + LICENSE
    }

    #[test]
    fn already_exists_error_message_mentions_force() {
        let (_base, dir) = named_dir("weather");
        std::fs::write(dir.join("xelian.toml"), "x").unwrap();

        let err = init_package(&dir, false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("--force"), "got: {msg}");
        assert!(msg.contains("xelian.toml"), "got: {msg}");
    }
}
