//! The `harbor push` validation + build pipeline (SPEC.md §8.1): an ordered,
//! fail-fast sequence of static checks culminating in a deterministic
//! `.harbor` archive. No step here ever executes package code or contacts
//! the network (§8.2, §8.4).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::errors::{ManifestError, ValidationError, ValidationWarning};
use crate::lockfile::{self, Lockfile, LockfileError};
use crate::manifest::{self, Manifest};
use crate::package::{self, PackageError};

/// SPEC.md §5.3: files (beyond `harbor.toml`, checked separately as part of
/// manifest parsing) that MUST be present at the package root.
const REQUIRED_FILES: &[&str] = &["README.md", "LICENSE"];

/// A hard failure at some step of the §8.1 pipeline. Each variant names the
/// step and the offending detail so the message is actionable on its own.
#[derive(Debug, Error)]
pub enum ValidateError {
    /// Step 1: `harbor.toml` itself is malformed TOML / missing a required field.
    #[error(transparent)]
    ManifestParse(#[from] ManifestError),

    /// Step 1: `harbor.toml` parsed but failed a semantic rule (§6, §8.1).
    #[error(transparent)]
    ManifestValidation(#[from] ValidationError),

    /// Step 2: an existing `harbor.lock` on disk does not parse.
    #[error("existing harbor.lock is malformed: {0}")]
    LockfileParse(LockfileError),

    /// Step 3: a required file (§5.3) is missing at the package root.
    #[error("required file {name:?} is missing at the package root (SPEC.md §5.3)")]
    MissingRequiredFile { name: String },

    /// Step 4: the declared `entrypoint` does not exist on disk.
    #[error("entrypoint {path:?} does not exist under the package root")]
    EntrypointMissing { path: String },

    /// Step 4: the declared `entrypoint` exists on disk but is excluded by
    /// `.gitignore`, which would produce a package that could never run
    /// (SPEC.md §5.4).
    #[error(
        "entrypoint {path:?} is not in the package file set — it is either \
         excluded by .gitignore or not a regular file (e.g. a symlink) — and \
         cannot be packaged (SPEC.md §5.4)"
    )]
    EntrypointExcluded { path: String },

    /// Step 5: a `[commands]` value is empty or all-whitespace.
    #[error("[commands] entry {name:?} has an empty value; commands must be non-empty strings")]
    EmptyCommand { name: String },

