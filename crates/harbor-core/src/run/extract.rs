//! Safe extraction of an in-memory tar entry list into a destination
//! directory (SPEC.md §9.5).
//!
//! Extraction always stages into a scratch directory first and renames it
//! into place only once every entry has been written successfully, so an
//! interrupted extraction (crash, killed process) never leaves a half
//! -populated version directory for a later `harbor run` to mistake for a
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

/// Extract `entries` (archive-relative path, file contents) into `dest`.
///
/// Every entry's path is checked with [`is_path_safe`] before anything is
/// written. Entries are staged under a fresh scratch directory inside
/// `staging_root` (expected to be `home.tmp()`), and that scratch directory
/// is renamed into place at `dest` only once every file has been written —
/// so a crash mid-extraction can never leave `dest` half-populated.
///
/// `entries` are assumed to be regular files only (no directory entries);
/// parent directories are created as needed from each file's path.
pub fn extract_entries(
    entries: &[(String, Vec<u8>)],
    dest: &Path,
    staging_root: &Path,
) -> Result<(), ExtractError> {
    // Reject any unsafe path before writing a single byte.
    for (path, _) in entries {
        if !is_path_safe(path) {
            return Err(ExtractError::UnsafePath { path: path.clone() });
        }
    }

    fs::create_dir_all(staging_root).map_err(|e| ExtractError::Io {
        path: staging_root.to_path_buf(),
        source: e,
    })?;

    // A unique-per-call scratch directory so concurrent `harbor run`
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

    for (path, contents) in entries {
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
    }

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| ExtractError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }

    fs::rename(&staging_dir, dest).map_err(|e| ExtractError::Io {
        path: dest.to_path_buf(),
        source: e,
    })?;

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
        assert!(is_path_safe("harbor.toml"));
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
            ("ok.txt".to_string(), b"fine".to_vec()),
            ("../escape.txt".to_string(), b"pwned".to_vec()),
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

        let entries = vec![("/abs/evil.txt".to_string(), b"pwned".to_vec())];

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
            ("harbor.toml".to_string(), b"name = \"pkg\"".to_vec()),
            ("src/main.py".to_string(), b"print('hi')".to_vec()),
        ];

        extract_entries(&entries, &dest, &staging_root).expect("should succeed");

        assert!(dest.join("harbor.toml").is_file());
        assert!(dest.join("src/main.py").is_file());
        assert_eq!(fs::read(dest.join("harbor.toml")).unwrap(), b"name = \"pkg\"");

        // The staging directory itself should be gone (renamed away), not
        // left behind as extra clutter under tmp/.
        let leftover: Vec<_> = fs::read_dir(&staging_root)
            .map(|rd| rd.filter_map(|e| e.ok()).collect())
            .unwrap_or_default();
        assert!(leftover.is_empty(), "staging dir should be empty after rename, got: {leftover:?}");
    }
}
