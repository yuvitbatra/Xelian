//! GitHub import front end for `xelian add` (SPEC.md §12).
//!
//! GitHub repositories are an **import source**, not the canonical registry
//! (§12.1). This module implements §12.2: resolve a repository ref to a
//! commit SHA, download and cache the repository at that SHA, detect its
//! language, infer a `xelian.toml`, and build the `.xelian` package +
//! `xelian.lock`. It performs no publishing (§12.3).
//!
//! The individual inference concerns live in focused submodules:
//! [`url`] (parsing, including monorepo subdirectories), [`detect`]
//! (language), [`entrypoint`] (the runnable file), [`pkgtype`] (agent vs MCP),
//! and [`build`] (producing declared build outputs).
//!
//! All user-facing progress/status output goes to stderr: stdout is reserved
//! for a launched package's own output (MCP stdio transport), matching the
//! convention in `run/runtime.rs` and `run/model.rs`.

pub mod build;
pub mod detect;
pub mod discover;
pub mod entrypoint;
pub mod pkgtype;
pub mod url;

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use thiserror::Error;

use crate::cache::XelianHome;
use crate::errors::ManifestError;
use crate::lockfile::{self, LockfileError};
use crate::manifest::{Language, Manifest, PackageType};
use crate::package::{self, PackageError};
use crate::run::extract::{self, ExtractError};
use crate::run::runtime::{run_command_checked, NodeRuntimeManager, RuntimeError};

pub use detect::detect_language;
pub use url::{parse_github_url, RepoRef};

/// Errors that can occur while importing a GitHub repository (§12.2).
#[derive(Debug, Error)]
pub enum GithubError {
    /// Not a URL this command accepts, or a component is unsafe.
    #[error("invalid GitHub repository URL {url:?}: {reason}")]
    InvalidUrl { url: String, reason: String },

    /// `git` is required (to resolve a ref to a commit SHA) and was not found.
    #[error("{0} is required for `xelian add` but was not found (or is not executable) in PATH")]
    MissingDependency(String),

    /// `git ls-remote` failed outright. Boxed to keep `GithubError` small
    /// (clippy `result_large_err`) — `RuntimeError` carries full output capture.
    #[error("failed to resolve {reference} for {repo}: {source}")]
    ResolveHead {
        repo: String,
        reference: String,
        #[source]
        source: Box<RuntimeError>,
    },

    /// `git ls-remote` ran but returned no well-formed commit SHA.
    #[error("`git ls-remote` for {repo} returned no commit for {reference:?}")]
    UnexpectedHeadFormat { repo: String, reference: String },

    /// Downloading the repository tarball failed.
    #[error("failed to download {repo} at {sha}: {reason}")]
    DownloadFailed {
        repo: String,
        sha: String,
        reason: String,
    },

