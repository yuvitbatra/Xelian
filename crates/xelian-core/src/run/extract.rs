//! Safe extraction of an in-memory tar entry list into a destination
//! directory (SPEC.md §9.5).
//!
//! Extraction always stages into a scratch directory first and renames it
//! into place only once every entry has been written successfully, so an
//! interrupted extraction (crash, killed process) never leaves a half
//! -populated version directory for a later `xelian run` to mistake for a
//! complete, cached copy.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

/// Errors that can occur while safely extracting a package's file entries.
#[derive(Debug, Error)]
pub enum ExtractError {
    /// An archive entry's path is absolute, contains a `..` component, or is
    /// otherwise not a plain relative path — rejected before anything is
    /// written to disk.
    #[error(
        "archive entry {path:?} has an unsafe path (absolute, or containing '..') \
         and was rejected"
    )]
    UnsafePath { path: String },

    /// Defense-in-depth: after joining an entry's path onto the staging
    /// directory and creating its parent directories, the resolved
    /// (canonical) location is not actually under the staging directory.
    #[error("archive entry {path:?} would extract outside the destination directory")]
    PathEscapesDestination { path: String },

    /// I/O failure while staging, writing, or renaming into place.
    #[error("I/O error at {path}: {source}", path = path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Whether an archive-relative entry path is safe to extract: non-empty, not
/// absolute, and containing no `..` (or root/prefix) components — only plain
/// `Normal` path components are allowed.
pub fn is_path_safe(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let p = Path::new(path);
    if p.is_absolute() {
        return false;
    }
    p.components().all(|c| matches!(c, Component::Normal(_)))
}

/// Extract `entries` (archive-relative path, file contents, tar mode bits)
/// into `dest`.
///
/// Every entry's path is checked with [`is_path_safe`] before anything is
/// written. Entries are staged under a fresh scratch directory inside
/// `staging_root` (expected to be `home.tmp()`), and that scratch directory
/// is renamed into place at `dest` only once every file has been written —
/// so a crash mid-extraction can never leave `dest` half-populated.
///
/// `entries` are assumed to be regular files only (no directory entries);
/// parent directories are created as needed from each file's path.
///
/// Returns `Ok(true)` if a concurrent `xelian run` of the same archive won
/// the race to populate `dest` first (see the module-level rename-race
/// note below) — the caller should treat that the same as a cache hit.
/// Returns `Ok(false)` for a normal, uncontested extraction.
pub fn extract_entries(
    entries: &[(String, Vec<u8>, u32)],
    dest: &Path,
    staging_root: &Path,
) -> Result<bool, ExtractError> {
    // Reject any unsafe path before writing a single byte.
    for (path, _, _) in entries {
        if !is_path_safe(path) {
            return Err(ExtractError::UnsafePath { path: path.clone() });
        }
    }

    fs::create_dir_all(staging_root).map_err(|e| ExtractError::Io {
        path: staging_root.to_path_buf(),
        source: e,
    })?;

    // A unique-per-call scratch directory so concurrent `xelian run`
    // invocations (or a leftover from a previous crashed run) never collide.
    let staging_dir = staging_root.join(format!("extract-{}", unique_suffix()));
    fs::create_dir_all(&staging_dir).map_err(|e| ExtractError::Io {
        path: staging_dir.clone(),
        source: e,
    })?;

    let canonical_staging = fs::canonicalize(&staging_dir).map_err(|e| ExtractError::Io {
        path: staging_dir.clone(),
        source: e,
    })?;

    for (path, contents, mode) in entries {
        let target = staging_dir.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| ExtractError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;

            // Canonical prefix check: now that the parent exists, resolve it
            // and confirm it's still under the staging root. `is_path_safe`
            // already rules out `..` components, so this is defense in depth
            // (e.g. against future path-construction bugs), not the primary
            // guard.
            let canonical_parent = fs::canonicalize(parent).map_err(|e| ExtractError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
            if !canonical_parent.starts_with(&canonical_staging) {
                let _ = fs::remove_dir_all(&staging_dir);
                return Err(ExtractError::PathEscapesDestination { path: path.clone() });
            }
        }

        fs::write(&target, contents).map_err(|e| ExtractError::Io {
            path: target.clone(),
            source: e,
        })?;

        set_executable_if_needed(&target, *mode).map_err(|e| ExtractError::Io {
            path: target.clone(),
            source: e,
        })?;
    }

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| ExtractError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }

    if let Err(e) = fs::rename(&staging_dir, dest) {
        // Rename-race, TOCTOU (§9.5): `run::run_local_archive` decides
        // `from_cache` by checking whether `dest` is non-empty *before*
        // extraction starts. If two `xelian run` invocations race on the
        // very first extraction of the same archive, both see an empty/
        // missing `dest`, both stage independently, and only one `rename`
        // can win — `rename` onto an existing non-empty directory fails on
        // Unix (`ENOTEMPTY`/`EEXIST`). Rather than surface that as a raw I/O
        // error, treat a post-failure non-empty `dest` as a concurrent-win
        // cache hit: the package is correctly cached (by the winner), so
        // this invocation just cleans up its own now-redundant staging copy.
        if dir_is_nonempty(dest) {
            let _ = fs::remove_dir_all(&staging_dir);
            return Ok(true);
        }
        return Err(ExtractError::Io {
            path: dest.to_path_buf(),
            source: e,
        });
    }

    Ok(false)
}

