//! Local cache layout for `~/.xelian/` (SPEC.md §11).
//!
//! `XelianHome` resolves and manages the on-disk layout Xelian uses to cache
//! downloaded packages, isolated environments, managed runtimes, models, logs,
//! and scratch space. See §11.1 for the directory structure and §11.3 for the
//! rules around `credentials.toml` isolation.
//!
//! ## Cache layout / namespacing (decision 2026-07-16)
//!
//! Contents under `packages/` and `envs/` are namespaced by **source**, not
//! just `(owner, name, version)`, so that packages pulled from different
//! origins never collide on disk even if they share a name/version:
//!
//! - `registry/<owner>/<name>/<version>/` — packages pulled from the public
//!   Xelian registry.
//! - `github/<owner>/<repo>/<sha>/` — packages imported via `xelian add
//!   <github-url>` (§12), addressed by commit SHA for reproducibility.
//! - `local/<name>/<version>/` — packages built/run from a local directory
//!   (no owner namespace, since there's no remote source to disambiguate).
//!
//! This module implements helpers for the `local/` namespace
//! (`local_package_dir`, `local_env_dir`, `local_grants_path`) and the
//! `github/` namespace (`github_package_dir`, `github_env_dir`,
//! `github_grants_path`, used by `xelian add`, §12.2); a later task that
//! implements registry pulls will add the `registry/` equivalents following
//! the same shape.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Describes a cached package's origin (SPEC.md §11.1 source-based layout).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageSource {
    /// `packages/local/<name>/<version>/` — built/run from a local directory.
    Local,
    /// `packages/github/<owner>/<repo>/<sha>/` — imported via `xelian add`.
    Github { owner: String, repo: String },
    /// `packages/registry/<owner>/<name>/<version>/` — pulled from the registry.
    Registry { owner: String },
}

/// A single cached package version found under `~/.xelian/packages/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedPackage {
    pub source: PackageSource,
    pub name: String,
    pub version: String,
    /// Full path to the extracted package directory on disk.
    pub path: PathBuf,
    /// Corresponding environment directory, if it exists.
    pub env_path: Option<PathBuf>,
}

/// Outcome of removing cached packages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveOutcome {
    pub removed_packages: Vec<PathBuf>,
    pub removed_envs: Vec<PathBuf>,
}

/// Errors that can occur while resolving a [`XelianHome`].
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// No home directory could be determined for the current user/OS, so
    /// `~/.xelian` cannot be resolved. See [`dirs::home_dir`] for the
    /// platform-specific conditions under which this happens (e.g. `HOME`
    /// unset on Unix).
    #[error(
        "could not determine the current user's home directory; \
         set the HOME environment variable or pass an explicit xelian home path"
    )]
    NoHomeDir,
}

/// Resolves and provides access to the Xelian home directory (`~/.xelian` by
/// default) and its standard subdirectories.
///
/// `XelianHome` only computes paths — it does not create anything on disk
/// until [`ensure_layout`](XelianHome::ensure_layout) is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XelianHome {
    root: PathBuf,
}

impl XelianHome {
    /// Resolves the default Xelian home directory: `$HOME/.xelian`.
    ///
    /// # Errors
    ///
    /// Returns [`CacheError::NoHomeDir`] if the home directory cannot be
    /// determined (see [`dirs::home_dir`]).
    pub fn resolve() -> Result<Self, CacheError> {
        let home = dirs::home_dir().ok_or(CacheError::NoHomeDir)?;
        Ok(Self {
            root: home.join(".xelian"),
        })
    }

