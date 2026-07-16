//! File-set collection and deterministic `.harbor` (tar.gz) archive building
//! (SPEC.md §5, §8.1 steps 6/8).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use flate2::write::GzEncoder;
use flate2::Compression;
use ignore::WalkBuilder;
use thiserror::Error;

/// Errors that can occur while collecting a package's file set or building
/// its archive.
#[derive(Debug, Error)]
pub enum PackageError {
    /// A failure while walking the directory tree (e.g. a malformed
    /// `.gitignore`, or a filesystem error encountered mid-walk).
    #[error("failed to walk {root}: {source}", root = root.display())]
    Walk {
        root: PathBuf,
        #[source]
        source: ignore::Error,
    },

    /// I/O failure while reading a source file or writing the archive.
    #[error("I/O error at {path}: {source}", path = path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Convert a filesystem-relative path into an archive-relative path: forward
/// slashes, no leading `./`.
fn to_archive_path(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether a regular file's permissions have any executable bit set. Always
/// `false` on non-Unix platforms (Harbor archives built there simply never
/// mark anything executable; this affects only the tar mode bits, not
/// correctness of the file contents).
#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    false
}

/// Walk `root`, honoring `.gitignore` semantics (including nested
/// `.gitignore` files and negation patterns), and return the set of regular
/// files to include in a `.harbor` archive as
/// `(archive-relative path, on-disk path)` pairs, sorted by archive-relative
/// path (byte order).
///
/// Dotfiles (other than `.git/`) are included unless a `.gitignore` pattern
/// excludes them — `.gitignore` files themselves are ordinary included files.
/// `.git/` metadata is always excluded regardless of `.gitignore` contents
/// (SPEC.md §5.4), and files previously produced by `build_archive`
/// (`*.harbor`) are always excluded so a rebuild never ingests a prior
/// archive. There is no mechanism to force-include an excluded file (V1).
pub fn collect_files(root: &Path) -> Result<Vec<(String, PathBuf)>, PackageError> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false);

    let mut files = Vec::new();
    for entry in builder.build() {
        let entry = entry.map_err(|e| PackageError::Walk {
            root: root.to_path_buf(),
            source: e,
        })?;
        let path = entry.path();
        if path == root {
            continue;
        }
        let is_file = entry.file_type().map(|ft| ft.is_file()).unwrap_or(false);
        if !is_file {
            continue;
        }

        let rel = path.strip_prefix(root).expect("walk entry must be under root");
        let archive_path = to_archive_path(rel);

        // Always exclude .git/ metadata, regardless of .gitignore contents.
        if archive_path.split('/').any(|component| component == ".git") {
            continue;
        }
        // Never ingest a previously built archive — or the staging file a
        // crashed build may have left behind — into a new one.
        if archive_path.ends_with(".harbor") || archive_path.ends_with(".harbor.tmp") {
            continue;
        }

        files.push((archive_path, path.to_path_buf()));
    }

    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    Ok(files)
}

