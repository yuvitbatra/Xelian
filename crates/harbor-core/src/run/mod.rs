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

/// The three target forms that `harbor run` accepts (SPEC.md §9.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunTarget {
    /// A local `.harbor` archive path.
    LocalArchive(PathBuf),
    /// A registry reference `owner/name`.
    RegistryRef { owner: String, name: String },
    /// A GitHub URL (https://github.com/owner/repo).
    GitHubUrl(String),
}

/// Parse a `harbor run` target string into one of the three accepted forms
/// (SPEC.md §9.2). Rejects any input that doesn't match any form.
pub fn parse_run_target(target: &str) -> Result<RunTarget, RunError> {
    // GitHub URL: must start with https://github.com/ or http://github.com/
    if target.starts_with("https://github.com/") || target.starts_with("http://github.com/") {
        return Ok(RunTarget::GitHubUrl(target.to_string()));
    }

    // Local archive: ends with .harbor or is an existing file
    let path = Path::new(target);
    if target.ends_with(".harbor") || path.is_file() {
        return Ok(RunTarget::LocalArchive(path.to_path_buf()));
    }

    // Registry reference: must contain exactly one `/` to separate owner/name,
    // and must not look like a URL or a file path.
    if let Some(slash_pos) = target.find('/') {
        let owner = &target[..slash_pos];
        let name = &target[slash_pos + 1..];
        // Both components must be safe path segments. `@` is rejected on the
        // name so `owner/pkg@1.0.0` fails loudly rather than being resolved as
        // a package literally named `pkg@1.0.0` — there is no `@version` pin
        // syntax in V1 (SPEC.md §9.2, §22). `.`/`..` and separators are
        // rejected so a ref can never fold up a cache directory level.
        if is_safe_ref_component(owner) && is_safe_ref_component(name) {
            return Ok(RunTarget::RegistryRef {
                owner: owner.to_string(),
                name: name.to_string(),
            });
        }
    }

    Err(RunError::InvalidTarget {
        target: target.to_string(),
    })
}

/// `true` if `s` is a safe single path segment for a registry `owner`/`name`:
/// non-empty, not `.`/`..`, and free of path separators, `@` pins, and `.`.
fn is_safe_ref_component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('.')
        && !s.contains('@')
}