    /// A tarball entry's path is unsafe after prefix stripping.
    #[error(
        "GitHub archive entry {path:?} has an unsafe path after stripping the top-level \
         directory and was rejected"
    )]
    UnsafeTarEntry { path: String },

    /// The URL named a subdirectory that does not exist at that commit.
    #[error("subdirectory {subdir:?} does not exist in {repo} at {sha}")]
    SubdirNotFound {
        repo: String,
        subdir: String,
        sha: String,
    },

    #[error(transparent)]
    Extract(#[from] ExtractError),

    /// A language Xelian recognizes but has no runtime manager for in V1 (§22).
    #[error(
        "unsupported language ({language}): Xelian v1 runs Python and Node packages only.\n\
         This repository cannot be imported until a {language} runtime manager exists."
    )]
    UnsupportedLanguage { language: String },

    /// No recognized project manifest at the package root.
    #[error(
        "could not detect project language in {path}\n\
         Looked for: pyproject.toml, setup.py, requirements.txt (Python); package.json (Node).\n\
         If this is a monorepo, point `xelian add` at the subdirectory that holds the package, \
         e.g. https://github.com/<owner>/<repo>/tree/main/<subdir>"
    )]
    UndetectedLanguage { path: String },

    /// The repository ships only a Dockerfile / docker-compose (no Python or
    /// Node manifest). Xelian's Python/Node runtimes can't run it, but Docker
    /// can — so point the user at that rather than a generic "can't detect".
    #[error(
        "this project runs via Docker (it ships a Dockerfile, no Python/Node package).\n\
         It's cached at {path}\n\
         Run it with Docker directly:\n  \
         docker build -t my-agent {path} && docker run -it my-agent\n\
         (A built-in Docker runtime for `xelian run` is on the roadmap.)"
    )]
    DockerOnly { path: String },

    /// No runnable entrypoint could be inferred. Reported *before* dependency
    /// installation so the user is not made to wait several minutes for a
    /// failure that was already knowable (§12.2 step 3).
    #[error(
        "could not determine how to run this package.\n\n\
         Imported and cached at:\n  {path}\n\n\
         Xelian inferred everything except `entrypoint`. To finish the import:\n  \
         1. edit {path}/xelian.toml and set `entrypoint` to the file that starts the program\n  \
         2. re-run the same `xelian add` command\n\n\
         (Some repositories are libraries with no runnable entrypoint at all — \
         those cannot be run by Xelian.)"
    )]
    NoEntrypoint { path: String },

    /// The URL named a monorepo root holding several runnable packages, so
    /// there is no single right answer. Lists the exact subdirectory commands.
    #[error(
        "{repo} is a monorepo containing several runnable packages — \
         pick the one you want:\n{choices}"
    )]
    AmbiguousMonorepo { repo: String, choices: String },

    /// The declared entrypoint is a build output and the build did not produce it.
    #[error(
        "the build completed but did not produce the expected entrypoint {entrypoint:?}\n\
         Package is cached at {path} — check its build script, then set `entrypoint` \
         in xelian.toml to the file the build actually emits."
    )]
    EntrypointNotBuilt { entrypoint: String, path: String },

    /// `npm run build` failed.
    #[error("the package's build step failed: {source}")]
    Build {
        #[source]
        source: Box<RuntimeError>,
    },

    #[error(transparent)]
    ManifestParse(#[from] ManifestError),

    #[error(transparent)]
    Package(#[from] PackageError),

    #[error(transparent)]
    Lockfile(#[from] LockfileError),

    /// The repository's `.gitignore` would exclude `xelian.toml` from the
    /// package file set.
    #[error(
        "xelian.toml would be excluded from the package archive by .gitignore; cannot build \
         a valid import — remove or adjust the .gitignore rule that excludes it"
    )]
    ManifestExcludedByGitignore,

    #[error("I/O error at {path}: {source}", path = path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Whether `name` is available and executable.
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

/// Download `url` to `dest`, writing through a temporary file so an
/// interrupted transfer never leaves a truncated tarball at the final path.
///
/// Uses the HTTP client Xelian already links rather than shelling out to
/// `curl`. `curl` is present on most machines but not all — and a hard
/// dependency on an external binary contradicts the single-static-binary,
/// zero-setup promise. `git` remains required, since resolving a ref to a
/// commit SHA genuinely needs it.
fn download_to_file(url: &str, dest: &Path) -> Result<(), String> {
    let response = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(30))
        .build()
        .get(url)
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => match code {
                404 => "not found (404) — check the repository and ref exist and are public".into(),
                403 => "forbidden (403) — the repository may be private or rate-limited".into(),
                other => format!("server returned HTTP {other}"),
            },
            ureq::Error::Transport(t) => format!("network error: {t}"),
        })?;

    let tmp = dest.with_extension("part");
    let mut file = fs::File::create(&tmp).map_err(|e| format!("cannot create {tmp:?}: {e}"))?;
    let copied = io::copy(&mut response.into_reader(), &mut file);

    match copied {
        Ok(_) => {
            drop(file);
            fs::rename(&tmp, dest).map_err(|e| format!("cannot finalize download: {e}"))
        }
        Err(e) => {
            drop(file);
            let _ = fs::remove_file(&tmp);
            Err(format!("download interrupted: {e}"))
        }
    }
}

