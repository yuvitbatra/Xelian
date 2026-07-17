//! Local cache layout for `~/.harbor/` (SPEC.md §11).
//!
//! `HarborHome` resolves and manages the on-disk layout Harbor uses to cache
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
//!   Harbor registry.
//! - `github/<owner>/<repo>/<sha>/` — packages imported via `harbor add
//!   <github-url>` (§12), addressed by commit SHA for reproducibility.
//! - `local/<name>/<version>/` — packages built/run from a local directory
//!   (no owner namespace, since there's no remote source to disambiguate).
//!
//! This module only implements helpers for the `local/` namespace
//! (`local_package_dir`, `local_env_dir`); later tasks that implement registry
//! pulls and `harbor add` will add the `registry/` and `github/` equivalents
//! following the same shape.

use std::io;
use std::path::{Path, PathBuf};

/// Errors that can occur while resolving a [`HarborHome`].
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// No home directory could be determined for the current user/OS, so
    /// `~/.harbor` cannot be resolved. See [`dirs::home_dir`] for the
    /// platform-specific conditions under which this happens (e.g. `HOME`
    /// unset on Unix).
    #[error(
        "could not determine the current user's home directory; \
         set the HOME environment variable or pass an explicit harbor home path"
    )]
    NoHomeDir,
}

/// Resolves and provides access to the Harbor home directory (`~/.harbor` by
/// default) and its standard subdirectories.
///
/// `HarborHome` only computes paths — it does not create anything on disk
/// until [`ensure_layout`](HarborHome::ensure_layout) is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarborHome {
    root: PathBuf,
}

impl HarborHome {
    /// Resolves the default Harbor home directory: `$HOME/.harbor`.
    ///
    /// # Errors
    ///
    /// Returns [`CacheError::NoHomeDir`] if the home directory cannot be
    /// determined (see [`dirs::home_dir`]).
    pub fn resolve() -> Result<Self, CacheError> {
        let home = dirs::home_dir().ok_or(CacheError::NoHomeDir)?;
        Ok(Self {
            root: home.join(".harbor"),
        })
    }

    /// Builds a `HarborHome` rooted at an explicit path, bypassing home
    /// directory resolution. Intended for tests and for a future `--harbor-home`
    /// style override flag.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { root: path.into() }
    }

    /// The Harbor home root directory (e.g. `~/.harbor`).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/packages` — downloaded/extracted `.harbor` packages.
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

    /// `<root>/logs` — Harbor CLI operation logs.
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
    /// operations (`harbor rm --all`), so lifecycle management belongs
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
    /// Grant state lives under `~/.harbor/` — never inside the extracted
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_layout_creates_all_six_subdirs() {
        let tmp = tempfile::tempdir().unwrap();
        let home = HarborHome::at(tmp.path());

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
        let home = HarborHome::at(tmp.path());

        home.ensure_layout().unwrap();
        // Second call must not fail or alter anything.
        home.ensure_layout().unwrap();

        assert!(home.packages().is_dir());
    }

    #[test]
    fn ensure_layout_does_not_create_credentials_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let home = HarborHome::at(tmp.path());

        home.ensure_layout().unwrap();

        assert!(!home.credentials_path().exists());
        assert_eq!(home.credentials_path(), tmp.path().join("credentials.toml"));
    }

    #[test]
    fn path_accessors_return_expected_joined_paths() {
        let home = HarborHome::at("/fake/root");

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
        let home = HarborHome::at("/fake/root");

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
        let home = HarborHome::at(tmp.path());

        let pkg_dir = home.local_package_dir("my-agent", "1.2.3");
        let env_dir = home.local_env_dir("my-agent", "1.2.3");

        assert!(!pkg_dir.exists());
        assert!(!env_dir.exists());
    }

    #[test]
    fn resolve_ends_with_dot_harbor_when_home_dir_exists() {
        // Don't set env vars here (process env is shared across test threads);
        // just check that if a home dir *is* resolvable on this machine, the
        // result is a `.harbor` suffix of it.
        if let Some(home_dir) = dirs::home_dir() {
            let resolved = HarborHome::resolve().unwrap();
            assert_eq!(resolved.root(), home_dir.join(".harbor"));
            assert_eq!(resolved.root().file_name().unwrap(), ".harbor");
        }
    }
}