    /// Steps 6/8: file-set collection or archive building failed.
    #[error(transparent)]
    Package(#[from] PackageError),

    /// Step 7: `harbor.lock` generation failed (e.g. a declared native
    /// lockfile is missing on disk).
    #[error("failed to generate harbor.lock: {0}")]
    LockfileGenerate(LockfileError),

    /// I/O failure reading/writing a file that isn't covered by a more
    /// specific variant above.
    #[error("I/O error at {path}: {source}", path = path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// What `validate_and_build` produced on success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOutcome {
    /// Path to the freshly built `.harbor` archive.
    pub archive_path: PathBuf,
    /// The `package-checksum` recorded in `harbor.lock` (SPEC.md §7.3).
    pub package_checksum: String,
    /// Non-fatal manifest warnings (SPEC.md §17) to surface to the user.
    pub warnings: Vec<ValidationWarning>,
    /// The lockfile written to `root/harbor.lock`.
    pub lockfile: Lockfile,
}

/// Strip a leading `./` and normalize backslashes to forward slashes, so a
/// manifest's `entrypoint` value can be compared against archive-relative
/// paths (which always use forward slashes, no leading `./`).
fn normalize_rel_path(p: &str) -> String {
    let p = p.strip_prefix("./").unwrap_or(p);
    p.replace('\\', "/")
}

/// Run the full SPEC.md §8.1 validation + build pipeline against the package
/// rooted at `root` (the directory containing `harbor.toml`), stopping at the
/// first failure. `out` overrides the default archive path
/// (`<name>-<version>.harbor` written into `root`).
///
/// On success, `root/harbor.lock` has been (re)written and the archive at
/// the returned path exists and is complete; on failure, neither is left in
/// a partial state (the archive is built to a temp path and renamed into
/// place only once fully written).
pub fn validate_and_build(root: &Path, out: Option<&Path>) -> Result<BuildOutcome, ValidateError> {
    // --- Step 1: parse + validate harbor.toml ---
    let manifest_path = root.join("harbor.toml");
    let manifest_str = fs::read_to_string(&manifest_path).map_err(|e| ValidateError::Io {
        path: manifest_path.clone(),
        source: e,
    })?;
    let manifest = Manifest::from_toml_str(&manifest_str)?;
    let warnings = manifest::validate_manifest(&manifest)?;

    // --- Step 2: re-validate an existing harbor.lock, if any ---
    let lock_path = root.join("harbor.lock");
    if lock_path.is_file() {
        let lock_str = fs::read_to_string(&lock_path).map_err(|e| ValidateError::Io {
            path: lock_path.clone(),
            source: e,
        })?;
        Lockfile::from_toml_str(&lock_str).map_err(ValidateError::LockfileParse)?;
    }

    // --- Step 3: required files exist (harbor.toml already handled above) ---
    for name in REQUIRED_FILES {
        if !root.join(name).is_file() {
            return Err(ValidateError::MissingRequiredFile {
                name: (*name).to_string(),
            });
        }
    }

    // --- Collect the include set once; used by steps 4, 6, and 8. ---
    let mut files = package::collect_files(root)?;

    // --- Step 4: entrypoint exists, and is not .gitignore-excluded ---
    let entrypoint_rel = normalize_rel_path(&manifest.entrypoint);
    if !root.join(&manifest.entrypoint).is_file() {
        return Err(ValidateError::EntrypointMissing {
            path: manifest.entrypoint.clone(),
        });
    }
    if !files.iter().any(|(p, _)| *p == entrypoint_rel) {
        return Err(ValidateError::EntrypointExcluded {
            path: manifest.entrypoint.clone(),
        });
    }

    // --- Step 5: [commands] values are non-empty, non-whitespace strings ---
    for (name, value) in &manifest.commands {
        if value.trim().is_empty() {
            return Err(ValidateError::EmptyCommand { name: name.clone() });
        }
    }

    // --- Steps 6 + 7: compute package-checksum and generate harbor.lock ---
    // `lockfile::generate` computes the package-checksum internally (over
    // `files`, which already excludes any `harbor.lock` entry by name) and
    // returns a fully-populated Lockfile.
    let lock = lockfile::generate(&manifest, root, &files).map_err(ValidateError::LockfileGenerate)?;
    let lock_toml = lock
        .to_toml_string()
        .map_err(ValidateError::LockfileGenerate)?;
    fs::write(&lock_path, &lock_toml).map_err(|e| ValidateError::Io {
        path: lock_path.clone(),
        source: e,
    })?;

    // The freshly written harbor.lock MUST be in the archive (§5.3) even on
    // a first-ever build, when `files` (collected before we wrote it) didn't
    // contain it yet. Replace any stale entry so the archive always reads
    // the just-written contents.
    files.retain(|(p, _)| p != "harbor.lock");
    files.push(("harbor.lock".to_string(), lock_path.clone()));
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

    let package_checksum = lock
        .package_checksum
        .clone()
        .expect("lockfile::generate always populates package_checksum");

    // --- Step 8: build the archive, via a temp file + rename so a failure
    // never leaves a partial archive at the final path. ---
    let archive_name = format!("{}-{}.harbor", manifest.name, manifest.version);
    let final_path = out.map(PathBuf::from).unwrap_or_else(|| root.join(&archive_name));
    let tmp_path = final_path.with_extension("harbor.tmp");

    if let Err(e) = package::build_archive(root, &files, &tmp_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e.into());
    }
    fs::rename(&tmp_path, &final_path).map_err(|e| ValidateError::Io {
        path: final_path.clone(),
        source: e,
    })?;

