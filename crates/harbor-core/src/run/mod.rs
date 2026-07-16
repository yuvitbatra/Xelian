//! The `harbor run` local-archive preparation pipeline (SPEC.md §9): checksum
//! verification, safe extraction, and re-validation of a `.harbor` archive.
//!
//! V1 scope (decision 2026-07-16): this module only prepares a package from a
//! LOCAL `.harbor` archive path — the registry-ref and GitHub-URL target
//! forms of §9.2 are handled elsewhere (later phases) and are not this
//! module's concern. Launching the prepared package (§9.7 onward) is also a
//! later phase; [`run_local_archive`] only gets a package validated and
//! extracted on disk.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::cache::HarborHome;
use crate::errors::{ManifestError, ValidationError, ValidationWarning};
use crate::lockfile::{compute_package_checksum_from_bytes, Lockfile, LockfileError};
use crate::manifest::{self, Manifest};

pub mod extract;

/// Errors that can occur while preparing a local `.harbor` archive for
/// launch (SPEC.md §9.4–§9.6.1).
#[derive(Debug, Error)]
pub enum RunError {
    /// I/O failure reading the archive itself (open, gzip/tar decode, or
    /// reading an entry's contents).
    #[error("I/O error at {path}: {source}", path = path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// The archive has no `harbor.lock` entry at all.
    #[error(
        "archive {path} does not contain a harbor.lock — this is not a valid Harbor package",
        path = path.display()
    )]
    MissingLockfile { path: PathBuf },

    /// The archive's `harbor.lock` entry exists but fails to parse.
    #[error("harbor.lock inside the archive is malformed: {0}")]
    LockfileParse(#[source] LockfileError),

    /// The archive's `harbor.lock` has no `package-checksum` recorded, so
    /// integrity cannot be verified (SPEC.md §9.4).
    #[error(
        "harbor.lock inside the archive has no package-checksum recorded; \
         cannot verify package integrity"
    )]
    MissingPackageChecksum,

    /// The recomputed package-checksum does not match the one recorded in
    /// the archive's own `harbor.lock` (SPEC.md §9.4). The archive is
    /// aborted before any extraction happens.
    #[error(
        "checksum mismatch: recomputed package-checksum ({actual}) does not match \
         the package-checksum recorded in harbor.lock ({expected}) — the archive \
         may be corrupt or tampered with; aborting before extraction"
    )]
    ChecksumMismatch { expected: String, actual: String },

    /// The archive has no `harbor.toml` entry at all.
    #[error(
        "archive {path} does not contain a harbor.toml — this is not a valid Harbor package",
        path = path.display()
    )]
    MissingManifest { path: PathBuf },

    /// `harbor.toml` (from the archive, or later from the extracted
    /// directory) fails to parse.
    #[error(transparent)]
    ManifestParse(#[from] ManifestError),

    /// `harbor.toml` parses but fails semantic validation (SPEC.md §9.6).
    #[error(transparent)]
    ManifestValidation(#[from] ValidationError),

    /// The manifest declares `os` (SPEC.md §6.2) and the current operating
    /// system is not in that list (SPEC.md §9.6.1).
    #[error(
        "this package only supports {declared:?}, but the current operating system is \
         {current:?}; cannot run here"
    )]
    UnsupportedOs { current: String, declared: Vec<String> },

    /// Safe-extraction failure (SPEC.md §9.5): an unsafe path in the
    /// archive, or an I/O failure while staging/renaming.
    #[error(transparent)]
    Extract(#[from] extract::ExtractError),
}

/// The outcome of successfully preparing a local `.harbor` archive: it has
/// been checksum-verified, extracted (or found already cached), and
/// re-validated. Launching it (SPEC.md §9.7 onward) is a later phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub name: String,
    pub version: String,
    pub package_dir: PathBuf,
    /// `true` if an existing extracted copy was reused (SPEC.md §9.5) rather
    /// than a fresh extraction happening on this call.
    pub from_cache: bool,
    /// Non-fatal manifest warnings (SPEC.md §17) from re-validation.
    pub warnings: Vec<ValidationWarning>,
}