    /// Builds a `XelianHome` rooted at an explicit path, bypassing home
    /// directory resolution. Intended for tests and for a future `--xelian-home`
    /// style override flag.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { root: path.into() }
    }

    /// The Xelian home root directory (e.g. `~/.xelian`).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/packages` — downloaded/extracted `.xelian` packages.
    pub fn packages(&self) -> PathBuf {
        self.root.join("packages")
    }

    /// `<root>/runtimes` — managed language runtimes (Python via uv, Node via npm).
    pub fn runtimes(&self) -> PathBuf {
        self.root.join("runtimes")
    }

    /// `<root>/envs` — isolated per-package-version environments.
    pub fn envs(&self) -> PathBuf {
        self.root.join("envs")
    }

    /// `<root>/models` — downloaded Ollama models.
    pub fn models(&self) -> PathBuf {
        self.root.join("models")
    }

    /// `<root>/logs` — Xelian CLI operation logs.
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// `<root>/tmp` — scratch space for in-progress downloads/extraction.
    pub fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// `<root>/credentials.toml` — registry authentication token(s) (§14.4).
    ///
    /// This method only returns the path. Nothing in this module creates,
    /// writes, or otherwise touches this file: per §11.3, `credentials.toml`
    /// is top-level cache-adjacent state that must survive cache-clearing
    /// operations (`xelian rm --all`), so lifecycle management belongs
    /// exclusively to the login/logout code path, not to cache layout setup.
    pub fn credentials_path(&self) -> PathBuf {
        self.root.join("credentials.toml")
    }

    /// The on-disk directory for a package pulled from a local directory
    /// source: `packages/local/<name>/<version>`.
    ///
    /// Does not create anything on disk.
    pub fn local_package_dir(&self, name: &str, version: &str) -> PathBuf {
        self.packages().join("local").join(name).join(version)
    }

    /// The on-disk directory for the isolated environment of a package pulled
    /// from a local directory source: `envs/local/<name>/<version>`.
    ///
    /// Does not create anything on disk.
    pub fn local_env_dir(&self, name: &str, version: &str) -> PathBuf {
        self.envs().join("local").join(name).join(version)
    }

    /// The grant-state file for a package pulled from a local directory
    /// source: `permissions/local/<name>/<version>.toml` (§16.2).
    ///
    /// Grant state lives under `~/.xelian/` — never inside the extracted
    /// package cache, whose contents are checksum-verified archive content a
    /// package author controls.
    ///
    /// Does not create anything on disk.
    pub fn local_grants_path(&self, name: &str, version: &str) -> PathBuf {
        self.root
            .join("permissions")
            .join("local")
            .join(name)
            .join(format!("{version}.toml"))
    }

    /// The on-disk directory for a package imported from GitHub at a
    /// specific commit SHA: `packages/github/<owner>/<repo>/<sha>` (SPEC.md
    /// §12.2 step 1).
    ///
    /// Addressed by commit SHA, not branch name, so re-imports of the same
    /// commit are reproducible and immutable the same way registry packages
    /// are (§9.11) — the cache decision behind this shape is recorded at the
    /// top of this module.
    ///
    /// Does not create anything on disk.
    pub fn github_package_dir(&self, owner: &str, repo: &str, sha: &str) -> PathBuf {
        self.packages().join("github").join(owner).join(repo).join(sha)
    }

    /// The on-disk directory for the isolated environment of a package
    /// imported from GitHub at a specific commit SHA:
    /// `envs/github/<owner>/<repo>/<sha>`.
    ///
    /// Does not create anything on disk.
    pub fn github_env_dir(&self, owner: &str, repo: &str, sha: &str) -> PathBuf {
        self.envs().join("github").join(owner).join(repo).join(sha)
    }

    /// The on-disk directory for a package pulled from the registry:
    /// `packages/registry/<owner>/<name>/<version>` (SPEC.md §11.1,
    /// decision 2026-07-16).
    ///
    /// Does not create anything on disk.
    pub fn registry_package_dir(&self, owner: &str, name: &str, version: &str) -> PathBuf {
        self.packages()
            .join("registry")
            .join(owner)
            .join(name)
            .join(version)
    }

    /// The on-disk directory for the isolated environment of a package pulled
    /// from the registry: `envs/registry/<owner>/<name>/<version>`.
    ///
    /// Does not create anything on disk.
    pub fn registry_env_dir(&self, owner: &str, name: &str, version: &str) -> PathBuf {
        self.envs()
            .join("registry")
            .join(owner)
            .join(name)
            .join(version)
    }

    /// The grant-state file for a package pulled from the registry:
    /// `permissions/registry/<owner>/<name>/<version>.toml` (§16.2).
    ///
    /// Does not create anything on disk.
    pub fn registry_grants_path(&self, owner: &str, name: &str, version: &str) -> PathBuf {
        self.root
            .join("permissions")
            .join("registry")
            .join(owner)
            .join(name)
            .join(format!("{version}.toml"))
    }

    /// The grant-state file for a package imported from GitHub at a specific
    /// commit SHA: `permissions/github/<owner>/<repo>/<sha>.toml` (§16.2).
    ///
    /// Grant state lives under `~/.xelian/` — never inside the extracted
    /// package cache, whose contents are checksum-verified archive content a
    /// package author controls (same rationale as
    /// [`local_grants_path`](Self::local_grants_path)).
    ///
    /// Does not create anything on disk.
    pub fn github_grants_path(&self, owner: &str, repo: &str, sha: &str) -> PathBuf {
        self.root
            .join("permissions")
            .join("github")
            .join(owner)
            .join(repo)
            .join(format!("{sha}.toml"))
    }

    /// Lazily creates the root directory and all six standard subdirectories
    /// (`packages/`, `runtimes/`, `envs/`, `models/`, `logs/`, `tmp/`).
    ///
    /// Idempotent: safe to call on every invocation of the CLI. Never creates
    /// or touches `credentials.toml` (§11.3).
    pub fn ensure_layout(&self) -> io::Result<()> {
        for dir in [
            self.root.clone(),
            self.packages(),
            self.runtimes(),
            self.envs(),
            self.models(),
            self.logs(),
            self.tmp(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

/// Walk `packages/` and return every cached package found across all sources
/// (local, github, registry). Omits directories that do not contain a
/// `xelian.toml` (i.e., are not valid extracted packages).
pub fn list_cached_packages(home: &XelianHome) -> io::Result<Vec<CachedPackage>> {
    let mut packages = Vec::new();

    let pkgs_root = home.packages();
    if !pkgs_root.is_dir() {
        return Ok(packages);
    }

    // --- local/<name>/<version>/ ---
    let local_dir = pkgs_root.join("local");
    if local_dir.is_dir() {
        for name_entry in fs::read_dir(&local_dir)? {
            let name_entry = name_entry?;
            let name_path = name_entry.path();
            if !name_path.is_dir() {
                continue;
            }
            let name = name_entry
                .file_name()
                .to_string_lossy()
                .into_owned();
            for ver_entry in fs::read_dir(&name_path)? {
                let ver_entry = ver_entry?;
                let ver_path = ver_entry.path();
                if !ver_path.is_dir() {
                    continue;
                }
                if !ver_path.join("xelian.toml").is_file() {
                    continue;
                }
                let version = ver_entry
                    .file_name()
                    .to_string_lossy()
                    .into_owned();
                let env_path = home.local_env_dir(&name, &version);
                packages.push(CachedPackage {
                    source: PackageSource::Local,
                    name: name.clone(),
                    version,
                    path: ver_path,
                    env_path: env_path.is_dir().then_some(env_path),
                });
            }
        }
    }

    // --- github/<owner>/<repo>/<sha>/ ---
    let github_dir = pkgs_root.join("github");
    if github_dir.is_dir() {
        for owner_entry in fs::read_dir(&github_dir)? {
            let owner_entry = owner_entry?;
            let owner_path = owner_entry.path();
            if !owner_path.is_dir() {
                continue;
            }
            let owner = owner_entry.file_name().to_string_lossy().into_owned();
            for repo_entry in fs::read_dir(&owner_path)? {
                let repo_entry = repo_entry?;
                let repo_path = repo_entry.path();
                if !repo_path.is_dir() {
                    continue;
                }
                let repo = repo_entry.file_name().to_string_lossy().into_owned();
                for sha_entry in fs::read_dir(&repo_path)? {
                    let sha_entry = sha_entry?;
                    let sha_path = sha_entry.path();
                    if !sha_path.is_dir() {
                        continue;
                    }
                    if !sha_path.join("xelian.toml").is_file() {
                        continue;
                    }
                    let sha = sha_entry.file_name().to_string_lossy().into_owned();
                    let env_path = home.github_env_dir(&owner, &repo, &sha);
                    packages.push(CachedPackage {
                        source: PackageSource::Github {
                            owner: owner.clone(),
                            repo: repo.clone(),
                        },
                        name: repo.clone(),
                        version: sha,
                        path: sha_path,
                        env_path: env_path.is_dir().then_some(env_path),
                    });
                }
            }
        }
    }

    // --- registry/<owner>/<name>/<version>/ ---
    let reg_dir = pkgs_root.join("registry");
    if reg_dir.is_dir() {
        for owner_entry in fs::read_dir(&reg_dir)? {
            let owner_entry = owner_entry?;
            let owner_path = owner_entry.path();
            if !owner_path.is_dir() {
                continue;
            }
            let owner = owner_entry.file_name().to_string_lossy().into_owned();
            for name_entry in fs::read_dir(&owner_path)? {
                let name_entry = name_entry?;
                let name_path = name_entry.path();
                if !name_path.is_dir() {
                    continue;
                }
                let name = name_entry.file_name().to_string_lossy().into_owned();
                for ver_entry in fs::read_dir(&name_path)? {
                    let ver_entry = ver_entry?;
                    let ver_path = ver_entry.path();
                    if !ver_path.is_dir() {
                        continue;
                    }
                    if !ver_path.join("xelian.toml").is_file() {
                        continue;
                    }
                    let version = ver_entry.file_name().to_string_lossy().into_owned();
                    let env_path = home.envs().join("registry").join(&owner).join(&name).join(&version);
                    let name_clone = name.clone();
                    packages.push(CachedPackage {
                        source: PackageSource::Registry {
                            owner: owner.clone(),
                        },
                        name: name_clone,
                        version,
                        path: ver_path,
                        env_path: env_path.is_dir().then_some(env_path),
                    });
                }
            }
        }
    }

    Ok(packages)
}

/// Remove all cached versions of `owner/name` from `packages/`. If
/// `remove_env` is true, also remove from `envs/`. Only affects local cache
/// — never contacts the registry (§13.6).
///
/// Every path is verified to stay within `~/.xelian/` before any deletion.
pub fn remove_packages(
    home: &XelianHome,
    owner: &str,
    name: &str,
    remove_env: bool,
) -> io::Result<RemoveOutcome> {
    let mut outcome = RemoveOutcome {
        removed_packages: Vec::new(),
        removed_envs: Vec::new(),
    };

    // --- github/<owner>/<name>/ ---
    let github_pkg = home.packages().join("github").join(owner).join(name);
    if github_pkg.is_dir() {
        for entry in fs::read_dir(&github_pkg)? {
            let entry = entry?;
            let sha_path = entry.path();
            if !sha_path.is_dir() || !confined_to_home(&sha_path, home) {
                continue;
            }
            fs::remove_dir_all(&sha_path)?;
            outcome.removed_packages.push(sha_path);
        }
    }

    // --- registry/<owner>/<name>/ ---
    let reg_pkg = home.packages().join("registry").join(owner).join(name);
    if reg_pkg.is_dir() {
        for entry in fs::read_dir(&reg_pkg)? {
            let entry = entry?;
            let ver_path = entry.path();
            if !ver_path.is_dir() || !confined_to_home(&ver_path, home) {
                continue;
            }
            fs::remove_dir_all(&ver_path)?;
            outcome.removed_packages.push(ver_path);
        }
    }

    // --- environments ---
    if remove_env {
        let github_env = home.envs().join("github").join(owner).join(name);
        if github_env.is_dir() {
            for entry in fs::read_dir(&github_env)? {
                let entry = entry?;
                let sha_path = entry.path();
                if !sha_path.is_dir() || !confined_to_home(&sha_path, home) {
                    continue;
                }
                fs::remove_dir_all(&sha_path)?;
                outcome.removed_envs.push(sha_path);
            }
        }
        let reg_env = home.envs().join("registry").join(owner).join(name);
        if reg_env.is_dir() {
            for entry in fs::read_dir(&reg_env)? {
                let entry = entry?;
                let ver_path = entry.path();
                if !ver_path.is_dir() || !confined_to_home(&ver_path, home) {
                    continue;
                }
                fs::remove_dir_all(&ver_path)?;
                outcome.removed_envs.push(ver_path);
            }
        }
    }

    Ok(outcome)
}

/// Remove all cached versions of a local package (built/run from a local
/// `.xelian` path) by `name`, from `packages/local/<name>/`. Local packages
/// have no owner namespace, so they cannot be addressed by the `owner/name`
/// form [`remove_packages`] expects — this is their removal path (§13.6).
///
/// If `remove_env` is true, also removes `envs/local/<name>/`. Every path is
/// verified to stay within `~/.xelian/` before deletion.
pub fn remove_local_packages(
    home: &XelianHome,
    name: &str,
    remove_env: bool,
) -> io::Result<RemoveOutcome> {
    let mut outcome = RemoveOutcome {
        removed_packages: Vec::new(),
        removed_envs: Vec::new(),
    };

    let local_pkg = home.packages().join("local").join(name);
    if local_pkg.is_dir() {
        for entry in fs::read_dir(&local_pkg)? {
            let ver_path = entry?.path();
            if !ver_path.is_dir() || !confined_to_home(&ver_path, home) {
                continue;
            }
            fs::remove_dir_all(&ver_path)?;
            outcome.removed_packages.push(ver_path);
        }
    }

    if remove_env {
        let local_env = home.envs().join("local").join(name);
        if local_env.is_dir() {
            for entry in fs::read_dir(&local_env)? {
                let ver_path = entry?.path();
                if !ver_path.is_dir() || !confined_to_home(&ver_path, home) {
                    continue;
                }
                fs::remove_dir_all(&ver_path)?;
                outcome.removed_envs.push(ver_path);
            }
        }
    }

    Ok(outcome)
}

/// Remove all children of `packages/`, `envs/`, `runtimes/`, `models/`.
/// Never touches `credentials.toml` (§11.3) or any other root-level file.
/// Every removed path is verified to stay within `~/.xelian/`.
pub fn remove_all(home: &XelianHome) -> io::Result<()> {
    for cache_dir in [home.packages(), home.envs(), home.runtimes(), home.models()] {
        if !cache_dir.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&cache_dir)? {
            let entry = entry?;
            let child = entry.path();
            if !confined_to_home(&child, home) {
                continue;
            }
            if child.is_dir() {
                fs::remove_dir_all(&child)?;
            } else {
                fs::remove_file(&child)?;
            }
        }
    }
    Ok(())
}

/// `true` if `path` is a strict descendant of `home.root()`. Acts as a
/// belt-and-braces guard against path-traversal bugs in removal logic.
fn confined_to_home(path: &Path, home: &XelianHome) -> bool {
    path.canonicalize()
        .ok()
        .and_then(|canon| {
            let root = home.root().canonicalize().ok()?;
            Some(canon.starts_with(&root))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_layout_creates_all_six_subdirs() {
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());

        home.ensure_layout().unwrap();

        assert!(home.root().is_dir());
        assert!(home.packages().is_dir());
        assert!(home.runtimes().is_dir());
        assert!(home.envs().is_dir());
        assert!(home.models().is_dir());
        assert!(home.logs().is_dir());
        assert!(home.tmp().is_dir());
    }

    #[test]
    fn ensure_layout_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());

        home.ensure_layout().unwrap();
        // Second call must not fail or alter anything.
        home.ensure_layout().unwrap();

        assert!(home.packages().is_dir());
    }

    #[test]
    fn ensure_layout_does_not_create_credentials_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());

        home.ensure_layout().unwrap();

        assert!(!home.credentials_path().exists());
        assert_eq!(home.credentials_path(), tmp.path().join("credentials.toml"));
    }

    #[test]
    fn path_accessors_return_expected_joined_paths() {
        let home = XelianHome::at("/fake/root");

        assert_eq!(home.root(), Path::new("/fake/root"));
        assert_eq!(home.packages(), Path::new("/fake/root/packages"));
        assert_eq!(home.runtimes(), Path::new("/fake/root/runtimes"));
        assert_eq!(home.envs(), Path::new("/fake/root/envs"));
        assert_eq!(home.models(), Path::new("/fake/root/models"));
        assert_eq!(home.logs(), Path::new("/fake/root/logs"));
        assert_eq!(home.tmp(), Path::new("/fake/root/tmp"));
        assert_eq!(
            home.credentials_path(),
            Path::new("/fake/root/credentials.toml")
        );
    }

    #[test]
    fn local_package_and_env_dir_shape() {
        let home = XelianHome::at("/fake/root");

        assert_eq!(
            home.local_package_dir("my-agent", "1.2.3"),
            Path::new("/fake/root/packages/local/my-agent/1.2.3")
        );
        assert_eq!(
            home.local_env_dir("my-agent", "1.2.3"),
            Path::new("/fake/root/envs/local/my-agent/1.2.3")
        );
    }

    #[test]
    fn local_package_dir_and_env_dir_do_not_create_anything() {
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());

        let pkg_dir = home.local_package_dir("my-agent", "1.2.3");
        let env_dir = home.local_env_dir("my-agent", "1.2.3");

        assert!(!pkg_dir.exists());
        assert!(!env_dir.exists());
    }

    #[test]
    fn github_package_env_and_grants_dir_shape() {
        let home = XelianHome::at("/fake/root");
        let sha = "a".repeat(40);

        assert_eq!(
            home.github_package_dir("octocat", "hello-world", &sha),
            Path::new(&format!("/fake/root/packages/github/octocat/hello-world/{sha}"))
        );
        assert_eq!(
            home.github_env_dir("octocat", "hello-world", &sha),
            Path::new(&format!("/fake/root/envs/github/octocat/hello-world/{sha}"))
        );
        assert_eq!(
            home.github_grants_path("octocat", "hello-world", &sha),
            Path::new(&format!(
                "/fake/root/permissions/github/octocat/hello-world/{sha}.toml"
            ))
        );
    }

    #[test]
    fn github_package_dir_does_not_create_anything() {
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());
        let sha = "b".repeat(40);

        let pkg_dir = home.github_package_dir("octocat", "hello-world", &sha);
        assert!(!pkg_dir.exists());
    }

    // ---- registry package path helpers (H-161) ----

    #[test]
    fn registry_package_env_and_grants_dir_shape() {
        let home = XelianHome::at("/fake/root");

        assert_eq!(
            home.registry_package_dir("testuser", "my-pkg", "1.2.3"),
            Path::new("/fake/root/packages/registry/testuser/my-pkg/1.2.3")
        );
        assert_eq!(
            home.registry_env_dir("testuser", "my-pkg", "1.2.3"),
            Path::new("/fake/root/envs/registry/testuser/my-pkg/1.2.3")
        );
        assert_eq!(
            home.registry_grants_path("testuser", "my-pkg", "1.2.3"),
            Path::new("/fake/root/permissions/registry/testuser/my-pkg/1.2.3.toml")
        );
    }

    #[test]
    fn registry_package_dir_does_not_create_anything() {
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());

        let pkg_dir = home.registry_package_dir("user", "pkg", "1.0.0");
        assert!(!pkg_dir.exists());
    }

    // ---- list_cached_packages (H-120) ----

    #[test]
    fn list_empty_when_no_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());
        home.ensure_layout().unwrap();

        let pkgs = list_cached_packages(&home).unwrap();
        assert!(pkgs.is_empty());
    }

    #[test]
    fn list_finds_local_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());
        home.ensure_layout().unwrap();

        // Simulate a cached local package.
        let pkg_dir = home.local_package_dir("my-agent", "1.0.0");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("xelian.toml"), b"dummy").unwrap();

        let pkgs = list_cached_packages(&home).unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].name, "my-agent");
        assert_eq!(pkgs[0].version, "1.0.0");
        assert_eq!(pkgs[0].source, PackageSource::Local);
    }

    #[test]
    fn list_finds_github_imports() {
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());
        home.ensure_layout().unwrap();

        let sha = "a".repeat(40);
        let pkg_dir = home.github_package_dir("octocat", "hello-world", &sha);
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("xelian.toml"), b"dummy").unwrap();

        let pkgs = list_cached_packages(&home).unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].name, "hello-world");
        assert_eq!(pkgs[0].version, sha);
        assert_eq!(
            pkgs[0].source,
            PackageSource::Github {
                owner: "octocat".into(),
                repo: "hello-world".into()
            }
        );
    }

    #[test]
    fn list_skips_directories_without_xelian_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());
        home.ensure_layout().unwrap();

        // A directory that exists but lacks xelian.toml must be ignored.
        let pkg_dir = home.local_package_dir("incomplete", "0.1.0");
        fs::create_dir_all(&pkg_dir).unwrap();
        // No xelian.toml written.

        let pkgs = list_cached_packages(&home).unwrap();
        assert!(pkgs.is_empty());
    }

    // ---- remove_packages (H-121) ----

    #[test]
    fn remove_package_keeps_env_without_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());
        home.ensure_layout().unwrap();

        let sha = "b".repeat(40);
        let pkg_dir = home.github_package_dir("testuser", "my-pkg", &sha);
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("xelian.toml"), b"dummy").unwrap();

        let env_dir = home.github_env_dir("testuser", "my-pkg", &sha);
        fs::create_dir_all(&env_dir).unwrap();
        fs::write(env_dir.join("xelian-env.ok"), b"").unwrap();

        let outcome = remove_packages(&home, "testuser", "my-pkg", false).unwrap();
        assert_eq!(outcome.removed_packages.len(), 1);
        assert!(outcome.removed_envs.is_empty());
        assert!(env_dir.is_dir(), "env must survive without --env");
    }

    #[test]
    fn remove_package_removes_env_with_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());
        home.ensure_layout().unwrap();

        let sha = "c".repeat(40);
        let pkg_dir = home.github_package_dir("testuser", "my-pkg", &sha);
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("xelian.toml"), b"dummy").unwrap();

        let env_dir = home.github_env_dir("testuser", "my-pkg", &sha);
        fs::create_dir_all(&env_dir).unwrap();
        fs::write(env_dir.join("xelian-env.ok"), b"").unwrap();

        let outcome = remove_packages(&home, "testuser", "my-pkg", true).unwrap();
        assert_eq!(outcome.removed_packages.len(), 1);
        assert_eq!(outcome.removed_envs.len(), 1);
        assert!(!env_dir.is_dir(), "env must be removed with --env");
    }

    #[test]
    fn remove_package_no_match_is_safe_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());
        home.ensure_layout().unwrap();

        let outcome = remove_packages(&home, "nonexistent", "pkg", false).unwrap();
        assert!(outcome.removed_packages.is_empty());
        assert!(outcome.removed_envs.is_empty());
    }

    #[test]
    fn remove_local_package_by_bare_name() {
        // Local packages (built/run from a local .xelian path) have no owner
        // namespace, so they are addressed by bare `name` — `remove_packages`'s
        // owner/name form can never reach them.
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());
        home.ensure_layout().unwrap();

        let pkg_dir = home.local_package_dir("my-agent", "1.0.0");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("xelian.toml"), b"dummy").unwrap();

        let env_dir = home.local_env_dir("my-agent", "1.0.0");
        fs::create_dir_all(&env_dir).unwrap();

        // Without --env: package removed, env kept.
        let outcome = remove_local_packages(&home, "my-agent", false).unwrap();
        assert_eq!(outcome.removed_packages.len(), 1);
        assert!(outcome.removed_envs.is_empty());
        assert!(!pkg_dir.is_dir(), "local package must be removed");
        assert!(env_dir.is_dir(), "env must survive without --env");

        // With --env on a re-created package: env removed too.
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("xelian.toml"), b"dummy").unwrap();
        let outcome = remove_local_packages(&home, "my-agent", true).unwrap();
        assert_eq!(outcome.removed_packages.len(), 1);
        assert_eq!(outcome.removed_envs.len(), 1);
        assert!(!env_dir.is_dir(), "env must be removed with --env");
    }

    // ---- remove_all (H-121) ----

    #[test]
    fn remove_all_clears_four_cache_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());
        home.ensure_layout().unwrap();

        // Populate something in each cacheable dir.
        fs::create_dir_all(home.packages().join("local/a/1.0.0")).unwrap();
        fs::create_dir_all(home.envs().join("local/a/1.0.0")).unwrap();
        fs::create_dir_all(home.runtimes().join("python")).unwrap();
        fs::create_dir_all(home.models().join("llama3")).unwrap();

        remove_all(&home).unwrap();

        assert!(home.packages().is_dir(), "packages dir itself still exists");
        assert!(home.envs().is_dir(), "envs dir itself still exists");
        assert!(home.runtimes().is_dir(), "runtimes dir itself still exists");
        assert!(home.models().is_dir(), "models dir itself still exists");
        // But their children should be gone.
        assert_eq!(
            fs::read_dir(home.packages()).unwrap().count(),
            0,
            "packages must be empty"
        );
        assert_eq!(
            fs::read_dir(home.envs()).unwrap().count(),
            0,
            "envs must be empty"
        );
        assert_eq!(
            fs::read_dir(home.runtimes()).unwrap().count(),
            0,
            "runtimes must be empty"
        );
        assert_eq!(
            fs::read_dir(home.models()).unwrap().count(),
            0,
            "models must be empty"
        );
    }

    #[test]
    fn remove_all_preserves_credentials_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());
        home.ensure_layout().unwrap();

        // Write a credentials.toml.
        fs::write(home.credentials_path(), b"token = \"secret\"").unwrap();
        // Write something in packages/ to clear.
        fs::create_dir_all(home.packages().join("local/x/1.0.0")).unwrap();

        remove_all(&home).unwrap();

        assert!(
            home.credentials_path().is_file(),
            "credentials.toml must survive remove_all"
        );
        assert_eq!(
            fs::read_to_string(home.credentials_path()).unwrap(),
            "token = \"secret\""
        );
    }

    #[test]
    fn resolve_ends_with_dot_xelian_when_home_dir_exists() {
        // Don't set env vars here (process env is shared across test threads);
        // just check that if a home dir *is* resolvable on this machine, the
        // result is a `.xelian` suffix of it.
        if let Some(home_dir) = dirs::home_dir() {
            let resolved = XelianHome::resolve().unwrap();
            assert_eq!(resolved.root(), home_dir.join(".xelian"));
            assert_eq!(resolved.root().file_name().unwrap(), ".xelian");
        }
    }
}