    Ok(BuildOutcome {
        archive_path: final_path,
        package_checksum,
        warnings,
        lockfile: lock,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_file(dir: &Path, rel: &str, contents: &str) {
        let full = dir.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full, contents).unwrap();
    }

    /// A minimal, fully valid package: harbor.toml + README + LICENSE +
    /// entrypoint, no [commands], no native lockfile declared.
    fn scaffold_valid_package(dir: &Path) {
        write_file(
            dir,
            "harbor.toml",
            r#"
spec-version = 1
name = "my-agent"
version = "1.0.0"
description = "A test agent."
package-type = "agent"
language = "python"
runtime = ">=3.11,<4"
entrypoint = "src/main.py"
license = "MIT"
permissions = []
features = []

[author]
name = "Jane Doe"
email = "jane@example.com"

[dependencies]
manifest = "pyproject.toml"
"#,
        );
        write_file(dir, "README.md", "# my-agent\n");
        write_file(dir, "LICENSE", "MIT License\n");
        write_file(dir, "src/main.py", "print('hi')\n");
        write_file(dir, "pyproject.toml", "[project]\nname = \"my-agent\"\n");
    }

    #[test]
    fn happy_path_builds_archive_and_writes_lockfile() {
        let dir = tempdir().unwrap();
        scaffold_valid_package(dir.path());

        let outcome = validate_and_build(dir.path(), None).expect("should succeed");

        assert!(outcome.archive_path.is_file());
        assert_eq!(
            outcome.archive_path.file_name().unwrap().to_str().unwrap(),
            "my-agent-1.0.0.harbor"
        );
        assert!(outcome.package_checksum.starts_with("sha256:"));
        assert!(outcome.warnings.is_empty());

        let lock_str = fs::read_to_string(dir.path().join("harbor.lock")).unwrap();
        let lock = Lockfile::from_toml_str(&lock_str).unwrap();
        assert!(lock.package_checksum.is_some());
        assert_eq!(lock.package_checksum, Some(outcome.package_checksum.clone()));
    }

    #[test]
    fn manifest_error_surfaces_even_when_readme_is_also_missing() {
        // Step 1 (manifest) must fail before step 3 (required files) is
        // even reached, even though both are broken here.
        let dir = tempdir().unwrap();
        scaffold_valid_package(dir.path());
        fs::remove_file(dir.path().join("README.md")).unwrap();
        write_file(dir.path(), "harbor.toml", "not valid toml {{{");

        let err = validate_and_build(dir.path(), None).unwrap_err();
        assert!(
            matches!(err, ValidateError::ManifestParse(_)),
            "expected a manifest parse error first, got: {err:?}"
        );
    }

    #[test]
    fn missing_readme_is_distinct_error() {
        let dir = tempdir().unwrap();
        scaffold_valid_package(dir.path());
        fs::remove_file(dir.path().join("README.md")).unwrap();

        let err = validate_and_build(dir.path(), None).unwrap_err();
        match err {
            ValidateError::MissingRequiredFile { name } => assert_eq!(name, "README.md"),
            other => panic!("expected MissingRequiredFile, got: {other:?}"),
        }
    }

    #[test]
    fn missing_license_is_distinct_error() {
        let dir = tempdir().unwrap();
        scaffold_valid_package(dir.path());
        fs::remove_file(dir.path().join("LICENSE")).unwrap();

        let err = validate_and_build(dir.path(), None).unwrap_err();
        match err {
            ValidateError::MissingRequiredFile { name } => assert_eq!(name, "LICENSE"),
            other => panic!("expected MissingRequiredFile, got: {other:?}"),
        }
    }

    #[test]
    fn gitignored_entrypoint_is_distinct_error() {
        let dir = tempdir().unwrap();
        scaffold_valid_package(dir.path());
        write_file(dir.path(), ".gitignore", "src/main.py\n");

        let err = validate_and_build(dir.path(), None).unwrap_err();
        match err {
            ValidateError::EntrypointExcluded { path } => assert_eq!(path, "src/main.py"),
            other => panic!("expected EntrypointExcluded, got: {other:?}"),
        }
    }

    #[test]
    fn missing_entrypoint_is_distinct_error() {
        let dir = tempdir().unwrap();
        scaffold_valid_package(dir.path());
        fs::remove_file(dir.path().join("src/main.py")).unwrap();

        let err = validate_and_build(dir.path(), None).unwrap_err();
        assert!(matches!(err, ValidateError::EntrypointMissing { .. }));
    }

    #[test]
    fn empty_command_value_is_error() {
        let dir = tempdir().unwrap();
        scaffold_valid_package(dir.path());
        let toml = fs::read_to_string(dir.path().join("harbor.toml")).unwrap();
        let with_commands = format!("{toml}\n[commands]\ntest = \"   \"\n");
        write_file(dir.path(), "harbor.toml", &with_commands);

        let err = validate_and_build(dir.path(), None).unwrap_err();
        match err {
            ValidateError::EmptyCommand { name } => assert_eq!(name, "test"),
            other => panic!("expected EmptyCommand, got: {other:?}"),
        }
    }

    #[test]
    fn malformed_existing_lockfile_is_distinct_error() {
        let dir = tempdir().unwrap();
        scaffold_valid_package(dir.path());
        write_file(dir.path(), "harbor.lock", "not valid toml {{{");

        let err = validate_and_build(dir.path(), None).unwrap_err();
        assert!(matches!(err, ValidateError::LockfileParse(_)));
    }

    #[test]
    fn archive_never_contains_gitignored_secret_or_itself() {
        let dir = tempdir().unwrap();
        scaffold_valid_package(dir.path());
        write_file(dir.path(), ".gitignore", "secret.txt\n");
        write_file(dir.path(), "secret.txt", "hush");

        let outcome = validate_and_build(dir.path(), None).expect("should succeed");

        let f = fs::File::open(&outcome.archive_path).unwrap();
        let decoder = flate2::read::GzDecoder::new(f);
        let mut archive = tar::Archive::new(decoder);
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();

        assert!(names.contains(&"harbor.toml".to_string()));
        assert!(names.contains(&"harbor.lock".to_string()));
        assert!(names.contains(&"README.md".to_string()));
        assert!(names.contains(&"LICENSE".to_string()));
        assert!(names.contains(&"src/main.py".to_string()));
        assert!(!names.contains(&"secret.txt".to_string()), "got: {names:?}");
        assert!(!names.iter().any(|n| n.ends_with(".harbor")), "got: {names:?}");
    }

    #[test]
    fn rebuilding_does_not_ingest_the_previous_archive() {
        let dir = tempdir().unwrap();
        scaffold_valid_package(dir.path());

        let first = validate_and_build(dir.path(), None).expect("first build should succeed");
        assert!(first.archive_path.is_file());

        // Build again; the archive produced the first time must not appear
        // inside the second archive's contents.
        let second = validate_and_build(dir.path(), None).expect("second build should succeed");
        let f = fs::File::open(&second.archive_path).unwrap();
        let decoder = flate2::read::GzDecoder::new(f);
        let mut archive = tar::Archive::new(decoder);
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(!names.iter().any(|n| n.ends_with(".harbor")), "got: {names:?}");
    }
}