pub mod env_vars;
pub mod extract;
pub mod launch;
pub mod model;
pub mod runtime;

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

    /// Belt-and-braces safety check, independent of manifest validation:
    /// the extraction destination computed from the (already-validated)
    /// manifest `name`/`version` somehow does not resolve under
    /// `home.packages()`. Should be unreachable in practice — the
    /// `validate_manifest` call earlier in the pipeline already rejects any
    /// `name`/`version` shape that could produce a `..`-laden or absolute
    /// path — but this exists so a future caller/refactor mistake gets a
    /// clear error instead of silently extracting outside the cache.
    #[error(
        "internal error: computed package destination {dest} is not confined to {packages_root}; \
         refusing to extract outside the Harbor packages directory",
        dest = dest.display(),
        packages_root = packages_root.display()
    )]
    UnsafeDestination { dest: PathBuf, packages_root: PathBuf },

    /// Failed to provision language runtime (Phase 6)
    #[error("failed to provision language runtime: {0}")]
    RuntimeProvision(#[source] runtime::RuntimeError),

    /// Failed to install dependencies (Phase 7)
    #[error("failed to install dependencies: {0}")]
    DependencyInstall(#[source] runtime::RuntimeError),

    /// The run target could not be parsed as a valid registry reference,
    /// local archive, or GitHub URL (SPEC.md §9.2).
    #[error("unrecognized run target {target:?}: expected a registry reference (owner/name), \
             a GitHub URL, or a local .harbor path")]
    InvalidTarget { target: String },
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
    let (_, lock_bytes, _) = entries
        .iter()
        .find(|(p, _, _)| p == "harbor.lock")
        .ok_or_else(|| RunError::MissingLockfile { path: archive.to_path_buf() })?;
    let lock_str = String::from_utf8_lossy(lock_bytes).into_owned();
    let lock = Lockfile::from_toml_str(&lock_str).map_err(RunError::LockfileParse)?;

    let expected_checksum = lock
        .package_checksum
        .clone()
        .ok_or(RunError::MissingPackageChecksum)?;
    // `compute_package_checksum_from_bytes` doesn't know about tar mode bits
    // (it hashes path+contents only, per the checksum convention), so build
    // its expected (path, contents) shape here rather than changing that
    // shared cross-module convention.
    let checksum_entries: Vec<(String, Vec<u8>)> =
        entries.iter().map(|(p, c, _)| (p.clone(), c.clone())).collect();
    let actual_checksum = compute_package_checksum_from_bytes(&checksum_entries);
    if actual_checksum != expected_checksum {
        return Err(RunError::ChecksumMismatch {
            expected: expected_checksum,
            actual: actual_checksum,
        });
    }

    // --- Parse harbor.toml (pre-extraction) purely for cache addressing. ---
    let (_, manifest_bytes, _) = entries
        .iter()
        .find(|(p, _, _)| p == "harbor.toml")
        .ok_or_else(|| RunError::MissingManifest { path: archive.to_path_buf() })?;
    let manifest_str = String::from_utf8_lossy(manifest_bytes).into_owned();
    let manifest_for_addressing = Manifest::from_toml_str(&manifest_str)?;

    // CRITICAL (path traversal, see wave4-review.md): validate `name`/
    // `version` BEFORE they're used to build the extraction destination.
    // A checksum-consistent archive only proves internal consistency — it
    // says nothing about whether `name`/`version` are safe path segments.
    // `validate_manifest` enforces the §19.3 charset (lowercase ascii/digit/
    // `_`/`-` only — no `/`, no `.`, no `..`) and SemVer 2.0.0 for `version`
    // (which likewise cannot contain a path separator), so a traversal
    // payload in either field is rejected here, before any extraction.
    // (The warnings returned here are discarded: the authoritative warnings
    // in `Prepared` come from the post-extraction re-validation below.)
    manifest::validate_manifest(&manifest_for_addressing)?;

    // --- H-051: safe extraction, or skip if already cached (§9.5). ---
    let dest = home.local_package_dir(&manifest_for_addressing.name, &manifest_for_addressing.version);

    // Belt-and-braces: even with `name`/`version` validated above, assert
    // the computed destination is still confined to `packages/` before
    // touching the filesystem at all. Never panics — a clear error instead.
    let packages_root = home.packages();
    if !dest.starts_with(&packages_root) {
        return Err(RunError::UnsafeDestination { dest, packages_root });
    }

    let mut from_cache = dir_is_nonempty(&dest).map_err(|e| RunError::Io {
        path: dest.clone(),
        source: e,
    })?;

    if !from_cache {
        // A `true` result means a concurrent `harbor run` of this same
        // archive won the stage-then-rename race first — treat that as a
        // cache hit rather than surfacing the raw rename failure.
        from_cache = extract::extract_entries(&entries, &dest, &home.tmp())?;
    }

    // --- H-052: re-validate + OS check (§9.6, §9.6.1), from the EXTRACTED
    // directory (not the in-memory bytes) — this is what a "cached" run
    // re-validates too, so a manually-tampered cache entry is still caught.
    let (manifest, warnings) = validate_extracted(&dest)?;

    Ok(Prepared {
        name: manifest.name,
        version: manifest.version,
        package_dir: dest,
        from_cache,
        warnings,
    })
}

/// Run the preparation pipeline for a `.harbor` archive that was downloaded
/// from the registry (SPEC.md §9.2–§9.6.1). Mirrors `run_local_archive` but
/// uses the registry-scoped cache under `packages/registry/<owner>/<name>/<version>/`.
pub fn run_registry_archive(
    archive: &Path,
    owner: &str,
    name: &str,
    home: &HarborHome,
) -> Result<Prepared, RunError> {
    let entries = read_archive_entries(archive)?;

    // --- checksum verify (§9.4) ---
    let (_, lock_bytes, _) = entries
        .iter()
        .find(|(p, _, _)| p == "harbor.lock")
        .ok_or_else(|| RunError::MissingLockfile {
            path: archive.to_path_buf(),
        })?;
    let lock_str = String::from_utf8_lossy(lock_bytes).into_owned();
    let lock = Lockfile::from_toml_str(&lock_str).map_err(RunError::LockfileParse)?;

    let expected_checksum = lock
        .package_checksum
        .clone()
        .ok_or(RunError::MissingPackageChecksum)?;
    let checksum_entries: Vec<(String, Vec<u8>)> =
        entries.iter().map(|(p, c, _)| (p.clone(), c.clone())).collect();
    let actual_checksum = compute_package_checksum_from_bytes(&checksum_entries);
    if actual_checksum != expected_checksum {
        return Err(RunError::ChecksumMismatch {
            expected: expected_checksum,
            actual: actual_checksum,
        });
    }

    // --- Parse harbor.toml for cache addressing ---
    let (_, manifest_bytes, _) = entries
        .iter()
        .find(|(p, _, _)| p == "harbor.toml")
        .ok_or_else(|| RunError::MissingManifest {
            path: archive.to_path_buf(),
        })?;
    let manifest_str = String::from_utf8_lossy(manifest_bytes).into_owned();
    let manifest_for_addressing = Manifest::from_toml_str(&manifest_str)?;
    manifest::validate_manifest(&manifest_for_addressing)?;

    // Use the registry name for cache path, not the manifest name (they
    // should match, but registry is the authoritative namespace).
    let pkg_version = &manifest_for_addressing.version;

    // --- destination in registry-scoped cache ---
    let dest = home.registry_package_dir(owner, name, pkg_version);
    let packages_root = home.packages();
    if !dest.starts_with(&packages_root) {
        return Err(RunError::UnsafeDestination {
            dest,
            packages_root,
        });
    }

    let mut from_cache =
        dir_is_nonempty(&dest).map_err(|e| RunError::Io {
            path: dest.clone(),
            source: e,
        })?;

    if !from_cache {
        from_cache = extract::extract_entries(&entries, &dest, &home.tmp())?;
    }

    // --- re-validate + OS check (§9.6, §9.6.1) ---
    let (manifest, warnings) = validate_extracted(&dest)?;

    Ok(Prepared {
        name: manifest.name,
        version: manifest.version,
        package_dir: dest,
        from_cache,
        warnings,
    })
}

/// Re-parse and re-validate `harbor.toml` from an already-extracted package
/// directory, and check OS compatibility (SPEC.md §9.6, §9.6.1).
///
/// This is the pipeline's shared entry point from manifest validation
/// onward: [`run_local_archive`] calls it as its own post-extraction step,
/// and `harbor add` (SPEC.md §12.2 step 7) calls it directly on a freshly
/// imported package directory, since a GitHub import has no `.harbor`
/// archive to check a package-checksum against in the first place.
pub fn validate_extracted(package_dir: &Path) -> Result<(Manifest, Vec<ValidationWarning>), RunError> {
    let manifest_path = package_dir.join("harbor.toml");
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

    Ok((manifest, warnings))
}

/// The result of a successful environment preparation: the environment
/// directory and the language runtime's bin directory.
#[derive(Debug, Clone)]
pub struct PreparedEnvironment {
    pub env_dir: PathBuf,
    pub bin_dir: PathBuf,
}

/// Prepare the isolated runtime environment and install dependencies (SPEC.md §9.7, §9.8).
///
/// `env_dir` is caller-supplied rather than derived here, so this function
/// makes no assumption about a package's source: `cmd_run` passes
/// `home.local_env_dir(name, version)`, `cmd_add` passes
/// `home.github_env_dir(owner, repo, sha)` (SPEC.md §12.2 step 7). `home` is
/// still needed for the runtime managers' own `runtimes/` and `tmp/` use.
///
/// Returns the environment info, or a `RunError` on failure.
pub fn prepare_environment(
    package_dir: &Path,
    manifest: &Manifest,
    home: &HarborHome,
    env_dir: PathBuf,
) -> Result<PreparedEnvironment, RunError> {
    let manager = runtime::get_runtime_manager(manifest.language);

    let bin_dir = manager
        .ensure_runtime(home, &manifest.runtime)
        .map_err(RunError::RuntimeProvision)?;

    if !env_dir.join("harbor-env.ok").is_file() {
        manager
            .install_dependencies(home, package_dir, &env_dir, manifest, &bin_dir)
            .map_err(RunError::DependencyInstall)?;
    }

    Ok(PreparedEnvironment { env_dir, bin_dir })
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

/// Upper bound on how many bytes we'll pre-allocate based on a tar header's
/// (attacker-controlled) `size` field, before actually reading that many
/// bytes out of the gzip stream. A hand-crafted archive can claim any size
/// up to `u64::MAX` in its header independent of what data actually follows;
/// without this cap, `Vec::with_capacity` would attempt that allocation
/// immediately — a cheap denial-of-service from a tiny file on disk.
/// `read_to_end` still grows the buffer past this if the entry legitimately
/// has more content than the cap; this only bounds the up-front guess.
const MAX_ENTRY_PREALLOC: usize = 16 * 1024 * 1024; // 16 MiB

/// Read every regular-file entry out of a gzip-compressed tar (`.harbor`)
/// archive into memory as `(archive-relative path, contents, tar mode bits)`
/// triples. Directory entries (and any other non-regular entry type) are
/// skipped. The mode is carried through purely so [`extract::extract_entries`]
/// can restore the execute bit; it plays no part in checksum verification.
///
/// Packages are small in V1 (SPEC.md), so reading the whole archive into
/// memory once up front — rather than streaming — keeps the checksum-verify
/// -then-extract pipeline simple and lets both steps share one read of the
/// archive.
fn read_archive_entries(archive: &Path) -> Result<Vec<(String, Vec<u8>, u32)>, RunError> {
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
        let mode = entry.header().mode().map_err(map_io)?;
        let cap = (entry.size() as usize).min(MAX_ENTRY_PREALLOC);
        let mut contents = Vec::with_capacity(cap);
        entry.read_to_end(&mut contents).map_err(map_io)?;
        entries.push((path, contents, mode));
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    // ---- H-160: parse_run_target ----

    #[test]
    fn parse_local_archive_by_extension() {
        let result = parse_run_target("./my-agent.harbor").unwrap();
        assert_eq!(
            result,
            RunTarget::LocalArchive(PathBuf::from("./my-agent.harbor"))
        );
    }

    #[test]
    fn parse_local_archive_by_existing_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("my-archive");
        fs::write(&file_path, b"not a real archive").unwrap();
        let result = parse_run_target(file_path.to_str().unwrap()).unwrap();
        assert_eq!(
            result,
            RunTarget::LocalArchive(file_path)
        );
    }

    #[test]
    fn parse_registry_ref_simple() {
        let result = parse_run_target("owner/my-agent").unwrap();
        assert_eq!(
            result,
            RunTarget::RegistryRef {
                owner: "owner".to_string(),
                name: "my-agent".to_string(),
            }
        );
    }

    #[test]
    fn parse_registry_ref_with_digits_and_hyphens() {
        let result = parse_run_target("user123/my-pkg-v2").unwrap();
        assert_eq!(
            result,
            RunTarget::RegistryRef {
                owner: "user123".to_string(),
                name: "my-pkg-v2".to_string(),
            }
        );
    }

    #[test]
    fn parse_github_url_https() {
        let result =
            parse_run_target("https://github.com/octocat/hello-world").unwrap();
        assert_eq!(
            result,
            RunTarget::GitHubUrl("https://github.com/octocat/hello-world".to_string())
        );
    }

    #[test]
    fn parse_github_url_http() {
        let result =
            parse_run_target("http://github.com/octocat/repo").unwrap();
        assert_eq!(
            result,
            RunTarget::GitHubUrl("http://github.com/octocat/repo".to_string())
        );
    }

    #[test]
    fn parse_github_url_with_path_suffix() {
        let result =
            parse_run_target("https://github.com/octocat/hello-world/tree/main").unwrap();
        assert_eq!(
            result,
            RunTarget::GitHubUrl(
                "https://github.com/octocat/hello-world/tree/main".to_string()
            )
        );
    }

    #[test]
    fn parse_invalid_target_no_slash_is_rejected() {
        let err = parse_run_target("justaname").unwrap_err();
        assert!(matches!(err, RunError::InvalidTarget { .. }));
    }

    #[test]
    fn parse_invalid_target_empty_parts() {
        let err = parse_run_target("/name").unwrap_err();
        assert!(matches!(err, RunError::InvalidTarget { .. }));
    }

    #[test]
    fn parse_invalid_target_triple_slash() {
        let err = parse_run_target("a/b/c").unwrap_err();
        assert!(matches!(err, RunError::InvalidTarget { .. }));
    }

    #[test]
    fn parse_rejects_version_pin_syntax() {
        // SPEC.md §9.2/§22: there is no `owner/package@version` pin syntax in
        // V1 — it must fail loudly, not resolve a package named `pkg@1.0.0`.
        let err = parse_run_target("owner/pkg@1.0.0").unwrap_err();
        assert!(matches!(err, RunError::InvalidTarget { .. }));
    }

    #[test]
    fn parse_rejects_traversal_in_name() {
        // A `..` component could fold up a cache directory level.
        assert!(matches!(
            parse_run_target("owner/..").unwrap_err(),
            RunError::InvalidTarget { .. }
        ));
        assert!(matches!(
            parse_run_target("../name").unwrap_err(),
            RunError::InvalidTarget { .. }
        ));
    }

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

    // ---- CRITICAL fix (wave4-review.md): manifest name/version must be
    // validated BEFORE they're used to build the extraction destination,
    // not just the tar entry paths. ----

    #[test]
    fn manifest_name_path_traversal_is_rejected_before_any_extraction() {
        let dir = tempdir().unwrap();
        // A hand-crafted archive with a hostile `name` field. The package
        // is otherwise entirely self-consistent — the checksum is computed
        // over these exact bytes — because the point is that internal
        // consistency alone must NOT be enough to reach extraction.
        let entries = sample_entries("../../evil", "1.0.0", &[]);
        let archive_path = dir.path().join("evil-name.harbor");
        write_archive(&archive_path, &entries);

        let home = HarborHome::at(dir.path().join("home"));
        home.ensure_layout().unwrap();

        let err = run_local_archive(&archive_path, &home).unwrap_err();
        assert!(
            matches!(err, RunError::ManifestValidation(_)),
            "expected the name-charset rule (§19.3) to reject this before extraction, got: {err:?}"
        );

        // Nothing must have been written anywhere: not under packages/, and
        // not at the traversal target either. `home/packages/local/../../evil`
        // resolves to `home/evil`.
        assert!(!home.packages().join("local").exists());
        assert!(!dir.path().join("home").join("evil").exists());
        assert!(!dir.path().join("evil").exists());
    }

    #[test]
    fn manifest_version_path_traversal_is_rejected_before_any_extraction() {
        let dir = tempdir().unwrap();
        let entries = sample_entries("evil-version-agent", "../../evil", &[]);
        let archive_path = dir.path().join("evil-version.harbor");
        write_archive(&archive_path, &entries);

        let home = HarborHome::at(dir.path().join("home"));
        home.ensure_layout().unwrap();

        let err = run_local_archive(&archive_path, &home).unwrap_err();
        assert!(
            matches!(err, RunError::ManifestValidation(_)),
            "expected SemVer validation to reject this before extraction, got: {err:?}"
        );

        assert!(!home.packages().join("local").exists());
        assert!(!dir.path().join("home").join("evil").exists());
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

    // ---- validate_extracted: the shared §9.6+ entry point (H-113) ----

    #[test]
    fn validate_extracted_happy_path_returns_manifest_and_no_warnings() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("harbor.toml"),
            minimal_manifest_toml("standalone-agent", "1.0.0", &[]),
        )
        .unwrap();
        fs::write(dir.path().join("src/main.py"), "print('hi')\n").unwrap();

        let (manifest, warnings) = validate_extracted(dir.path()).expect("should validate");
        assert_eq!(manifest.name, "standalone-agent");
        assert_eq!(manifest.version, "1.0.0");
        assert!(warnings.is_empty());
    }

    #[test]
    fn validate_extracted_surfaces_manifest_validation_errors() {
        let dir = tempdir().unwrap();
        // Invalid `name` per §19.3 (uppercase not allowed) — must be caught
        // by re-validation, exactly as it would be inside run_local_archive.
        fs::write(
            dir.path().join("harbor.toml"),
            minimal_manifest_toml("Not-A-Valid-Name", "1.0.0", &[]),
        )
        .unwrap();

        let err = validate_extracted(dir.path()).unwrap_err();
        assert!(matches!(err, RunError::ManifestValidation(_)), "got: {err:?}");
    }

    #[test]
    fn validate_extracted_rejects_missing_harbor_toml() {
        let dir = tempdir().unwrap();
        let err = validate_extracted(dir.path()).unwrap_err();
        assert!(matches!(err, RunError::Io { .. }), "got: {err:?}");
    }

    #[test]
    fn validate_extracted_enforces_os_compatibility() {
        let current = current_os();
        let other = if current == "macos" { "linux" } else { "macos" };

        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("harbor.toml"),
            minimal_manifest_toml("os-mismatch-agent", "1.0.0", &[other]),
        )
        .unwrap();

        let err = validate_extracted(dir.path()).unwrap_err();
        assert!(matches!(err, RunError::UnsupportedOs { .. }), "got: {err:?}");
    }

    // ---- executable mode bits survive extraction (wave4-review.md) ----

    #[cfg(unix)]
    #[test]
    fn executable_bit_is_preserved_through_the_real_build_and_run_pipeline() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let pkg_root = dir.path().join("pkg");
        fs::create_dir_all(pkg_root.join("src")).unwrap();

        fs::write(
            pkg_root.join("harbor.toml"),
            minimal_manifest_toml("exec-agent", "1.0.0", &[]),
        )
        .unwrap();
        fs::write(pkg_root.join("src/main.py"), b"print('hi')\n").unwrap();

        // An executable entrypoint script, mode set on disk before the
        // real `package::collect_files` + `package::build_archive` pipeline
        // ever sees it — this is what causes `build_archive` to record
        // tar mode 0o755 for this entry (see `package.rs`'s `is_executable`).
        let entry_script = pkg_root.join("run.sh");
        fs::write(&entry_script, b"#!/bin/sh\necho hi\n").unwrap();
        fs::set_permissions(&entry_script, fs::Permissions::from_mode(0o755)).unwrap();

        // harbor.lock, with a package-checksum computed over everything
        // except itself, exactly as `harbor push` would produce it.
        let files_before_lock = crate::package::collect_files(&pkg_root).unwrap();
        let checksum = crate::lockfile::compute_package_checksum(&files_before_lock).unwrap();
        let lock = lockfile_with_checksum("1.0.0", Some(checksum));
        fs::write(pkg_root.join("harbor.lock"), lock.to_toml_string().unwrap()).unwrap();

        let files = crate::package::collect_files(&pkg_root).unwrap();
        let archive_path = dir.path().join("exec-agent-1.0.0.harbor");
        crate::package::build_archive(&pkg_root, &files, &archive_path).unwrap();

        let home = HarborHome::at(dir.path().join("home"));
        home.ensure_layout().unwrap();

        let prepared = run_local_archive(&archive_path, &home).expect("should succeed");

        let extracted_script = prepared.package_dir.join("run.sh");
        let script_mode = fs::metadata(&extracted_script).unwrap().permissions().mode();
        assert_ne!(
            script_mode & 0o111,
            0,
            "execute bit should survive the full build-then-run pipeline, got mode {script_mode:o}"
        );

        // A non-executable file in the same package must not gain the bit.
        let extracted_readme = prepared.package_dir.join("src/main.py");
        let py_mode = fs::metadata(&extracted_readme).unwrap().permissions().mode();
        assert_eq!(
            py_mode & 0o111,
            0,
            "non-executable entry must not gain the execute bit, got mode {py_mode:o}"
        );
    }
}