/// Build a deterministic `.harbor` archive (gzip-compressed tar, SPEC.md
/// §5.1) at `out_path` from `files` (as returned by [`collect_files`]).
///
/// Entries are appended in sorted archive-path order (byte order) with
/// `mtime = 0`, `uid = gid = 0`, empty `uname`/`gname`, and mode `0o755` for
/// files with any executable bit set on disk, `0o644` otherwise. The gzip
/// wrapper's own `mtime` field is also fixed at `0` so that identical inputs
/// always produce byte-identical archives regardless of when they're built.
///
/// `root` is accepted for interface symmetry with [`collect_files`] and is
/// not otherwise used: `files`' on-disk paths are already fully resolved.
pub fn build_archive(
    _root: &Path,
    files: &[(String, PathBuf)],
    out_path: &Path,
) -> Result<(), PackageError> {
    let mut sorted: Vec<&(String, PathBuf)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

    let out_file = fs::File::create(out_path).map_err(|e| PackageError::Io {
        path: out_path.to_path_buf(),
        source: e,
    })?;
    let encoder = GzEncoder::new(out_file, Compression::default());
    let mut tar_builder = tar::Builder::new(encoder);

    for (archive_path, disk_path) in sorted {
        let mut f = fs::File::open(disk_path).map_err(|e| PackageError::Io {
            path: disk_path.clone(),
            source: e,
        })?;
        let metadata = f.metadata().map_err(|e| PackageError::Io {
            path: disk_path.clone(),
            source: e,
        })?;

        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(metadata.len());
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mode(if is_executable(&metadata) { 0o755 } else { 0o644 });
        let _ = header.set_username("");
        let _ = header.set_groupname("");
        header.set_cksum();

        tar_builder
            .append_data(&mut header, archive_path, &mut f)
            .map_err(|e| PackageError::Io {
                path: disk_path.clone(),
                source: e,
            })?;
    }

    let encoder = tar_builder.into_inner().map_err(|e| PackageError::Io {
        path: out_path.to_path_buf(),
        source: e,
    })?;
    encoder.finish().map_err(|e| PackageError::Io {
        path: out_path.to_path_buf(),
        source: e,
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_file(dir: &Path, rel: &str, contents: &[u8]) -> PathBuf {
        let full = dir.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full, contents).unwrap();
        full
    }

    fn archive_paths(files: &[(String, PathBuf)]) -> Vec<String> {
        files.iter().map(|(p, _)| p.clone()).collect()
    }

    // ---- collect_files: .gitignore exclusion ----

    #[test]
    fn gitignored_file_never_appears() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), ".gitignore", b"secret.txt\n");
        write_file(dir.path(), "secret.txt", b"shh");
        write_file(dir.path(), "keep.txt", b"kept");

        let files = collect_files(dir.path()).unwrap();
        let paths = archive_paths(&files);

        assert!(!paths.contains(&"secret.txt".to_string()), "got: {paths:?}");
        assert!(paths.contains(&"keep.txt".to_string()));
        assert!(paths.contains(&".gitignore".to_string()), ".gitignore itself must be included");
    }

    #[test]
    fn nested_gitignore_with_negation_is_respected() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), ".gitignore", b"sub/*.log\n");
        // Note: a pattern of bare "*" would also match the .gitignore file
        // itself (real git semantics — there's no implicit self-exception),
        // so this uses a narrower pattern that leaves sub/.gitignore itself
        // included while still exercising negation.
        write_file(dir.path(), "sub/.gitignore", b"*.txt\n!keep.txt\n");
        write_file(dir.path(), "sub/keep.txt", b"kept");
        write_file(dir.path(), "sub/drop.txt", b"dropped");
        write_file(dir.path(), "sub/noisy.log", b"dropped too");

        let files = collect_files(dir.path()).unwrap();
        let paths = archive_paths(&files);

        assert!(paths.contains(&"sub/keep.txt".to_string()), "got: {paths:?}");
        assert!(!paths.contains(&"sub/drop.txt".to_string()), "got: {paths:?}");
        assert!(!paths.contains(&"sub/noisy.log".to_string()), "got: {paths:?}");
        assert!(paths.contains(&"sub/.gitignore".to_string()));
    }

    #[test]
    fn git_directory_is_always_excluded() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), ".git/HEAD", b"ref: refs/heads/main");
        write_file(dir.path(), ".git/objects/deadbeef", b"blob");
        write_file(dir.path(), "README.md", b"# hi");

        let files = collect_files(dir.path()).unwrap();
        let paths = archive_paths(&files);

        assert!(paths.iter().all(|p| !p.starts_with(".git/")), "got: {paths:?}");
        assert!(paths.contains(&"README.md".to_string()));
    }

    #[test]
    fn previous_harbor_archive_is_excluded_from_collection() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "README.md", b"# hi");
        write_file(dir.path(), "stale-1.0.0.harbor", b"not a real archive");

        let files = collect_files(dir.path()).unwrap();
        let paths = archive_paths(&files);

        assert!(!paths.iter().any(|p| p.ends_with(".harbor")), "got: {paths:?}");
    }

    #[test]
    fn files_are_sorted_by_path_bytes() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "zeta.txt", b"z");
        write_file(dir.path(), "alpha.txt", b"a");
        write_file(dir.path(), "mid/beta.txt", b"b");

        let files = collect_files(dir.path()).unwrap();
        let paths = archive_paths(&files);
        let mut expected = paths.clone();
        expected.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        assert_eq!(paths, expected);
    }

    // ---- build_archive: determinism ----

    #[test]
    fn identical_inputs_produce_byte_identical_archives() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "README.md", b"# hi");
        write_file(dir.path(), "src/main.py", b"print('hi')\n");

        let files = collect_files(dir.path()).unwrap();

        let out1 = dir.path().join("a.harbor");
        let out2 = dir.path().join("b.harbor");
        build_archive(dir.path(), &files, &out1).unwrap();
        // Ensure any wall-clock-based nondeterminism would have a chance to
        // manifest by building the second archive distinctly afterward.
        build_archive(dir.path(), &files, &out2).unwrap();

        let bytes1 = fs::read(&out1).unwrap();
        let bytes2 = fs::read(&out2).unwrap();
        assert_eq!(bytes1, bytes2, "archives built from identical inputs must be byte-identical");
    }

    #[test]
    fn built_archive_is_inspectable_by_tar_and_contains_expected_entries() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "README.md", b"# hi");
        write_file(dir.path(), "src/main.py", b"print('hi')\n");

        let files = collect_files(dir.path()).unwrap();
        let out = dir.path().join("pkg.harbor");
        build_archive(dir.path(), &files, &out).unwrap();

        // Re-open with the `tar` crate itself (equivalent to `tar -tzf`).
        let f = fs::File::open(&out).unwrap();
        let decoder = flate2::read::GzDecoder::new(f);
        let mut archive = tar::Archive::new(decoder);
        let mut names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();

        assert_eq!(names, vec!["README.md".to_string(), "src/main.py".to_string()]);
    }
}