/// Run the local-archive preparation pipeline (SPEC.md §9.4–§9.6.1) for
/// `archive`, caching the extracted result under `home`.
///
/// Steps, in order, each aborting immediately on failure:
/// 1. Read the archive's entries into memory once (§9.4).
/// 2. Recompute the package-checksum and compare against `harbor.lock`'s
///    recorded value; mismatch aborts before any extraction or writes.
/// 3. Parse `harbor.toml` (from the archive) to determine `(name, version)`
///    for cache addressing.
/// 4. Extract into `home.local_package_dir(name, version)` — skipped
///    entirely if that directory already exists and is non-empty (§9.5,
///    packages are immutable).
/// 5. Re-parse and re-validate `harbor.toml` from the extracted directory,
///    and check OS compatibility (§9.6, §9.6.1).
pub fn run_local_archive(archive: &Path, home: &HarborHome) -> Result<Prepared, RunError> {
    let entries = read_archive_entries(archive)?;

    // --- H-050: checksum verify (§9.4) — must happen before any extraction. ---
    let (_, lock_bytes) = entries
        .iter()
        .find(|(p, _)| p == "harbor.lock")
        .ok_or_else(|| RunError::MissingLockfile { path: archive.to_path_buf() })?;
    let lock_str = String::from_utf8_lossy(lock_bytes).into_owned();
    let lock = Lockfile::from_toml_str(&lock_str).map_err(RunError::LockfileParse)?;

    let expected_checksum = lock
        .package_checksum
        .clone()
        .ok_or(RunError::MissingPackageChecksum)?;
    let actual_checksum = compute_package_checksum_from_bytes(&entries);
    if actual_checksum != expected_checksum {
        return Err(RunError::ChecksumMismatch {
            expected: expected_checksum,
            actual: actual_checksum,
        });
    }

    // --- Parse harbor.toml (pre-extraction) purely for cache addressing. ---
    let (_, manifest_bytes) = entries
        .iter()
        .find(|(p, _)| p == "harbor.toml")
        .ok_or_else(|| RunError::MissingManifest { path: archive.to_path_buf() })?;
    let manifest_str = String::from_utf8_lossy(manifest_bytes).into_owned();
    let manifest_for_addressing = Manifest::from_toml_str(&manifest_str)?;

    // --- H-051: safe extraction, or skip if already cached (§9.5). ---
    let dest = home.local_package_dir(&manifest_for_addressing.name, &manifest_for_addressing.version);
    let from_cache = dir_is_nonempty(&dest).map_err(|e| RunError::Io {
        path: dest.clone(),
        source: e,
    })?;

    if !from_cache {
        extract::extract_entries(&entries, &dest, &home.tmp())?;
    }

    // --- H-052: re-validate + OS check (§9.6, §9.6.1), from the EXTRACTED
    // directory (not the in-memory bytes) — this is what a "cached" run
    // re-validates too, so a manually-tampered cache entry is still caught.
    let manifest_path = dest.join("harbor.toml");
    let manifest_str = fs::read_to_string(&manifest_path).map_err(|e| RunError::Io {
        path: manifest_path.clone(),
        source: e,
    })?;
    let manifest = Manifest::from_toml_str(&manifest_str)?;
    let warnings = manifest::validate_manifest(&manifest)?;

    let current = current_os();
    if !os_allowed(&manifest.os, current) {
        return Err(RunError::UnsupportedOs {
            current: current.to_string(),
            declared: manifest.os.clone(),
        });
    }

    Ok(Prepared {
        name: manifest.name,
        version: manifest.version,
        package_dir: dest,
        from_cache,
        warnings,
    })
}

/// Whether `current` (one of `"linux"`, `"macos"`, `"windows"`) is allowed to
/// run a package that declares `declared` in its `os` field (SPEC.md §9.6.1).
/// An empty `declared` list means no restriction — every OS is allowed.
///
/// Kept as a pure function (no `cfg!` inside it) so it can be unit-tested
/// independently of whatever OS the test suite actually runs on.
fn os_allowed(declared: &[String], current: &str) -> bool {
    declared.is_empty() || declared.iter().any(|o| o == current)
}

/// The current operating system, using the same identifiers as
/// [`crate::manifest::ALLOWED_OS`] (SPEC.md §6.2, §9.6.1).
fn current_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    }
}

/// Whether `dir` exists and contains at least one entry.
fn dir_is_nonempty(dir: &Path) -> io::Result<bool> {
    if !dir.is_dir() {
        return Ok(false);
    }
    let mut rd = fs::read_dir(dir)?;
    Ok(rd.next().is_some())
}

