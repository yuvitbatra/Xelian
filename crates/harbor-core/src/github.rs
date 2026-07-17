//! GitHub import front end for `harbor add` (SPEC.md §12).
//!
//! GitHub repositories are an **import source**, not the canonical registry
//! (§12.1). This module implements §12.2 steps 1–5: resolving a repository's
//! default branch to a commit SHA, downloading and caching the repository at
//! that SHA, detecting its language, inferring a `harbor.toml`, and building
//! the `.harbor` package + `harbor.lock`. It performs no publishing and no
//! registry interaction (§12.3) — running the built package (§12.2 step 7)
//! and wiring this into the `harbor add` CLI command are later work.
//!
//! All user-facing progress/status output goes to stderr: stdout is reserved
//! for a launched package's own output (MCP stdio transport), matching the
//! convention in `run/runtime.rs` and `run/model.rs`.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use thiserror::Error;

use crate::cache::HarborHome;
use crate::errors::ManifestError;
use crate::lockfile::{self, LockfileError};
use crate::manifest::{Language, Manifest};
use crate::package::{self, PackageError};
use crate::run::extract::{self, ExtractError};
use crate::run::runtime::{run_command_checked, NodeRuntimeManager, RuntimeError};

/// Errors that can occur while importing a GitHub repository (§12.2).
#[derive(Debug, Error)]
pub enum GithubError {
    /// `parse_github_url` rejected the input — not `https://github.com/<owner>/<repo>`
    /// (with an optional trailing `/` or `.git` suffix), or `<owner>`/`<repo>`
    /// is not a safe, plain path component.
    #[error("invalid GitHub repository URL {url:?}: {reason}")]
    InvalidUrl { url: String, reason: String },

    /// `git` or `curl` is required for `harbor add` and was not found (or is
    /// not executable) in `PATH`.
    #[error("{0} is required for `harbor add` but was not found (or is not executable) in PATH")]
    MissingDependency(String),

    /// `git ls-remote` failed outright (network error, repo does not exist,
    /// etc). Carries the underlying process error, including its stderr.
    ///
    /// `source` is boxed to keep `GithubError` itself small (clippy
    /// `result_large_err`) — `RuntimeError` carries full command
    /// stdout/stderr capture and is comparatively large.
    #[error("failed to resolve the default branch for {owner}/{repo}: {source}")]
    ResolveHead {
        owner: String,
        repo: String,
        #[source]
        source: Box<RuntimeError>,
    },

    /// `git ls-remote ... HEAD` ran successfully but its output did not
    /// contain a well-formed 40-lowercase-hex-char commit SHA on its first
    /// line.
    #[error("`git ls-remote` for {owner}/{repo} returned an unexpected HEAD line: {line:?}")]
    UnexpectedHeadFormat { owner: String, repo: String, line: String },

    /// Downloading the repository tarball via `curl` failed. `source` is
    /// boxed for the same reason as [`GithubError::ResolveHead`].
    #[error("failed to download {owner}/{repo} at {sha}: {source}")]
    Download {
        owner: String,
        repo: String,
        sha: String,
        #[source]
        source: Box<RuntimeError>,
    },

    /// A tarball entry's path, after stripping the GitHub-tarball top-level
    /// directory prefix, is unsafe (absolute, or contains `..`) — rejected
    /// before anything is written to disk (mirrors `run::extract::is_path_safe`).
    #[error(
        "GitHub archive entry {path:?} has an unsafe path after stripping the top-level \
         directory and was rejected"
    )]
    UnsafeTarEntry { path: String },

    /// Safe-extraction failure once tarball entries have been validated
    /// (staging/rename I/O failure — see `run::extract`).
    #[error(transparent)]
    Extract(#[from] ExtractError),

    /// Detected a `Cargo.toml` at the repository root: Rust has no runtime
    /// manager in V1 (§22), so import must fail with a clear message rather
    /// than attempting to proceed (§12.2 step 2).
    #[error("unsupported language ({language}): no runtime manager exists for this language in V1")]
    UnsupportedLanguage { language: String },

    /// None of the known language marker files were found at the repository
    /// root.
    #[error(
        "could not detect project language: no recognized project manifest \
         (pyproject.toml, package.json, ...) found at the repository root"
    )]
    UndetectedLanguage,

    /// The generated (or pre-existing) `harbor.toml` failed to parse.
    #[error(transparent)]
    ManifestParse(#[from] ManifestError),

    /// File-set collection or archive building failed (§5, §8.1).
    #[error(transparent)]
    Package(#[from] PackageError),

    /// `harbor.lock` generation or serialization failed (§7).
    #[error(transparent)]
    Lockfile(#[from] LockfileError),

    /// The repository's own `.gitignore` would exclude `harbor.toml` from
    /// the package file set — building an archive without it would produce a
    /// broken package, so import fails clearly instead (§12.2 steps 4–5).
    #[error(
        "harbor.toml would be excluded from the package archive by .gitignore; cannot build \
         a valid import — remove or adjust the .gitignore rule that excludes it"
    )]
    ManifestExcludedByGitignore,

    /// I/O failure not covered by a more specific variant above.
    #[error("I/O error at {path}: {source}", path = path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// A parsed `https://github.com/<owner>/<repo>` reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub owner: String,
    pub repo: String,
}