/// Whether `dir` exists and contains at least one entry. Used only to
/// detect the rename-race concurrent-win case above.
fn dir_is_nonempty(dir: &Path) -> bool {
    match fs::read_dir(dir) {
        Ok(mut rd) => rd.next().is_some(),
        Err(_) => false,
    }
}

/// On Unix, set the extracted file's mode to `0o755` if the archive's
/// recorded tar mode has any execute bit set (owner, group, or other) —
/// preserving the "executable entrypoint" intent from `package::build_archive`
/// (which sets exactly `0o755` or `0o644`, never anything in between).
/// Files with no execute bit are left at the process's default create mode.
/// A no-op on non-Unix platforms, where the concept doesn't apply.
#[cfg(unix)]
fn set_executable_if_needed(target: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if mode & 0o111 != 0 {
        fs::set_permissions(target, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_executable_if_needed(_target: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

/// A cheap, process-local unique token for scratch directory names. Not
/// cryptographically anything — just enough to avoid collisions between
/// concurrent extractions.
fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos}-{count}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn safe_relative_paths_are_accepted() {
        assert!(is_path_safe("xelian.toml"));
        assert!(is_path_safe("src/main.py"));
        assert!(is_path_safe("a/b/c/d.txt"));
    }

    #[test]
    fn parent_dir_traversal_is_rejected() {
        assert!(!is_path_safe("../x"));
        assert!(!is_path_safe("a/../../b"));
        assert!(!is_path_safe("../../../etc/passwd"));
    }

    #[test]
    fn absolute_paths_are_rejected() {
        assert!(!is_path_safe("/abs/x"));
        assert!(!is_path_safe("/etc/passwd"));
    }

    #[test]
    fn empty_path_is_rejected() {
        assert!(!is_path_safe(""));
    }

    #[test]
    fn extract_entries_rejects_traversal_before_writing_anything() {
        let tmp = tempdir().unwrap();
        let staging_root = tmp.path().join("tmp");
        let dest = tmp.path().join("dest");

        let entries = vec![
            ("ok.txt".to_string(), b"fine".to_vec(), 0o644),
            ("../escape.txt".to_string(), b"pwned".to_vec(), 0o644),
        ];

        let err = extract_entries(&entries, &dest, &staging_root).unwrap_err();
        assert!(matches!(err, ExtractError::UnsafePath { .. }));
        assert!(!dest.exists(), "destination must not be created on rejection");
        // Nothing should have escaped above the tempdir either.
        assert!(!tmp.path().join("escape.txt").exists());
    }

    #[test]
    fn extract_entries_rejects_absolute_path_before_writing_anything() {
        let tmp = tempdir().unwrap();
        let staging_root = tmp.path().join("tmp");
        let dest = tmp.path().join("dest");

        let entries = vec![("/abs/evil.txt".to_string(), b"pwned".to_vec(), 0o644)];

        let err = extract_entries(&entries, &dest, &staging_root).unwrap_err();
        assert!(matches!(err, ExtractError::UnsafePath { .. }));
        assert!(!dest.exists());
    }

    #[test]
    fn extract_entries_happy_path_writes_files_and_renames_into_dest() {
        let tmp = tempdir().unwrap();
        let staging_root = tmp.path().join("tmp");
        let dest = tmp.path().join("packages").join("local").join("pkg").join("1.0.0");

        let entries = vec![
            ("xelian.toml".to_string(), b"name = \"pkg\"".to_vec(), 0o644),
            ("src/main.py".to_string(), b"print('hi')".to_vec(), 0o644),
        ];

        let concurrent_win =
            extract_entries(&entries, &dest, &staging_root).expect("should succeed");
        assert!(!concurrent_win, "uncontested extraction is not a concurrent-win");

        assert!(dest.join("xelian.toml").is_file());
        assert!(dest.join("src/main.py").is_file());
        assert_eq!(fs::read(dest.join("xelian.toml")).unwrap(), b"name = \"pkg\"");

        // The staging directory itself should be gone (renamed away), not
        // left behind as extra clutter under tmp/.
        let leftover: Vec<_> = fs::read_dir(&staging_root)
            .map(|rd| rd.filter_map(|e| e.ok()).collect())
            .unwrap_or_default();
        assert!(leftover.is_empty(), "staging dir should be empty after rename, got: {leftover:?}");
    }

    #[cfg(unix)]
    #[test]
    fn executable_mode_bit_is_restored_on_extraction() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempdir().unwrap();
        let staging_root = tmp.path().join("tmp");
        let dest = tmp.path().join("dest");

        let entries = vec![
            ("run.sh".to_string(), b"#!/bin/sh\necho hi\n".to_vec(), 0o755),
            ("README.md".to_string(), b"# hi\n".to_vec(), 0o644),
        ];

        extract_entries(&entries, &dest, &staging_root).expect("should succeed");

        let script_mode = fs::metadata(dest.join("run.sh")).unwrap().permissions().mode();
        assert_ne!(script_mode & 0o111, 0, "execute bit should be set, got mode {script_mode:o}");

        // A non-executable entry must not be granted the bit just because
        // some other entry in the same archive had it.
        let readme_mode = fs::metadata(dest.join("README.md")).unwrap().permissions().mode();
        assert_eq!(
            readme_mode & 0o111,
            0,
            "non-executable entry must not gain the execute bit, got mode {readme_mode:o}"
        );
    }

    #[test]
    fn concurrent_rename_race_is_treated_as_a_cache_hit() {
        let tmp = tempdir().unwrap();
        let staging_root = tmp.path().join("tmp");
        let dest = tmp.path().join("dest");

        // Simulate another process having already won the race: `dest`
        // exists and is non-empty by the time our `rename` runs.
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("winner.txt"), b"already extracted by someone else").unwrap();

        let entries = vec![("xelian.toml".to_string(), b"name = \"pkg\"".to_vec(), 0o644)];

        let concurrent_win = extract_entries(&entries, &dest, &staging_root)
            .expect("a rename race onto a non-empty dest must not be a hard error");
        assert!(concurrent_win, "should be reported as a concurrent-win cache hit");

        // The winner's content must be left completely untouched.
        assert!(dest.join("winner.txt").is_file());
        assert!(!dest.join("xelian.toml").exists());

        // Our own staging directory must be cleaned up, not left behind.
        let leftover: Vec<_> = fs::read_dir(&staging_root)
            .map(|rd| rd.filter_map(|e| e.ok()).collect())
            .unwrap_or_default();
        assert!(leftover.is_empty(), "staging dir should be cleaned up, got: {leftover:?}");
    }
}