/// Read every regular-file entry out of a gzip-compressed tar (`.harbor`)
/// archive into memory as `(archive-relative path, contents)` pairs.
/// Directory entries (and any other non-regular entry type) are skipped.
///
/// Packages are small in V1 (SPEC.md), so reading the whole archive into
/// memory once up front — rather than streaming — keeps the checksum-verify
/// -then-extract pipeline simple and lets both steps share one read of the
/// archive.
fn read_archive_entries(archive: &Path) -> Result<Vec<(String, Vec<u8>)>, RunError> {
    let map_io = |source: io::Error| RunError::Io {
        path: archive.to_path_buf(),
        source,
    };

    let file = fs::File::open(archive).map_err(map_io)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar_archive = tar::Archive::new(decoder);

    let mut entries = Vec::new();
    for entry in tar_archive.entries().map_err(map_io)? {
        let mut entry = entry.map_err(map_io)?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let path = entry.path().map_err(map_io)?.to_string_lossy().into_owned();
        let mut contents = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut contents).map_err(map_io)?;
        entries.push((path, contents));
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn minimal_manifest_toml(name: &str, version: &str, os: &[&str]) -> String {
        let os_line = if os.is_empty() {
            String::new()
        } else {
            let list = os.iter().map(|o| format!("\"{o}\"")).collect::<Vec<_>>().join(", ");
            format!("os = [{list}]\n")
        };
        format!(
            r#"
spec-version = 1
name = "{name}"
version = "{version}"
description = "A test package."
package-type = "agent"
language = "python"
runtime = ">=3.11,<4"
entrypoint = "src/main.py"
license = "MIT"
permissions = []
features = []
{os_line}
[author]
name = "Jane Doe"
email = "jane@example.com"

[dependencies]
manifest = "pyproject.toml"
"#
        )
    }

    fn lockfile_with_checksum(version: &str, checksum: Option<String>) -> Lockfile {
        Lockfile {
            spec_version: 1,
            harbor_version: crate::HARBOR_VERSION.to_string(),
            package_version: version.to_string(),
            generated_at: "2026-07-16T10:00:00Z".to_string(),
            native_manifest: "pyproject.toml".to_string(),
            native_lockfile: None,
            native_lock_checksum: None,
            package_checksum: checksum,
        }
    }

    /// Build the full set of (archive-relative path, contents) entries for a
    /// minimal valid package, with a correctly computed package-checksum
    /// baked into `harbor.lock`.
    fn sample_entries(name: &str, version: &str, os: &[&str]) -> Vec<(String, Vec<u8>)> {
        let manifest_toml = minimal_manifest_toml(name, version, os);
        let mut files: Vec<(String, Vec<u8>)> = vec![
            ("harbor.toml".to_string(), manifest_toml.into_bytes()),
            ("README.md".to_string(), b"# test\n".to_vec()),
            ("LICENSE".to_string(), b"MIT\n".to_vec()),
            ("src/main.py".to_string(), b"print('hi')\n".to_vec()),
        ];
        let checksum = compute_package_checksum_from_bytes(&files);
        let lock = lockfile_with_checksum(version, Some(checksum));
        files.push(("harbor.lock".to_string(), lock.to_toml_string().unwrap().into_bytes()));
        files
    }

    /// Set a tar header's path field directly (bypassing `Header::set_path`'s
    /// `..`/absolute-path validation), so tests can build archives containing
    /// the kind of hostile entries a real attacker's archive might carry —
    /// `tar::Builder::append_data` refuses to construct those on purpose, but
    /// nothing stops a hand-crafted `.harbor` file from having one.
    fn set_raw_path(header: &mut tar::Header, path: &str) {
        let gnu = header.as_gnu_mut().expect("built with Header::new_gnu()");
        for b in gnu.name.iter_mut() {
            *b = 0;
        }
        let bytes = path.as_bytes();
        let n = bytes.len().min(gnu.name.len());
        gnu.name[..n].copy_from_slice(&bytes[..n]);
    }

    /// Write `entries` as a gzip-compressed tar archive at `path` (a
    /// from-scratch tar/flate2 build, independent of `package::build_archive`,
    /// so these tests don't depend on that module's behavior).
    fn write_archive(path: &Path, entries: &[(String, Vec<u8>)]) {
        let f = fs::File::create(path).unwrap();
        let encoder = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (p, contents) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            set_raw_path(&mut header, p);
            header.set_cksum();
            // `append` (not `append_data`) uses the header verbatim, since
            // its path was already set above without going through the
            // validating setter.
            builder.append(&header, contents.as_slice()).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
    }

    // ---- H-050: checksum verify ----

    #[test]
    fn happy_path_prepares_and_extracts_the_package() {
        let dir = tempdir().unwrap();
        let entries = sample_entries("sample-agent", "1.0.0", &[]);
        let archive_path = dir.path().join("sample-agent-1.0.0.harbor");
        write_archive(&archive_path, &entries);

        let home = HarborHome::at(dir.path().join("home"));
        home.ensure_layout().unwrap();

        let prepared = run_local_archive(&archive_path, &home).expect("should succeed");
        assert_eq!(prepared.name, "sample-agent");
        assert_eq!(prepared.version, "1.0.0");
        assert!(!prepared.from_cache);
        assert!(prepared.warnings.is_empty());
        assert!(prepared.package_dir.join("harbor.toml").is_file());
        assert!(prepared.package_dir.join("src/main.py").is_file());
        assert!(prepared.package_dir.join("harbor.lock").is_file());
    }

    #[test]
    fn checksum_mismatch_aborts_before_extraction() {
        let dir = tempdir().unwrap();
        let mut entries = sample_entries("bad-agent", "1.0.0", &[]);
        // Mutate a non-harbor.lock file's contents after the checksum was
        // computed, so the recorded package-checksum is now stale.
        for (path, contents) in entries.iter_mut() {
            if path == "src/main.py" {
                contents.push(b'!');
            }
        }
        let archive_path = dir.path().join("bad-agent-1.0.0.harbor");
        write_archive(&archive_path, &entries);

        let home = HarborHome::at(dir.path().join("home"));
        home.ensure_layout().unwrap();

        let err = run_local_archive(&archive_path, &home).unwrap_err();
        assert!(matches!(err, RunError::ChecksumMismatch { .. }), "got: {err:?}");

        let dest = home.local_package_dir("bad-agent", "1.0.0");
        assert!(!dest.exists(), "destination must not be created on checksum mismatch");
    }

    #[test]
    fn missing_lockfile_is_a_clear_error() {
        let dir = tempdir().unwrap();
        let mut entries = sample_entries("no-lock-agent", "1.0.0", &[]);
        entries.retain(|(p, _)| p != "harbor.lock");
        let archive_path = dir.path().join("no-lock-agent-1.0.0.harbor");
        write_archive(&archive_path, &entries);

        let home = HarborHome::at(dir.path().join("home"));
        home.ensure_layout().unwrap();

        let err = run_local_archive(&archive_path, &home).unwrap_err();
        assert!(matches!(err, RunError::MissingLockfile { .. }), "got: {err:?}");
    }

    #[test]
    fn missing_package_checksum_is_a_clear_error() {
        let dir = tempdir().unwrap();
        let mut entries = sample_entries("no-checksum-agent", "1.0.0", &[]);
        let lock = lockfile_with_checksum("1.0.0", None);
        for (path, contents) in entries.iter_mut() {
            if path == "harbor.lock" {
                *contents = lock.to_toml_string().unwrap().into_bytes();
            }
        }
        let archive_path = dir.path().join("no-checksum-agent-1.0.0.harbor");
        write_archive(&archive_path, &entries);

        let home = HarborHome::at(dir.path().join("home"));
        home.ensure_layout().unwrap();

        let err = run_local_archive(&archive_path, &home).unwrap_err();
        assert!(matches!(err, RunError::MissingPackageChecksum), "got: {err:?}");
    }

    #[test]
    fn malformed_lockfile_is_a_clear_error() {
        let dir = tempdir().unwrap();
        let mut entries = sample_entries("bad-lock-agent", "1.0.0", &[]);
        for (path, contents) in entries.iter_mut() {
            if path == "harbor.lock" {
                *contents = b"not valid toml {{{".to_vec();
            }
        }
        let archive_path = dir.path().join("bad-lock-agent-1.0.0.harbor");
        write_archive(&archive_path, &entries);

        let home = HarborHome::at(dir.path().join("home"));
        home.ensure_layout().unwrap();

        let err = run_local_archive(&archive_path, &home).unwrap_err();
        assert!(matches!(err, RunError::LockfileParse(_)), "got: {err:?}");
    }

    // ---- H-051: safe extraction ----

    #[test]
    fn path_traversal_entries_are_rejected_via_the_pure_helper() {
        assert!(!extract::is_path_safe("../x"));
        assert!(!extract::is_path_safe("/abs/x"));
        assert!(!extract::is_path_safe("a/../../b"));
        assert!(extract::is_path_safe("a/b/c"));
    }

    #[test]
    fn archive_with_traversal_entry_is_rejected_and_nothing_is_extracted() {
        let dir = tempdir().unwrap();
        let mut entries = sample_entries("evil-agent", "1.0.0", &[]);
        entries.push(("../evil.txt".to_string(), b"pwned".to_vec()));

        // Recompute+rewrite the checksum so the (unrelated) checksum-verify
        // step passes and we're specifically exercising extraction safety.
        let checksum = compute_package_checksum_from_bytes(&entries);
        let lock = lockfile_with_checksum("1.0.0", Some(checksum));
        for (path, contents) in entries.iter_mut() {
            if path == "harbor.lock" {
                *contents = lock.to_toml_string().unwrap().into_bytes();
            }
        }

        let archive_path = dir.path().join("evil-agent-1.0.0.harbor");
        write_archive(&archive_path, &entries);

        let home = HarborHome::at(dir.path().join("home"));
        home.ensure_layout().unwrap();

        let err = run_local_archive(&archive_path, &home).unwrap_err();
        assert!(matches!(err, RunError::Extract(_)), "got: {err:?}");

        let dest = home.local_package_dir("evil-agent", "1.0.0");
        assert!(!dest.exists());
        // And nothing escaped above the fake home either.
        assert!(!dir.path().join("evil.txt").exists());
    }

    // ---- skip-if-cached (§9.5 immutability) ----

    #[test]
    fn skip_extraction_when_destination_already_cached() {
        let dir = tempdir().unwrap();
        let entries = sample_entries("cached-agent", "1.0.0", &[]);
        let archive_path = dir.path().join("cached-agent-1.0.0.harbor");
        write_archive(&archive_path, &entries);

        let home = HarborHome::at(dir.path().join("home"));
        home.ensure_layout().unwrap();

        // Pre-populate the destination as if a previous run had already
        // extracted it, plus a sentinel file that a fresh extraction of
        // this archive would never produce.
        let dest = home.local_package_dir("cached-agent", "1.0.0");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("harbor.toml"), minimal_manifest_toml("cached-agent", "1.0.0", &[]))
            .unwrap();
        fs::write(dest.join("sentinel.txt"), b"already here").unwrap();

        let prepared = run_local_archive(&archive_path, &home).expect("should succeed");
        assert!(prepared.from_cache);
        assert!(dest.join("sentinel.txt").is_file(), "cached destination must be left untouched");
        assert!(
            !dest.join("README.md").exists(),
            "a skipped extraction must not add the archive's other files"
        );
    }

    // ---- H-052: OS compatibility check ----

    #[test]
    fn os_allowed_pure_function_behavior() {
        assert!(os_allowed(&[], "macos"), "no os declared means unrestricted");
        assert!(os_allowed(&["macos".to_string()], "macos"));
        assert!(os_allowed(&["linux".to_string(), "macos".to_string()], "macos"));
        assert!(!os_allowed(&["linux".to_string()], "macos"));
        assert!(!os_allowed(&["windows".to_string()], "linux"));
    }

    #[test]
    fn run_fails_when_current_os_is_not_in_the_declared_list() {
        let current = current_os();
        let other = if current == "macos" { "linux" } else { "macos" };

        let dir = tempdir().unwrap();
        let entries = sample_entries("os-agent", "1.0.0", &[other]);
        let archive_path = dir.path().join("os-agent-1.0.0.harbor");
        write_archive(&archive_path, &entries);

        let home = HarborHome::at(dir.path().join("home"));
        home.ensure_layout().unwrap();

        let err = run_local_archive(&archive_path, &home).unwrap_err();
        match err {
            RunError::UnsupportedOs { current: c, declared } => {
                assert_eq!(c, current);
                assert_eq!(declared, vec![other.to_string()]);
            }
            other => panic!("expected UnsupportedOs, got: {other:?}"),
        }

        // Extraction happens before the OS check (which re-reads from the
        // extracted dir), so the directory does exist — but this is still a
        // hard failure and `Prepared` must never be returned.
    }

    #[test]
    fn run_succeeds_when_current_os_is_in_the_declared_list() {
        let current = current_os();

        let dir = tempdir().unwrap();
        let entries = sample_entries("os-ok-agent", "1.0.0", &[current]);
        let archive_path = dir.path().join("os-ok-agent-1.0.0.harbor");
        write_archive(&archive_path, &entries);

        let home = HarborHome::at(dir.path().join("home"));
        home.ensure_layout().unwrap();

        let prepared = run_local_archive(&archive_path, &home).expect("should succeed");
        assert_eq!(prepared.name, "os-ok-agent");
    }
}