/// Parse a GitHub repository URL of the form `https://github.com/<owner>/<repo>`,
/// with an optional trailing `/` and/or `.git` suffix.
///
/// Rejects: non-`https` schemes, non-`github.com` hosts, extra path segments
/// (e.g. `/tree/main`), and empty or unsafe `owner`/`repo` components
/// (defense in depth — these values become cache path components, so `..`
/// and leading `.` are refused even though GitHub itself would already
/// reject most such names).
pub fn parse_github_url(url: &str) -> Result<RepoRef, GithubError> {
    if url.starts_with("http://") {
        return Err(GithubError::InvalidUrl {
            url: url.to_string(),
            reason: "http is not supported; use https://".to_string(),
        });
    }

    let rest = url.strip_prefix("https://").ok_or_else(|| GithubError::InvalidUrl {
        url: url.to_string(),
        reason: "must start with https://".to_string(),
    })?;

    let rest = rest.strip_prefix("github.com/").ok_or_else(|| GithubError::InvalidUrl {
        url: url.to_string(),
        reason: "host must be github.com".to_string(),
    })?;

    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let rest = rest.strip_suffix(".git").unwrap_or(rest);

    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() != 2 {
        return Err(GithubError::InvalidUrl {
            url: url.to_string(),
            reason: "expected exactly https://github.com/<owner>/<repo>".to_string(),
        });
    }

    let (owner, repo) = (parts[0], parts[1]);
    if !is_safe_repo_component(owner) {
        return Err(GithubError::InvalidUrl {
            url: url.to_string(),
            reason: format!("invalid owner {owner:?}"),
        });
    }
    if !is_safe_repo_component(repo) {
        return Err(GithubError::InvalidUrl {
            url: url.to_string(),
            reason: format!("invalid repo {repo:?}"),
        });
    }

    Ok(RepoRef {
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

/// Whether `s` is safe to use as a single cache path component: non-empty,
/// not `..`, not starting with `.`, and containing no path separators.
fn is_safe_repo_component(s: &str) -> bool {
    !s.is_empty() && s != ".." && !s.starts_with('.') && !s.contains('/') && !s.contains('\\')
}

/// Whether `name` is a well-formed `<binary> --version`-style availability
/// check for `git`/`curl`, run before any operation that needs them so a
/// missing dependency produces a clear, specific error instead of a raw
/// "No such file or directory".
fn ensure_binary_available(name: &str) -> Result<(), GithubError> {
    let ok = Command::new(name)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err(GithubError::MissingDependency(name.to_string()))
    }
}

/// Whether `s` is exactly 40 lowercase hexadecimal characters (a full Git
/// commit SHA-1).
fn is_valid_commit_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Resolve a repository's default branch (`HEAD`) to a commit SHA (SPEC.md
/// §12.2 step 1, H-110) via `git ls-remote <url> HEAD` — no GitHub API call,
/// so no rate limits.
pub fn resolve_head_sha(repo: &RepoRef) -> Result<String, GithubError> {
    ensure_binary_available("git")?;

    let url = format!("https://github.com/{}/{}.git", repo.owner, repo.repo);
    let mut cmd = Command::new("git");
    cmd.arg("ls-remote").arg(&url).arg("HEAD");
    let output = run_command_checked(&mut cmd).map_err(|e| GithubError::ResolveHead {
        owner: repo.owner.clone(),
        repo: repo.repo.clone(),
        source: Box::new(e),
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next().unwrap_or("").trim();
    let sha = first_line.split_whitespace().next().unwrap_or("");

    if !is_valid_commit_sha(sha) {
        return Err(GithubError::UnexpectedHeadFormat {
            owner: repo.owner.clone(),
            repo: repo.repo.clone(),
            line: first_line.to_string(),
        });
    }

    Ok(sha.to_string())
}

/// Whether `dir` exists and contains at least one entry.
fn dir_is_nonempty(dir: &Path) -> bool {
    match fs::read_dir(dir) {
        Ok(mut rd) => rd.next().is_some(),
        Err(_) => false,
    }
}

/// Strip the top-level path component GitHub tarballs wrap every entry in
/// (`<repo>-<sha>/...`). Returns `None` for the top-level directory entry
/// itself (nothing left after stripping) — it carries no content to extract.
fn strip_top_level_component(path: &str) -> Option<String> {
    match path.split_once('/') {
        Some((_, rest)) if !rest.is_empty() => Some(rest.to_string()),
        _ => None,
    }
}

/// Read a downloaded GitHub tarball into the `(archive-relative path,
/// contents, tar mode bits)` triples [`extract::extract_entries`] expects,
/// stripping the GitHub-tarball top-level directory prefix from every entry.
///
/// Every stripped path is validated with [`extract::is_path_safe`] before
/// its contents are read; an unsafe path is a hard error (not silently
/// skipped). Symlink entries are skipped with a stderr warning — an archive
/// symlink extracted into a shared cache directory is a traversal hazard.
/// Pax header entries and other non regular/directory/symlink entry types
/// tar crates may surface are silently ignored.
fn read_github_tarball_entries(tarball_path: &Path) -> Result<Vec<(String, Vec<u8>, u32)>, GithubError> {
    let map_io = |source: io::Error| GithubError::Io {
        path: tarball_path.to_path_buf(),
        source,
    };

    let file = fs::File::open(tarball_path).map_err(map_io)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    let mut out = Vec::new();
    for entry in archive.entries().map_err(map_io)? {
        let mut entry = entry.map_err(map_io)?;
        let entry_type = entry.header().entry_type();
        let raw_path = entry.path().map_err(map_io)?.to_string_lossy().into_owned();

        let Some(stripped) = strip_top_level_component(&raw_path) else {
            continue;
        };

        match entry_type {
            tar::EntryType::Regular => {
                if !extract::is_path_safe(&stripped) {
                    return Err(GithubError::UnsafeTarEntry { path: stripped });
                }
                let mode = entry.header().mode().unwrap_or(0o644);
                let mut contents = Vec::new();
                entry.read_to_end(&mut contents).map_err(map_io)?;
                out.push((stripped, contents, mode));
            }
            tar::EntryType::Directory => {
                if !extract::is_path_safe(&stripped) {
                    return Err(GithubError::UnsafeTarEntry { path: stripped });
                }
                // Directories are implicit: extract_entries creates parent
                // directories as needed from each file's path.
            }
            tar::EntryType::Symlink => {
                eprintln!("warning: skipping symlink entry in GitHub archive: {stripped}");
            }
            _ => {
                // Pax global/extended headers and any other entry type we
                // don't need (device nodes, fifos, ...): ignore.
            }
        }
    }

    Ok(out)
}

/// Download and extract a repository at a specific commit SHA (SPEC.md
/// §12.2 step 1, H-110), caching it at
/// `home.github_package_dir(owner, repo, sha)`.
///
/// If the destination already exists and is non-empty, returns it
/// immediately without any network activity — packages are immutable once
/// cached (§9.11), and imports are addressed by commit SHA specifically so
/// this is safe.
pub fn fetch_repo(repo: &RepoRef, sha: &str, home: &HarborHome) -> Result<PathBuf, GithubError> {
    let dest = home.github_package_dir(&repo.owner, &repo.repo, sha);
    if dir_is_nonempty(&dest) {
        eprintln!("(cached) {}/{} @ {sha} — {}", repo.owner, repo.repo, dest.display());
        return Ok(dest);
    }

    ensure_binary_available("curl")?;

    fs::create_dir_all(home.tmp()).map_err(|e| GithubError::Io {
        path: home.tmp(),
        source: e,
    })?;

    let tarball_path = home.tmp().join(format!("{}-{}-{}.tar.gz", repo.owner, repo.repo, sha));
    let url = format!(
        "https://codeload.github.com/{}/{}/tar.gz/{}",
        repo.owner, repo.repo, sha
    );

    eprintln!("Downloading {}/{} @ {sha}...", repo.owner, repo.repo);
    let mut cmd = Command::new("curl");
    cmd.arg("-LsSf").arg(&url).arg("-o").arg(&tarball_path);
    run_command_checked(&mut cmd).map_err(|e| GithubError::Download {
        owner: repo.owner.clone(),
        repo: repo.repo.clone(),
        sha: sha.to_string(),
        source: Box::new(e),
    })?;

    let result = (|| -> Result<(), GithubError> {
        let entries = read_github_tarball_entries(&tarball_path)?;
        extract::extract_entries(&entries, &dest, &home.tmp())?;
        Ok(())
    })();

    // Best-effort cleanup regardless of outcome.
    let _ = fs::remove_file(&tarball_path);

    result?;
    Ok(dest)
}

/// Language detection marker table (SPEC.md §12.2 step 2, H-111): checked in
/// order, first match wins. Extensible by appending a row, not by
/// redesigning this function.
enum DetectionOutcome {
    Language(Language),
    UnsupportedLanguage(&'static str),
}

const LANGUAGE_MARKERS: &[(&str, DetectionOutcome)] = &[
    ("pyproject.toml", DetectionOutcome::Language(Language::Python)),
    ("package.json", DetectionOutcome::Language(Language::Node)),
    ("Cargo.toml", DetectionOutcome::UnsupportedLanguage("rust")),
];

/// Detect a checked-out repository's language by marker-file precedence
/// (SPEC.md §12.2 step 2, H-111).
pub fn detect_language(checkout: &Path) -> Result<Language, GithubError> {
    for (marker, outcome) in LANGUAGE_MARKERS {
        if checkout.join(marker).is_file() {
            return match outcome {
                DetectionOutcome::Language(lang) => Ok(*lang),
                DetectionOutcome::UnsupportedLanguage(name) => {
                    Err(GithubError::UnsupportedLanguage { language: (*name).to_string() })
                }
            };
        }
    }
    Err(GithubError::UndetectedLanguage)
}

fn language_label(language: Language) -> &'static str {
    match language {
        Language::Python => "python",
        Language::Node => "node",
    }
}

#[derive(Deserialize)]
struct PyProjectToml {
    project: Option<PyProjectProjectTable>,
}

#[derive(Deserialize)]
struct PyProjectProjectTable {
    #[serde(rename = "requires-python")]
    requires_python: Option<String>,
}

/// Infer the `runtime` constraint for a Python project: `requires-python`
/// from `[project]` in `pyproject.toml` if present and parseable, else the
/// placeholder `">=3.9"`.
fn infer_python_runtime(checkout: &Path) -> String {
    let Ok(contents) = fs::read_to_string(checkout.join("pyproject.toml")) else {
        return ">=3.9".to_string();
    };
    let Ok(parsed) = toml::from_str::<PyProjectToml>(&contents) else {
        return ">=3.9".to_string();
    };
    parsed
        .project
        .and_then(|p| p.requires_python)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| ">=3.9".to_string())
}

/// First existing of the conventional Python entrypoint locations, else the
/// placeholder `"PLEASE_EDIT_entrypoint.py"` (import must not fail — launch
/// will later fail with a clear `EntrypointMissing`).
fn infer_python_entrypoint(checkout: &Path) -> String {
    for candidate in ["main.py", "src/main.py", "app.py", "src/app.py"] {
        if checkout.join(candidate).is_file() {
            return candidate.to_string();
        }
    }
    "PLEASE_EDIT_entrypoint.py".to_string()
}

#[derive(Deserialize)]
struct PackageJson {
    main: Option<String>,
    engines: Option<PackageJsonEngines>,
}

#[derive(Deserialize)]
struct PackageJsonEngines {
    node: Option<String>,
}

fn read_package_json(checkout: &Path) -> Option<PackageJson> {
    let contents = fs::read_to_string(checkout.join("package.json")).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Infer the `runtime` constraint for a Node project: `engines.node` from
/// `package.json` if present and syntactically supported by
/// [`NodeRuntimeManager`]'s constraint parser, else the placeholder `">=18"`.
fn infer_node_runtime(checkout: &Path) -> String {
    let Some(constraint) = read_package_json(checkout).and_then(|p| p.engines).and_then(|e| e.node)
    else {
        return ">=18".to_string();
    };
    if constraint.trim().is_empty() {
        return ">=18".to_string();
    }
    if NodeRuntimeManager.validate_constraint_syntax(&constraint).is_ok() {
        constraint
    } else {
        ">=18".to_string()
    }
}

/// Infer the `entrypoint` for a Node project: `package.json`'s `main` field
/// if that file exists, else the first existing of `index.js`/`src/index.js`,
/// else the placeholder `"PLEASE_EDIT_entrypoint.js"`.
fn infer_node_entrypoint(checkout: &Path) -> String {
    if let Some(main) = read_package_json(checkout).and_then(|p| p.main) {
        if checkout.join(&main).is_file() {
            return main;
        }
    }
    for candidate in ["index.js", "src/index.js"] {
        if checkout.join(candidate).is_file() {
            return candidate.to_string();
        }
    }
    "PLEASE_EDIT_entrypoint.js".to_string()
}

/// Slugify a raw string into the SPEC.md §19.3 name charset: lowercase every
/// character; replace every character outside `[a-z0-9_-]` with `-`;
/// collapse runs of `-`; trim leading/trailing `-`.
fn slugify_name(raw: &str) -> String {
    let mut mapped = String::with_capacity(raw.len());
    for c in raw.chars() {
        let lc = c.to_ascii_lowercase();
        if lc.is_ascii_lowercase() || lc.is_ascii_digit() || lc == '_' || lc == '-' {
            mapped.push(lc);
        } else {
            mapped.push('-');
        }
    }

    let mut collapsed = String::with_capacity(mapped.len());
    let mut prev_dash = false;
    for c in mapped.chars() {
        if c == '-' {
            if !prev_dash {
                collapsed.push('-');
            }
            prev_dash = true;
        } else {
            collapsed.push(c);
            prev_dash = false;
        }
    }

    collapsed.trim_matches('-').to_string()
}

/// Derive a package `name` from a repository name (SPEC.md §19.3, §12.2 step
/// 3): slugify, then fall back to the placeholder `"imported-package"` if the
/// result still doesn't satisfy the naming rules (reuses
/// [`crate::init::is_valid_package_name`] rather than a third copy of the
/// rule).
fn derive_package_name(repo_name: &str) -> String {
    let candidate = slugify_name(repo_name);
    if crate::init::is_valid_package_name(&candidate) {
        candidate
    } else {
        "imported-package".to_string()
    }
}

/// Render the inferred `harbor.toml` contents (SPEC.md §12.2 step 3).
#[allow(clippy::too_many_arguments)]
fn render_manifest_toml(
    name: &str,
    description: &str,
    language: Language,
    runtime: &str,
    entrypoint: &str,
    dep_manifest: &str,
    dep_lockfile: Option<&str>,
) -> String {
    use std::fmt::Write as _;

    let mut s = String::new();
    let _ = writeln!(s, "spec-version = 1");
    let _ = writeln!(s, "name = \"{name}\"");
    let _ = writeln!(s, "version = \"0.0.0\"");
    let _ = writeln!(s, "description = \"{description}\"");
    let _ = writeln!(s, "package-type = \"agent\"");
    let _ = writeln!(s, "language = \"{}\"", language_label(language));
    let _ = writeln!(s, "runtime = \"{runtime}\"");
    let _ = writeln!(s, "entrypoint = \"{entrypoint}\"");
    let _ = writeln!(s, "license = \"PLEASE_EDIT\"");
    let _ = writeln!(s, "permissions = []");
    let _ = writeln!(s, "features = []");
    let _ = writeln!(s);
    let _ = writeln!(s, "[author]");
    let _ = writeln!(s, "name = \"PLEASE_EDIT\"");
    let _ = writeln!(s, "email = \"please-edit@example.invalid\"");
    let _ = writeln!(s);
    let _ = writeln!(s, "[dependencies]");
    let _ = writeln!(s, "manifest = \"{dep_manifest}\"");
    if let Some(lockfile) = dep_lockfile {
        let _ = writeln!(s, "lockfile = \"{lockfile}\"");
    }
    s
}

/// Infer and write a `harbor.toml` manifest into `checkout` (SPEC.md §12.2
/// step 3, H-112), or return an existing one verbatim.
///
/// `language`, `runtime`, `entrypoint`, and `dependencies` are inferred from
/// repository conventions; every other field that cannot be mechanically
/// derived gets a placeholder value (`PLEASE_EDIT*`) — this function MUST
/// NOT fail merely because such fields are placeholders (§12.2 step 3).
///
/// If `checkout` already contains a `harbor.toml` (the repository is already
/// a Harbor package), it is kept verbatim and returned as-is — no
/// inference, no overwrite.
pub fn infer_manifest(checkout: &Path, repo: &RepoRef, sha: &str) -> Result<String, GithubError> {
    let manifest_path = checkout.join("harbor.toml");
    if manifest_path.is_file() {
        eprintln!(
            "{}/{} already contains a harbor.toml; keeping it as-is",
            repo.owner, repo.repo
        );
        return fs::read_to_string(&manifest_path).map_err(|e| GithubError::Io {
            path: manifest_path.clone(),
            source: e,
        });
    }

    let language = detect_language(checkout)?;
    eprintln!("Detected language: {}", language_label(language));

    let (runtime, entrypoint, dep_manifest, dep_lockfile): (String, String, &str, Option<&str>) =
        match language {
            Language::Python => {
                let lockfile = checkout.join("uv.lock").is_file().then_some("uv.lock");
                (
                    infer_python_runtime(checkout),
                    infer_python_entrypoint(checkout),
                    "pyproject.toml",
                    lockfile,
                )
            }
            Language::Node => {
                let lockfile = checkout
                    .join("package-lock.json")
                    .is_file()
                    .then_some("package-lock.json");
                (
                    infer_node_runtime(checkout),
                    infer_node_entrypoint(checkout),
                    "package.json",
                    lockfile,
                )
            }
        };

    let name = derive_package_name(&repo.repo);
    let description = format!(
        "Imported from https://github.com/{}/{} at {sha}",
        repo.owner, repo.repo
    );

    let manifest_toml = render_manifest_toml(
        &name,
        &description,
        language,
        &runtime,
        &entrypoint,
        dep_manifest,
        dep_lockfile,
    );

    // The generated manifest MUST parse before it's written — a bug here is
    // ours, not the imported repo's, so surface it the same way as a normal
    // parse failure rather than writing something broken to disk.
    let manifest = Manifest::from_toml_str(&manifest_toml)?;
    eprintln!("Inferred entrypoint: {}", manifest.entrypoint);

    fs::write(&manifest_path, &manifest_toml).map_err(|e| GithubError::Io {
        path: manifest_path.clone(),
        source: e,
    })?;

    Ok(manifest_toml)
}

/// Generate `harbor.lock` and build the `.harbor` archive for an imported
/// checkout (SPEC.md §12.2 steps 4–5).
///
/// Deliberately does not use [`crate::validate::validate_and_build`]: that
/// pipeline enforces §8.1 publish-time rules (README/LICENSE present,
/// declared entrypoint exists, etc.) that an arbitrary imported repository
/// may not satisfy, and imported packages are local-only until an explicit
/// `harbor push` (§12.3) re-runs full validation. Instead this mirrors
/// `validate_and_build`'s collect → generate-lock → force-include-lock →
/// build-archive ordering directly, using [`package::collect_files`],
/// [`lockfile::generate`], and [`package::build_archive`].
pub fn build_import(checkout: &Path) -> Result<(), GithubError> {
    let manifest_path = checkout.join("harbor.toml");
    let manifest_str = fs::read_to_string(&manifest_path).map_err(|e| GithubError::Io {
        path: manifest_path.clone(),
        source: e,
    })?;
    let manifest = Manifest::from_toml_str(&manifest_str)?;

    let mut files = package::collect_files(checkout)?;
    if !files.iter().any(|(p, _)| p == "harbor.toml") {
        return Err(GithubError::ManifestExcludedByGitignore);
    }

    let lock = lockfile::generate(&manifest, checkout, &files)?;
    let lock_toml = lock.to_toml_string()?;
    let lock_path = checkout.join("harbor.lock");
    fs::write(&lock_path, &lock_toml).map_err(|e| GithubError::Io {
        path: lock_path.clone(),
        source: e,
    })?;

    // Force-include the freshly written harbor.lock, exactly as
    // `validate::validate_and_build` does, so a .gitignore rule can never
    // exclude it from the archive.
    files.retain(|(p, _)| p != "harbor.lock");
    files.push(("harbor.lock".to_string(), lock_path.clone()));
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

    // Build via a temp path + rename (mirrors `validate::validate_and_build`)
    // so a failure partway through never leaves a partial archive at the
    // final path.
    let archive_name = format!("{}-{}.harbor", manifest.name, manifest.version);
    let out_path = checkout.join(&archive_name);
    let tmp_path = out_path.with_extension("harbor.tmp");

    if let Err(e) = package::build_archive(checkout, &files, &tmp_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e.into());
    }
    fs::rename(&tmp_path, &out_path).map_err(|e| GithubError::Io {
        path: out_path.clone(),
        source: e,
    })?;

    Ok(())
}

/// The outcome of a successful GitHub import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOutcome {
    pub repo: RepoRef,
    pub sha: String,
    pub package_dir: PathBuf,
    /// `true` if this SHA had already been fully imported previously (the
    /// cached directory already contained `harbor.toml` and `harbor.lock`),
    /// so detection/inference/lock/archive were all skipped.
    pub from_cache: bool,
}

/// Import a GitHub repository as a local Harbor package (SPEC.md §12.2 steps
/// 1–5): resolve the default branch to a commit SHA, download and cache the
/// repository at that SHA (skipped entirely on a cache hit), detect its
/// language, infer a `harbor.toml`, and build `harbor.lock` + the `.harbor`
/// archive. Does not publish anything (§12.3) and does not run the package —
/// running it (§12.2 step 7) is the caller's responsibility.
pub fn import_github(url: &str, home: &HarborHome) -> Result<ImportOutcome, GithubError> {
    let repo = parse_github_url(url)?;

    eprintln!("Resolving {}/{}...", repo.owner, repo.repo);
    let sha = resolve_head_sha(&repo)?;
    eprintln!("Resolved to commit {sha}");

    let checkout = fetch_repo(&repo, &sha, home)?;

    // An already-imported SHA directory already contains harbor.toml and
    // harbor.lock from a previous complete import — packages are immutable
    // (§9.11), so there is nothing further to do.
    let already_imported = checkout.join("harbor.toml").is_file() && checkout.join("harbor.lock").is_file();
    if already_imported {
        return Ok(ImportOutcome {
            repo,
            sha,
            package_dir: checkout,
            from_cache: true,
        });
    }

    infer_manifest(&checkout, &repo, &sha)?;
    build_import(&checkout)?;
    eprintln!("Cached at {}", checkout.display());

    Ok(ImportOutcome {
        repo,
        sha,
        package_dir: checkout,
        from_cache: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ---- URL parsing ----

    #[test]
    fn parses_valid_url() {
        let r = parse_github_url("https://github.com/octocat/hello-world").unwrap();
        assert_eq!(r.owner, "octocat");
        assert_eq!(r.repo, "hello-world");
    }

    #[test]
    fn parses_url_with_trailing_slash() {
        let r = parse_github_url("https://github.com/octocat/hello-world/").unwrap();
        assert_eq!(r.owner, "octocat");
        assert_eq!(r.repo, "hello-world");
    }

    #[test]
    fn parses_url_with_dot_git_suffix() {
        let r = parse_github_url("https://github.com/octocat/hello-world.git").unwrap();
        assert_eq!(r.owner, "octocat");
        assert_eq!(r.repo, "hello-world");
    }

    #[test]
    fn rejects_extra_path_segments() {
        let err = parse_github_url("https://github.com/octocat/hello-world/tree/main").unwrap_err();
        assert!(matches!(err, GithubError::InvalidUrl { .. }));
    }

    #[test]
    fn rejects_non_github_host() {
        let err = parse_github_url("https://gitlab.com/octocat/hello-world").unwrap_err();
        assert!(matches!(err, GithubError::InvalidUrl { .. }));
    }

    #[test]
    fn rejects_http_scheme() {
        let err = parse_github_url("http://github.com/octocat/hello-world").unwrap_err();
        assert!(matches!(err, GithubError::InvalidUrl { .. }));
    }

    #[test]
    fn rejects_empty_owner_or_repo() {
        assert!(parse_github_url("https://github.com//hello-world").is_err());
        assert!(parse_github_url("https://github.com/octocat/").is_err());
    }

    #[test]
    fn rejects_traversal_components_defensively() {
        assert!(parse_github_url("https://github.com/../hello-world").is_err());
        assert!(parse_github_url("https://github.com/octocat/..").is_err());
    }

    // ---- Language detection precedence ----

    #[test]
    fn python_wins_when_both_pyproject_and_package_json_present() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "[project]\nname = \"x\"\n").unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        assert_eq!(detect_language(dir.path()).unwrap(), Language::Python);
    }

    #[test]
    fn node_detected_when_only_package_json_present() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        assert_eq!(detect_language(dir.path()).unwrap(), Language::Node);
    }

    #[test]
    fn cargo_toml_is_a_clear_unsupported_rust_error() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

        let err = detect_language(dir.path()).unwrap_err();
        match err {
            GithubError::UnsupportedLanguage { language } => assert_eq!(language, "rust"),
            other => panic!("expected UnsupportedLanguage, got {other:?}"),
        }
    }

    #[test]
    fn empty_dir_is_undetected() {
        let dir = tempdir().unwrap();
        let err = detect_language(dir.path()).unwrap_err();
        assert!(matches!(err, GithubError::UndetectedLanguage));
    }

    // ---- Manifest inference ----

    fn sample_repo() -> RepoRef {
        RepoRef {
            owner: "acme".to_string(),
            repo: "widget".to_string(),
        }
    }

    #[test]
    fn python_fixture_infers_runtime_and_entrypoint() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.py"), "print('hi')\n").unwrap();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"widget\"\nrequires-python = \">=3.10\"\n",
        )
        .unwrap();

        let repo = sample_repo();
        let sha = "a".repeat(40);
        let toml_str = infer_manifest(dir.path(), &repo, &sha).expect("should infer");
        let manifest = Manifest::from_toml_str(&toml_str).expect("generated manifest must parse");

        assert_eq!(manifest.language, Language::Python);
        assert_eq!(manifest.runtime, ">=3.10");
        assert_eq!(manifest.entrypoint, "src/main.py");
        assert_eq!(manifest.dependencies.manifest, "pyproject.toml");
        assert!(dir.path().join("harbor.toml").is_file());
    }

    #[test]
    fn node_fixture_infers_entrypoint_from_package_json_main() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/index.js"), "console.log(1)\n").unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name":"widget","main":"src/index.js","engines":{"node":">=20"}}"#,
        )
        .unwrap();

        let repo = sample_repo();
        let sha = "b".repeat(40);
        let toml_str = infer_manifest(dir.path(), &repo, &sha).expect("should infer");
        let manifest = Manifest::from_toml_str(&toml_str).expect("generated manifest must parse");

        assert_eq!(manifest.language, Language::Node);
        assert_eq!(manifest.entrypoint, "src/index.js");
        assert_eq!(manifest.runtime, ">=20");
        assert_eq!(manifest.dependencies.manifest, "package.json");
    }

    #[test]
    fn node_engines_with_unsupported_syntax_falls_back() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name":"widget","engines":{"node":"18.x"}}"#,
        )
        .unwrap();

        let repo = sample_repo();
        let sha = "c".repeat(40);
        let toml_str = infer_manifest(dir.path(), &repo, &sha).expect("should infer");
        let manifest = Manifest::from_toml_str(&toml_str).expect("generated manifest must parse");

        assert_eq!(manifest.runtime, ">=18");
    }

    #[test]
    fn missing_entrypoint_gets_placeholder_not_error() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "[project]\nname = \"widget\"\n").unwrap();

        let repo = sample_repo();
        let sha = "d".repeat(40);
        let toml_str = infer_manifest(dir.path(), &repo, &sha).expect("must not fail");
        let manifest = Manifest::from_toml_str(&toml_str).expect("generated manifest must parse");

        assert_eq!(manifest.entrypoint, "PLEASE_EDIT_entrypoint.py");
        assert_eq!(manifest.license, "PLEASE_EDIT");
    }

    #[test]
    fn generated_toml_round_trips_and_description_is_derivable() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        let repo = sample_repo();
        let sha = "e".repeat(40);
        let toml_str = infer_manifest(dir.path(), &repo, &sha).expect("should infer");
        let manifest = Manifest::from_toml_str(&toml_str).expect("must round-trip");

        assert_eq!(
            manifest.description,
            format!("Imported from https://github.com/acme/widget at {sha}")
        );
        assert_eq!(manifest.version, "0.0.0");
        assert_eq!(manifest.package_type, crate::manifest::PackageType::Agent);
    }

    #[test]
    fn existing_harbor_toml_is_preserved_verbatim() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "[project]\nname = \"widget\"\n").unwrap();
        let original = "spec-version = 1\nname = \"already-a-package\"\n# a repo that is already a harbor package\n";
        fs::write(dir.path().join("harbor.toml"), original).unwrap();

        let repo = sample_repo();
        let sha = "f".repeat(40);
        let toml_str = infer_manifest(dir.path(), &repo, &sha).expect("should return existing content");

        assert_eq!(toml_str, original);
        let on_disk = fs::read_to_string(dir.path().join("harbor.toml")).unwrap();
        assert_eq!(on_disk, original, "existing harbor.toml must not be overwritten");
    }

    // ---- Name derivation ----

    #[test]
    fn messy_repo_name_is_slugified_into_a_valid_name() {
        let name = derive_package_name("My_Repo.Name");
        assert!(crate::init::is_valid_package_name(&name), "got: {name:?}");
        assert_eq!(name, "my_repo-name");
    }

    #[test]
    fn hopeless_repo_name_falls_back_to_placeholder() {
        assert_eq!(derive_package_name("!!!"), "imported-package");
        assert_eq!(derive_package_name(".."), "imported-package");
        assert_eq!(derive_package_name(""), "imported-package");
    }

    // ---- Tarball prefix-strip + extraction ----

    fn set_raw_tar_path(header: &mut tar::Header, path: &str) {
        let gnu = header.as_gnu_mut().expect("built with Header::new_gnu()");
        for b in gnu.name.iter_mut() {
            *b = 0;
        }
        let bytes = path.as_bytes();
        let n = bytes.len().min(gnu.name.len());
        gnu.name[..n].copy_from_slice(&bytes[..n]);
    }

    fn write_test_tarball(path: &Path, entries: &[(&str, &[u8])]) {
        let f = fs::File::create(path).unwrap();
        let encoder = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (p, contents) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            set_raw_tar_path(&mut header, p);
            header.set_cksum();
            builder.append(&header, *contents).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn tarball_prefix_is_stripped_and_extraction_lands_in_dest() {
        let dir = tempdir().unwrap();
        let tarball_path = dir.path().join("repo.tar.gz");
        write_test_tarball(
            &tarball_path,
            &[
                ("widget-abc123/README.md", b"# hi" as &[u8]),
                ("widget-abc123/src/main.py", b"print(1)"),
            ],
        );

        let entries = read_github_tarball_entries(&tarball_path).expect("should read");
        let paths: Vec<&str> = entries.iter().map(|(p, _, _)| p.as_str()).collect();
        assert!(paths.contains(&"README.md"), "got: {paths:?}");
        assert!(paths.contains(&"src/main.py"), "got: {paths:?}");
        assert!(!paths.iter().any(|p| p.contains("widget-abc123")));

        let dest = dir.path().join("dest");
        let staging = dir.path().join("staging");
        extract::extract_entries(&entries, &dest, &staging).expect("should extract");

        assert!(dest.join("README.md").is_file());
        assert!(dest.join("src/main.py").is_file());
        assert_eq!(fs::read(dest.join("README.md")).unwrap(), b"# hi");
    }

    #[test]
    fn tarball_entry_with_traversal_after_strip_is_rejected() {
        let dir = tempdir().unwrap();
        let tarball_path = dir.path().join("evil.tar.gz");
        write_test_tarball(
            &tarball_path,
            &[("widget-abc123/../evil.txt", b"pwned" as &[u8])],
        );

        let err = read_github_tarball_entries(&tarball_path).unwrap_err();
        assert!(matches!(err, GithubError::UnsafeTarEntry { .. }), "got: {err:?}");
    }

    // ---- build_import: lock + archive (§12.2 steps 4-5) ----

    #[test]
    fn build_import_generates_lock_and_archive_containing_both() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.py"), "print('hi')\n").unwrap();
        fs::write(dir.path().join("pyproject.toml"), "[project]\nname = \"widget\"\n").unwrap();

        let repo = sample_repo();
        let sha = "1".repeat(40);
        infer_manifest(dir.path(), &repo, &sha).expect("infer should succeed");

        build_import(dir.path()).expect("build_import should succeed");

        assert!(dir.path().join("harbor.lock").is_file());
        let lock_str = fs::read_to_string(dir.path().join("harbor.lock")).unwrap();
        let lock = crate::lockfile::Lockfile::from_toml_str(&lock_str).expect("lock must parse");
        assert!(lock.package_checksum.is_some());

        let manifest_str = fs::read_to_string(dir.path().join("harbor.toml")).unwrap();
        let manifest = Manifest::from_toml_str(&manifest_str).unwrap();
        let archive_path = dir.path().join(format!("{}-{}.harbor", manifest.name, manifest.version));
        assert!(archive_path.is_file());

        let f = fs::File::open(&archive_path).unwrap();
        let decoder = flate2::read::GzDecoder::new(f);
        let mut archive = tar::Archive::new(decoder);
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"harbor.toml".to_string()), "got: {names:?}");
        assert!(names.contains(&"harbor.lock".to_string()), "got: {names:?}");
        assert!(names.contains(&"src/main.py".to_string()), "got: {names:?}");
    }

    #[test]
    fn build_import_errors_clearly_when_gitignore_excludes_harbor_toml() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "[project]\nname = \"widget\"\n").unwrap();

        let repo = sample_repo();
        let sha = "2".repeat(40);
        infer_manifest(dir.path(), &repo, &sha).expect("infer should succeed");

        // A hostile/incidental .gitignore that would swallow harbor.toml too.
        fs::write(dir.path().join(".gitignore"), "*.toml\n").unwrap();

        let err = build_import(dir.path()).unwrap_err();
        assert!(
            matches!(err, GithubError::ManifestExcludedByGitignore),
            "got: {err:?}"
        );
        assert!(!dir.path().join("harbor.lock").is_file(), "must not build a broken archive");
    }

    // ---- pure helpers ----

    #[test]
    fn commit_sha_validation() {
        assert!(is_valid_commit_sha(&"a".repeat(40)));
        assert!(is_valid_commit_sha("0123456789abcdef0123456789abcdef01234567"));
        assert!(!is_valid_commit_sha(&"A".repeat(40)), "must reject uppercase");
        assert!(!is_valid_commit_sha(&"a".repeat(39)), "must reject short shas");
        assert!(!is_valid_commit_sha(&"g".repeat(40)), "must reject non-hex");
    }
}