/// Whether `s` is exactly 40 lowercase hex characters (a full commit SHA-1).
fn is_valid_commit_sha(s: &str) -> bool {
    s.len() == 40
        && s.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Resolve a repository ref to a commit SHA (SPEC.md §12.2 step 1, H-110) via
/// `git ls-remote` — no GitHub API call, so no rate limits.
///
/// A `/tree/<ref>/...` URL resolves that ref; a plain URL resolves `HEAD`.
pub fn resolve_head_sha(repo: &RepoRef) -> Result<String, GithubError> {
    ensure_binary_available("git")?;

    let remote = format!("https://github.com/{}/{}.git", repo.owner, repo.repo);
    let reference = repo.git_ref.as_deref().unwrap_or("HEAD");

    let mut cmd = Command::new("git");
    cmd.arg("ls-remote").arg(&remote).arg(reference);
    let output = run_command_checked(&mut cmd).map_err(|e| GithubError::ResolveHead {
        repo: repo.label(),
        reference: reference.to_string(),
        source: Box::new(e),
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    pick_sha(&stdout, reference).ok_or_else(|| GithubError::UnexpectedHeadFormat {
        repo: repo.label(),
        reference: reference.to_string(),
    })
}

/// Choose the commit SHA from `git ls-remote` output.
///
/// `ls-remote <ref>` can return several lines (a branch and a tag of the same
/// name), so prefer an exact branch match, then a tag, then any valid row.
fn pick_sha(stdout: &str, reference: &str) -> Option<String> {
    let rows: Vec<(&str, &str)> = stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let sha = parts.next()?;
            let name = parts.next().unwrap_or("");
            is_valid_commit_sha(sha).then_some((sha, name))
        })
        .collect();

    for want in [
        format!("refs/heads/{reference}"),
        format!("refs/tags/{reference}"),
    ] {
        if let Some((sha, _)) = rows.iter().find(|(_, name)| *name == want) {
            return Some((*sha).to_string());
        }
    }
    rows.first().map(|(sha, _)| (*sha).to_string())
}

/// Whether a cached import is complete and reusable.
///
/// Gated on the *artifacts of a finished import*, not on directory
/// non-emptiness: a previous run that downloaded the repository and then
/// failed (unknown language, no entrypoint) leaves a populated directory
/// behind. Treating that as a cache hit made the retry skip inference
/// entirely and fail with a worse error than the first attempt.
fn cached_import_is_complete(dir: &Path) -> bool {
    dir.join("xelian.toml").is_file() && dir.join("xelian.lock").is_file()
}

/// Strip the top-level directory GitHub tarballs wrap entries in
/// (`<repo>-<sha>/...`), plus `subdir/` when importing a monorepo package.
/// Returns `None` for entries outside the requested subdirectory.
fn strip_prefix_components(path: &str, subdir: Option<&str>) -> Option<String> {
    let rest = match path.split_once('/') {
        Some((_, rest)) if !rest.is_empty() => rest,
        _ => return None,
    };
    match subdir {
        None => Some(rest.to_string()),
        Some(sub) => rest
            .strip_prefix(&format!("{sub}/"))
            .filter(|r| !r.is_empty())
            .map(str::to_string),
    }
}

/// Read a downloaded GitHub tarball into the `(archive-relative path,
/// contents, tar mode bits)` triples [`extract::extract_entries`] expects.
///
/// Symlink entries are skipped with a warning — an archive symlink extracted
/// into a shared cache directory is a traversal hazard.
fn read_github_tarball_entries(
    tarball_path: &Path,
    subdir: Option<&str>,
) -> Result<Vec<(String, Vec<u8>, u32)>, GithubError> {
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

        let Some(stripped) = strip_prefix_components(&raw_path, subdir) else {
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
                // Implicit: extract_entries creates parents from file paths.
            }
            tar::EntryType::Symlink => {
                eprintln!("warning: skipping symlink entry in GitHub archive: {stripped}");
            }
            _ => {}
        }
    }

    Ok(out)
}

/// Download and extract a repository at a specific commit SHA, caching it at
/// `home.github_package_dir(owner, repo, <cache key>)`.
///
/// The `bool` in the return value is `true` when a complete previous import
/// was reused.
pub fn fetch_repo(
    repo: &RepoRef,
    sha: &str,
    home: &XelianHome,
) -> Result<(PathBuf, bool), GithubError> {
    // The repository is always extracted whole, addressed by commit SHA, and
    // the package directory is a path *inside* it.
    //
    // Extracting only the subdirectory would be smaller, but breaks monorepo
    // builds: a subpackage's `tsconfig.json` routinely says
    // `"extends": "../../tsconfig.json"`, and its build fails outright if the
    // repository around it is missing. Keeping the whole checkout also means
    // sibling subpackages at the same SHA share one download.
    let repo_root = home.github_package_dir(&repo.owner, &repo.repo, sha);
    let package_dir = match &repo.subdir {
        Some(subdir) => repo_root.join(subdir),
        None => repo_root.clone(),
    };

    if cached_import_is_complete(&package_dir) {
        eprintln!("(cached) {} @ {sha}", repo.label());
        return Ok((package_dir, true));
    }

    // The checkout may already be present from a sibling subpackage import at
    // this same SHA — reuse it rather than re-downloading.
    if repo_root.is_dir() && dir_is_nonempty(&repo_root) {
        if repo.subdir.is_some() && !package_dir.is_dir() {
            return Err(GithubError::SubdirNotFound {
                repo: format!("{}/{}", repo.owner, repo.repo),
                subdir: repo.subdir.clone().unwrap_or_default(),
                sha: sha.to_string(),
            });
        }
        return Ok((package_dir, false));
    }

    fs::create_dir_all(home.tmp()).map_err(|e| GithubError::Io {
        path: home.tmp(),
        source: e,
    })?;

    let tarball_path = home
        .tmp()
        .join(format!("{}-{}-{sha}.tar.gz", repo.owner, repo.repo));
    let dl_url = format!(
        "https://codeload.github.com/{}/{}/tar.gz/{}",
        repo.owner, repo.repo, sha
    );

    eprintln!("Downloading {} @ {sha}...", repo.label());
    download_to_file(&dl_url, &tarball_path).map_err(|reason| GithubError::DownloadFailed {
        repo: repo.label(),
        sha: sha.to_string(),
        reason,
    })?;

    let result = (|| -> Result<(), GithubError> {
        let entries = read_github_tarball_entries(&tarball_path, None)?;
        extract::extract_entries(&entries, &repo_root, &home.tmp())?;
        Ok(())
    })();

    let _ = fs::remove_file(&tarball_path);
    result?;

    if repo.subdir.is_some() && !package_dir.is_dir() {
        return Err(GithubError::SubdirNotFound {
            repo: format!("{}/{}", repo.owner, repo.repo),
            subdir: repo.subdir.clone().unwrap_or_default(),
            sha: sha.to_string(),
        });
    }

    Ok((package_dir, false))
}

/// Whether `dir` exists and contains at least one entry.
fn dir_is_nonempty(dir: &Path) -> bool {
    fs::read_dir(dir)
        .map(|mut rd| rd.next().is_some())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Manifest inference (§12.2 step 3)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PyProjectToml {
    project: Option<PyProjectProjectTable>,
}

#[derive(Deserialize)]
struct PyProjectProjectTable {
    #[serde(rename = "requires-python")]
    requires_python: Option<String>,
}

/// `requires-python` if present and parseable, else `">=3.9"`.
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

#[derive(Deserialize)]
struct PackageJsonEnginesOnly {
    engines: Option<PackageJsonEngines>,
}

#[derive(Deserialize)]
struct PackageJsonEngines {
    node: Option<String>,
}

/// `engines.node` if syntactically supported by the constraint parser, else `">=18"`.
fn infer_node_runtime(checkout: &Path) -> String {
    let parsed = fs::read_to_string(checkout.join("package.json"))
        .ok()
        .and_then(|c| serde_json::from_str::<PackageJsonEnginesOnly>(&c).ok());

    let Some(constraint) = parsed.and_then(|p| p.engines).and_then(|e| e.node) else {
        return ">=18".to_string();
    };
    if constraint.trim().is_empty() {
        return ">=18".to_string();
    }
    if NodeRuntimeManager
        .validate_constraint_syntax(&constraint)
        .is_ok()
    {
        constraint
    } else {
        ">=18".to_string()
    }
}

/// Slugify into the SPEC.md §19.3 name charset.
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

/// Derive a package `name` (SPEC.md §19.3). Names below the 3-character
/// minimum are padded rather than discarded, so `.../tree/main/src/db` keeps
/// a meaningful name instead of collapsing to `imported-package`.
fn derive_package_name(basis: &str) -> String {
    let candidate = slugify_name(basis);
    if crate::init::is_valid_package_name(&candidate) {
        return candidate;
    }
    if !candidate.is_empty() && candidate.len() < 3 {
        let padded = format!("{candidate}-pkg");
        if crate::init::is_valid_package_name(&padded) {
            return padded;
        }
    }
    "imported-package".to_string()
}

/// Escape a value for a TOML basic string.
fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[allow(clippy::too_many_arguments)]
fn render_manifest_toml(
    name: &str,
    description: &str,
    package_type: PackageType,
    language: Language,
    runtime: &str,
    entrypoint: &str,
    dep_manifest: &str,
    dep_lockfile: Option<&str>,
) -> String {
    use std::fmt::Write as _;

    let (description, runtime, entrypoint) = (
        toml_escape(description),
        toml_escape(runtime),
        toml_escape(entrypoint),
    );

    let mut s = String::new();
    let _ = writeln!(s, "spec-version = 1");
    let _ = writeln!(s, "name = \"{name}\"");
    let _ = writeln!(s, "version = \"0.0.0\"");
    let _ = writeln!(s, "description = \"{description}\"");
    let _ = writeln!(s, "package-type = \"{package_type}\"");
    let _ = writeln!(s, "language = \"{}\"", detect::language_label(language));
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

/// The result of inferring (or reusing) a manifest.
#[derive(Debug)]
pub struct InferredManifest {
    pub toml: String,
    /// `true` when the entrypoint is a declared build output that does not
    /// exist yet, so a build must run before launch.
    pub needs_build: bool,
}

/// Infer and write a `xelian.toml` into `checkout`, or return an existing one
/// verbatim (SPEC.md §12.2 step 3, H-112).
///
/// Fields that cannot be mechanically derived get `PLEASE_EDIT` placeholders;
/// this function MUST NOT fail merely because such fields are placeholders.
/// It *does* fail when no entrypoint can be inferred, because that is the one
/// field without which the package can never run — and failing here, before
/// dependency installation, saves the user a multi-minute wait for a failure
/// that was already knowable.
pub fn infer_manifest(
    checkout: &Path,
    repo: &RepoRef,
    sha: &str,
) -> Result<InferredManifest, GithubError> {
    let manifest_path = checkout.join("xelian.toml");
    if manifest_path.is_file() {
        eprintln!(
            "{} already contains a xelian.toml; keeping it as-is",
            repo.label()
        );
        let toml = fs::read_to_string(&manifest_path).map_err(|e| GithubError::Io {
            path: manifest_path.clone(),
            source: e,
        })?;
        let needs_build = Manifest::from_toml_str(&toml)
            .ok()
            .is_some_and(|m| !checkout.join(&m.entrypoint).is_file());
        return Ok(InferredManifest { toml, needs_build });
    }

    let language = detect_language(checkout).map_err(|e| match e {
        GithubError::UndetectedLanguage { .. } => GithubError::UndetectedLanguage {
            path: checkout.display().to_string(),
        },
        other => other,
    })?;
    eprintln!("Detected language: {}", detect::language_label(language));

    let found = entrypoint::infer(checkout, language, repo.package_basis()).ok_or_else(|| {
        // No runnable Python/Node entrypoint. If the repo ships a Dockerfile
        // (e.g. OpenHands: a Python project that actually runs via Docker),
        // point the user at Docker rather than a generic "set entrypoint".
        if checkout.join("Dockerfile").is_file() || checkout.join("docker-compose.yml").is_file() {
            GithubError::DockerOnly {
                path: checkout.display().to_string(),
            }
        } else {
            GithubError::NoEntrypoint {
                path: checkout.display().to_string(),
            }
        }
    })?;

    // A generated launcher (console-script-only projects) must exist on disk
    // before the archive is built and before launch resolves the entrypoint.
    if let Some(source) = &found.shim {
        let launcher = checkout.join(&found.path);
        fs::write(&launcher, source).map_err(|e| GithubError::Io {
            path: launcher.clone(),
            source: e,
        })?;
        eprintln!(
            "Generated {} — this project runs via its console script",
            found.path
        );
    }

    let package_type = pkgtype::infer(checkout, language);

    let (runtime, dep_manifest, dep_lockfile): (String, &str, Option<&str>) = match language {
        Language::Python => {
            let lockfile = checkout.join("uv.lock").is_file().then_some("uv.lock");
            // A repo with only requirements.txt has no pyproject to install from.
            let dep_manifest = if checkout.join("pyproject.toml").is_file() {
                "pyproject.toml"
            } else {
                "requirements.txt"
            };
            (infer_python_runtime(checkout), dep_manifest, lockfile)
        }
        Language::Node => {
            let lockfile = checkout
                .join("package-lock.json")
                .is_file()
                .then_some("package-lock.json");
            (infer_node_runtime(checkout), "package.json", lockfile)
        }
    };

    let name = derive_package_name(repo.package_basis());
    let description = format!(
        "Imported from https://github.com/{}/{} at {sha}",
        repo.owner, repo.repo
    );

    let manifest_toml = render_manifest_toml(
        &name,
        &description,
        package_type,
        language,
        &runtime,
        &found.path,
        dep_manifest,
        dep_lockfile,
    );

    // The generated manifest MUST parse before it is written — a failure here
    // is our bug, not the imported repo's.
    let manifest = Manifest::from_toml_str(&manifest_toml)?;
    eprintln!(
        "Inferred {} entrypoint: {}{}",
        manifest.package_type,
        manifest.entrypoint,
        if found.exists {
            ""
        } else {
            " (produced by the package's build step)"
        }
    );

    fs::write(&manifest_path, &manifest_toml).map_err(|e| GithubError::Io {
        path: manifest_path.clone(),
        source: e,
    })?;

    Ok(InferredManifest {
        toml: manifest_toml,
        // A generated launcher was just written, so it exists now; only a
        // real build output still needs a build step.
        needs_build: !found.exists && found.shim.is_none(),
    })
}

/// Generate `xelian.lock` and build the `.xelian` archive (§12.2 steps 4–5).
///
/// Deliberately does not use [`crate::validate::validate_and_build`]: that
/// pipeline enforces §8.1 publish-time rules (README/LICENSE present, etc.)
/// an arbitrary imported repository may not satisfy, and imported packages
/// are local-only until an explicit `xelian push` (§12.3) re-runs full
/// validation.
pub fn build_import(checkout: &Path) -> Result<(), GithubError> {
    let manifest_path = checkout.join("xelian.toml");
    let manifest_str = fs::read_to_string(&manifest_path).map_err(|e| GithubError::Io {
        path: manifest_path.clone(),
        source: e,
    })?;
    let manifest = Manifest::from_toml_str(&manifest_str)?;

    let mut files = package::collect_files(checkout)?;
    if !files.iter().any(|(p, _)| p == "xelian.toml") {
        return Err(GithubError::ManifestExcludedByGitignore);
    }

    let lock = lockfile::generate(&manifest, checkout, &files)?;
    let lock_toml = lock.to_toml_string()?;
    let lock_path = checkout.join("xelian.lock");
    fs::write(&lock_path, &lock_toml).map_err(|e| GithubError::Io {
        path: lock_path.clone(),
        source: e,
    })?;

    // Force-include the freshly written lock so .gitignore cannot drop it.
    files.retain(|(p, _)| p != "xelian.lock");
    files.push(("xelian.lock".to_string(), lock_path.clone()));
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

    let archive_name = format!("{}-{}.xelian", manifest.name, manifest.version);
    let out_path = checkout.join(&archive_name);
    let tmp_path = out_path.with_extension("xelian.tmp");

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
    /// `true` if this exact import had already completed previously.
    pub from_cache: bool,
    /// `true` when the entrypoint must be produced by a build step before launch.
    pub needs_build: bool,
}

/// Import a GitHub repository as a local Xelian package (SPEC.md §12.2 steps 1–5).
///
/// Does not publish (§12.3) and does not run the package — running it is the
/// caller's responsibility.
pub fn import_github(url_str: &str, home: &XelianHome) -> Result<ImportOutcome, GithubError> {
    let mut repo = parse_github_url(url_str)?;

    eprintln!("Resolving {}...", repo.label());
    let sha = resolve_head_sha(&repo)?;
    eprintln!("Resolved to commit {sha}");

    let (mut checkout, was_cached) = fetch_repo(&repo, &sha, home)?;

    if was_cached {
        // A complete cached import: manifest and lock already exist. The
        // entrypoint may still be a build output that was cleaned since.
        let needs_build = fs::read_to_string(checkout.join("xelian.toml"))
            .ok()
            .and_then(|s| Manifest::from_toml_str(&s).ok())
            .is_some_and(|m| !checkout.join(&m.entrypoint).is_file());
        return Ok(ImportOutcome {
            repo,
            sha,
            package_dir: checkout,
            from_cache: true,
            needs_build,
        });
    }

    // Monorepo root: no runnable package here, but member packages inside.
    // Descend into the single obvious one, or list the choices (§12.2).
    if repo.subdir.is_none() && !checkout.join("xelian.toml").is_file() {
        maybe_descend_into_subpackage(&mut repo, &mut checkout, &sha, home)?;
    }

    let inferred = infer_manifest(&checkout, &repo, &sha)?;
    build_import(&checkout)?;
    eprintln!("Cached at {}", checkout.display());

    Ok(ImportOutcome {
        repo,
        sha,
        package_dir: checkout,
        from_cache: false,
        needs_build: inferred.needs_build,
    })
}

/// When a repository root has no runnable package of its own, look for member
/// packages inside it (a monorepo). Descend into the single unambiguous one by
/// re-pointing `repo`/`checkout` at its subdirectory; if the choice is
/// genuinely ambiguous, fail with the exact `xelian add` commands to pick
/// from. A root that is simply not a monorepo is left untouched, so normal
/// inference (and its own clear error) proceeds.
fn maybe_descend_into_subpackage(
    repo: &mut RepoRef,
    checkout: &mut PathBuf,
    sha: &str,
    home: &XelianHome,
) -> Result<(), GithubError> {
    // Only bother when the root itself yields no entrypoint — a runnable root
    // is never overridden by something nested.
    if let Ok(lang) = detect_language(checkout) {
        if entrypoint::infer(checkout, lang, repo.package_basis()).is_some() {
            return Ok(());
        }
    }

    let subpackages = discover::find_subpackages(checkout);
    if subpackages.is_empty() {
        return Ok(());
    }

    if let Some(chosen) = discover::pick_unambiguous(&subpackages) {
        eprintln!(
            "{} is a monorepo; using its {} package: {}",
            repo.label(),
            chosen.package_type,
            chosen.subdir
        );
        repo.subdir = Some(chosen.subdir.clone());
        *checkout = home
            .github_package_dir(&repo.owner, &repo.repo, sha)
            .join(&chosen.subdir);
        return Ok(());
    }

    // Ambiguous: hand the user the exact commands rather than guessing.
    let mut lines = String::new();
    for s in &subpackages {
        lines.push_str(&format!(
            "\n  xelian add https://github.com/{}/{}/tree/{}/{}   # {}",
            repo.owner,
            repo.repo,
            repo.git_ref.as_deref().unwrap_or("HEAD"),
            s.subdir,
            s.package_type
        ));
    }
    Err(GithubError::AmbiguousMonorepo {
        repo: repo.label(),
        choices: lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_repo() -> RepoRef {
        RepoRef {
            owner: "acme".to_string(),
            repo: "widget".to_string(),
            subdir: None,
            git_ref: None,
        }
    }

    // ---- SHA selection ----

    #[test]
    fn commit_sha_validation() {
        assert!(is_valid_commit_sha(&"a".repeat(40)));
        assert!(is_valid_commit_sha(
            "0123456789abcdef0123456789abcdef01234567"
        ));
        assert!(!is_valid_commit_sha(&"A".repeat(40)), "rejects uppercase");
        assert!(!is_valid_commit_sha(&"a".repeat(39)), "rejects short");
        assert!(!is_valid_commit_sha(&"g".repeat(40)), "rejects non-hex");
    }

    #[test]
    fn pick_sha_prefers_the_branch_over_a_same_named_tag() {
        let sha_tag = "1".repeat(40);
        let sha_branch = "2".repeat(40);
        let out = format!("{sha_tag}\trefs/tags/main\n{sha_branch}\trefs/heads/main\n");
        assert_eq!(pick_sha(&out, "main").unwrap(), sha_branch);
    }

    #[test]
    fn pick_sha_handles_bare_head_output() {
        let sha = "3".repeat(40);
        assert_eq!(pick_sha(&format!("{sha}\tHEAD\n"), "HEAD").unwrap(), sha);
    }

    #[test]
    fn pick_sha_rejects_garbage() {
        assert_eq!(pick_sha("not a sha\n", "HEAD"), None);
        assert_eq!(pick_sha("", "HEAD"), None);
    }

    // ---- tarball prefix stripping ----

    #[test]
    fn strips_the_github_top_level_directory() {
        assert_eq!(
            strip_prefix_components("widget-abc/README.md", None).as_deref(),
            Some("README.md")
        );
        assert_eq!(strip_prefix_components("widget-abc/", None), None);
    }

    #[test]
    fn strips_subdir_and_filters_everything_outside_it() {
        assert_eq!(
            strip_prefix_components("servers-abc/src/github/index.ts", Some("src/github"))
                .as_deref(),
            Some("index.ts")
        );
        assert_eq!(
            strip_prefix_components("servers-abc/src/redis/index.ts", Some("src/github")),
            None,
            "entries outside the subdir must be dropped"
        );
        assert_eq!(
            strip_prefix_components("servers-abc/README.md", Some("src/github")),
            None
        );
    }

    // ---- name derivation ----

    #[test]
    fn messy_repo_name_is_slugified() {
        let name = derive_package_name("My_Repo.Name");
        assert!(crate::init::is_valid_package_name(&name), "got: {name:?}");
        assert_eq!(name, "my_repo-name");
    }

    #[test]
    fn short_names_are_padded_rather_than_discarded() {
        assert_eq!(derive_package_name("db"), "db-pkg");
    }

    #[test]
    fn hopeless_names_fall_back_to_placeholder() {
        assert_eq!(derive_package_name("!!!"), "imported-package");
        assert_eq!(derive_package_name(""), "imported-package");
    }

    // ---- cache completeness (regression) ----

    #[test]
    fn a_downloaded_but_unfinished_import_is_not_a_cache_hit() {
        // A failed import leaves a populated directory. Reusing it made the
        // retry skip inference and fail worse than the first attempt.
        let d = tempdir().unwrap();
        fs::write(d.path().join("README.md"), "downloaded, never inferred").unwrap();
        assert!(!cached_import_is_complete(d.path()));

        fs::write(d.path().join("xelian.toml"), "x").unwrap();
        assert!(!cached_import_is_complete(d.path()), "lock still missing");

        fs::write(d.path().join("xelian.lock"), "y").unwrap();
        assert!(cached_import_is_complete(d.path()));
    }

    // ---- manifest inference ----

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

        let sha = "a".repeat(40);
        let out = infer_manifest(dir.path(), &sample_repo(), &sha).unwrap();
        let manifest = Manifest::from_toml_str(&out.toml).unwrap();

        assert_eq!(manifest.language, Language::Python);
        assert_eq!(manifest.runtime, ">=3.10");
        assert_eq!(manifest.entrypoint, "src/main.py");
        assert_eq!(manifest.dependencies.manifest, "pyproject.toml");
        assert!(!out.needs_build);
    }

    #[test]
    fn requirements_only_project_installs_from_requirements_txt() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("requirements.txt"), "flask\n").unwrap();
        fs::write(dir.path().join("main.py"), "print(1)\n").unwrap();

        let sha = "b".repeat(40);
        let out = infer_manifest(dir.path(), &sample_repo(), &sha).unwrap();
        let manifest = Manifest::from_toml_str(&out.toml).unwrap();

        assert_eq!(manifest.dependencies.manifest, "requirements.txt");
        assert_eq!(manifest.entrypoint, "main.py");
    }

    #[test]
    fn mcp_servers_are_typed_mcp_not_agent() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name":"thing","main":"index.js","dependencies":{"@modelcontextprotocol/sdk":"^1"}}"#,
        )
        .unwrap();
        fs::write(dir.path().join("index.js"), "1\n").unwrap();

        let sha = "c".repeat(40);
        let out = infer_manifest(dir.path(), &sample_repo(), &sha).unwrap();
        let manifest = Manifest::from_toml_str(&out.toml).unwrap();
        assert_eq!(manifest.package_type, PackageType::Mcp);
    }

    #[test]
    fn build_output_entrypoint_is_flagged_needs_build() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/index.ts"), "export {}\n").unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"name":"x","main":"dist/index.js","scripts":{"build":"tsc"}}"#,
        )
        .unwrap();

        let sha = "d".repeat(40);
        let out = infer_manifest(dir.path(), &sample_repo(), &sha).unwrap();
        assert!(out.needs_build);
        let manifest = Manifest::from_toml_str(&out.toml).unwrap();
        assert_eq!(manifest.entrypoint, "dist/index.js");
    }

    #[test]
    fn library_without_an_entrypoint_fails_with_actionable_guidance() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"swarm\"\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("swarm")).unwrap();
        fs::write(dir.path().join("swarm/__init__.py"), "").unwrap();

        let sha = "e".repeat(40);
        let err = infer_manifest(dir.path(), &sample_repo(), &sha).unwrap_err();
        match &err {
            GithubError::NoEntrypoint { .. } => {
                let msg = err.to_string();
                assert!(msg.contains("xelian.toml"), "must name the file to edit");
                assert!(msg.contains("entrypoint"), "must name the field to set");
            }
            other => panic!("expected NoEntrypoint, got {other:?}"),
        }
    }

    #[test]
    fn existing_xelian_toml_is_preserved_verbatim() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"widget\"\n",
        )
        .unwrap();
        let original = "spec-version = 1\nname = \"already-a-package\"\n# hand written\n";
        fs::write(dir.path().join("xelian.toml"), original).unwrap();

        let sha = "f".repeat(40);
        let out = infer_manifest(dir.path(), &sample_repo(), &sha).unwrap();
        assert_eq!(out.toml, original);
        assert_eq!(
            fs::read_to_string(dir.path().join("xelian.toml")).unwrap(),
            original
        );
    }

    #[test]
    fn unsupported_language_is_distinct_from_undetected() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("go.mod"), "module x\n").unwrap();
        let sha = "1".repeat(40);
        match infer_manifest(dir.path(), &sample_repo(), &sha).unwrap_err() {
            GithubError::UnsupportedLanguage { language } => assert_eq!(language, "go"),
            other => panic!("expected UnsupportedLanguage, got {other:?}"),
        }
    }

    // ---- build_import ----

    #[test]
    fn build_import_generates_lock_and_archive_containing_both() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.py"), "print('hi')\n").unwrap();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"widget\"\n",
        )
        .unwrap();

        let sha = "1".repeat(40);
        infer_manifest(dir.path(), &sample_repo(), &sha).unwrap();
        build_import(dir.path()).unwrap();

        assert!(dir.path().join("xelian.lock").is_file());
        let lock_str = fs::read_to_string(dir.path().join("xelian.lock")).unwrap();
        let lock = crate::lockfile::Lockfile::from_toml_str(&lock_str).unwrap();
        assert!(lock.package_checksum.is_some());

        let manifest_str = fs::read_to_string(dir.path().join("xelian.toml")).unwrap();
        let manifest = Manifest::from_toml_str(&manifest_str).unwrap();
        let archive_path = dir
            .path()
            .join(format!("{}-{}.xelian", manifest.name, manifest.version));
        assert!(archive_path.is_file());

        let f = fs::File::open(&archive_path).unwrap();
        let decoder = flate2::read::GzDecoder::new(f);
        let mut archive = tar::Archive::new(decoder);
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"xelian.toml".to_string()), "got: {names:?}");
        assert!(names.contains(&"xelian.lock".to_string()), "got: {names:?}");
        assert!(names.contains(&"src/main.py".to_string()), "got: {names:?}");
    }

    #[test]
    fn build_import_errors_clearly_when_gitignore_excludes_xelian_toml() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"widget\"\n",
        )
        .unwrap();
        fs::write(dir.path().join("main.py"), "print(1)\n").unwrap();

        let sha = "2".repeat(40);
        infer_manifest(dir.path(), &sample_repo(), &sha).unwrap();
        fs::write(dir.path().join(".gitignore"), "*.toml\n").unwrap();

        let err = build_import(dir.path()).unwrap_err();
        assert!(
            matches!(err, GithubError::ManifestExcludedByGitignore),
            "got: {err:?}"
        );
        assert!(!dir.path().join("xelian.lock").is_file());
    }
}
