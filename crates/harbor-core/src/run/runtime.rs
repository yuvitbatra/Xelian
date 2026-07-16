//! Language runtime managers and dependency installation (SPEC.md §10).

use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;
use sha2::{Digest, Sha256};
use crate::cache::HarborHome;
use crate::manifest::{Language, Manifest};

/// Errors that can occur during language runtime provisioning and dependency installation.
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("I/O error during runtime operation: {0}")]
    Io(#[from] std::io::Error),

    #[error("Network error fetching runtime artifact: {0}")]
    Network(String),

    #[error("Failed to parse Node.js version constraint '{constraint}': {err}")]
    InvalidConstraint { constraint: String, err: String },

    #[error("Unsupported npm range syntax in '{constraint}': {reason}")]
    UnsupportedConstraintSyntax { constraint: String, reason: String },

    #[error("No available Node.js version satisfies constraint '{constraint}'")]
    NoSatisfyingVersion { constraint: String },

    #[error("Integrity check failed: downloaded Node.js archive checksum does not match official SHASUMS256.txt")]
    IntegrityMismatch,

    #[error("Process execution failed: {0}")]
    Process(String),

    #[error("Validation failed: {0}")]
    Validation(String),
}

/// Helper to execute a command and handle errors.
fn run_command_checked(cmd: &mut Command) -> Result<std::process::Output, RuntimeError> {
    let output = cmd.output().map_err(RuntimeError::Io)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        return Err(RuntimeError::Process(format!(
            "Command failed: status={:?}\nstdout={}\nstderr={}",
            output.status, stdout, stderr
        )));
    }
    Ok(output)
}

/// Helper to compute the SHA-256 hex string of a file.
fn compute_sha256(path: &Path) -> Result<String, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

/// Helper to create a cross-platform symlink.
fn create_symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    std::os::unix::fs::symlink(src, dst)?;
    #[cfg(windows)]
    {
        if src.is_dir() {
            std::os::windows::fs::symlink_dir(src, dst)?;
        } else {
            std::os::windows::fs::symlink_file(src, dst)?;
        }
    }
    Ok(())
}

/// Helper to check if a binary is in the system PATH.
fn find_in_path(binary_name: &str) -> Option<PathBuf> {
    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            let candidate = path.join(binary_name);
            if candidate.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if let Ok(metadata) = candidate.metadata() {
                        if metadata.mode() & 0o111 != 0 {
                            return Some(candidate);
                        }
                    }
                }
                #[cfg(not(unix))]
                return Some(candidate);
            }
        }
    }
    None
}

/// A mkdir-based mutex serializing environment creation for a
/// `(name, version)` env dir. `mkdir` is atomic on every platform Harbor
/// targets, so whichever process creates `<env_dir>.lock` first holds the
/// lock; it is removed on drop. Waiters poll, and give up with a clear
/// error after a timeout so a crashed holder cannot wedge Harbor forever.
struct EnvLock {
    path: PathBuf,
}

impl EnvLock {
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

    fn acquire(env_dir: &Path) -> Result<Self, RuntimeError> {
        let lock_path = env_dir.with_extension("lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let start = std::time::Instant::now();
        loop {
            match std::fs::create_dir(&lock_path) {
                Ok(()) => return Ok(EnvLock { path: lock_path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if start.elapsed() > Self::TIMEOUT {
                        return Err(RuntimeError::Process(format!(
                            "timed out waiting for another harbor process to finish \
                             installing this environment (lock: {}); if no other \
                             harbor is running, delete that directory and retry",
                            lock_path.display()
                        )));
                    }
                    std::thread::sleep(Self::POLL_INTERVAL);
                }
                Err(e) => return Err(RuntimeError::Io(e)),
            }
        }
    }
}

impl Drop for EnvLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.path);
    }
}

/// The runtime manager interface for ensuring a language runtime and installing dependencies.
pub trait RuntimeManager {
    /// Ensure the manager and a matching runtime version exist, returning the path to the bin directory.
    fn ensure_runtime(&self, home: &HarborHome, constraint: &str) -> Result<PathBuf, RuntimeError>;

    /// Create the environment and install package dependencies in the target directory.
    fn install_dependencies(
        &self,
        home: &HarborHome,
        package_dir: &Path,
        env_dir: &Path,
        manifest: &Manifest,
        bin_dir: &Path,
    ) -> Result<(), RuntimeError>;
}

/// Python runtime manager implementing [`RuntimeManager`].
pub struct PythonRuntimeManager;

impl PythonRuntimeManager {
    /// Search for `uv` in `PATH` or at `~/.harbor/runtimes/bin/uv`.
    fn find_uv(&self, home: &HarborHome) -> Option<PathBuf> {
        let local_uv = home.runtimes().join("bin").join("uv");
        if local_uv.is_file() && self.check_uv_executable(&local_uv) {
            return Some(local_uv);
        }
        find_in_path("uv")
    }

    /// Check if a `uv` binary is executable and runs successfully.
    fn check_uv_executable(&self, path: &Path) -> bool {
        let mut cmd = Command::new(path);
        cmd.arg("--version");
        if let Ok(output) = cmd.output() {
            output.status.success()
        } else {
            false
        }
    }

    /// Download and run the Astral `uv` installer.
    fn install_uv(&self, home: &HarborHome) -> Result<PathBuf, RuntimeError> {
        println!("uv not found. Installing uv automatically under {}...", home.runtimes().join("bin").display());

        std::fs::create_dir_all(home.runtimes().join("bin"))?;
        std::fs::create_dir_all(home.tmp())?;

        let script_path = home.tmp().join("uv-install.sh");

        // Download via curl
        let mut curl_cmd = Command::new("curl");
        curl_cmd
            .arg("-LsSf")
            .arg("https://astral.sh/uv/install.sh")
            .arg("-o")
            .arg(&script_path);
        run_command_checked(&mut curl_cmd)?;

        // Execute via sh
        let mut sh_cmd = Command::new("sh");
        sh_cmd
            .arg(&script_path)
            .env("INSTALLER_NO_MODIFY_PATH", "1")
            .env("UV_INSTALL_DIR", home.runtimes().join("bin"));
        run_command_checked(&mut sh_cmd)?;

        // Verify it exists now
        let local_uv = home.runtimes().join("bin").join("uv");
        if !local_uv.is_file() {
            return Err(RuntimeError::Process("uv install completed but binary not found".to_string()));
        }

        if !self.check_uv_executable(&local_uv) {
            return Err(RuntimeError::Process("Installed uv is not executable or failed to run".to_string()));
        }

        // Clean up installer script
        let _ = std::fs::remove_file(script_path);

        Ok(local_uv)
    }
}

impl RuntimeManager for PythonRuntimeManager {
    fn ensure_runtime(&self, home: &HarborHome, _constraint: &str) -> Result<PathBuf, RuntimeError> {
        let uv_path = match self.find_uv(home) {
            Some(path) => path,
            None => self.install_uv(home)?,
        };

        let bin_dir = uv_path.parent()
            .ok_or_else(|| RuntimeError::Process("Invalid uv path: no parent directory".to_string()))?
            .to_path_buf();
        Ok(bin_dir)
    }

    fn install_dependencies(
        &self,
        _home: &HarborHome,
        package_dir: &Path,
        env_dir: &Path,
        manifest: &Manifest,
        bin_dir: &Path,
    ) -> Result<(), RuntimeError> {
        let uv_path = bin_dir.join("uv");

        // Python venvs are created in place (renaming a venv breaks the
        // absolute-path shebangs uv writes into bin/), so concurrent runs
        // must be serialized: take a mkdir-based lock next to the env dir.
        let _lock = EnvLock::acquire(env_dir)?;

        // Another process may have completed the install while we waited on
        // the lock.
        if env_dir.join("harbor-env.ok").is_file() {
            return Ok(());
        }

        // Staged execution helper: if anything fails, we clean up the environment folder.
        let install_result = (|| {
            // Check if env already exists and has a sentinel.
            if env_dir.join("harbor-env.ok").is_file() {
                return Ok(());
            }

            if env_dir.exists() {
                std::fs::remove_dir_all(env_dir)?;
            }
            std::fs::create_dir_all(env_dir)?;

            // 1. Create the virtual environment using `uv venv` directly in target location.
            println!("Creating Python virtual environment in {}...", env_dir.display());
            let mut venv_cmd = Command::new(&uv_path);
            venv_cmd
                .arg("venv")
                .arg(env_dir)
                .arg("--python")
                .arg(&manifest.runtime);
            run_command_checked(&mut venv_cmd)?;

            // 2. Install dependencies.
            // Check if dependencies.lockfile is present.
            if let Some(ref lockfile_name) = manifest.dependencies.lockfile {
                let lockfile_path = package_dir.join(lockfile_name);
                if lockfile_path.is_file() {
                    println!("Syncing dependencies from lockfile: {}...", lockfile_name);
                    let mut sync_cmd = Command::new(&uv_path);
                    sync_cmd
                        .arg("sync")
                        .arg("--frozen")
                        .current_dir(package_dir)
                        .env("UV_PROJECT_ENVIRONMENT", env_dir);
                    run_command_checked(&mut sync_cmd)?;
                    return Ok(());
                }
            }

            // Fallback: no lockfile or lockfile not found on disk.
            let manifest_name = &manifest.dependencies.manifest;
            if manifest_name == "pyproject.toml" {
                println!("Installing dependencies from pyproject.toml...");
                let mut pip_cmd = Command::new(&uv_path);
                pip_cmd
                    .arg("pip")
                    .arg("install")
                    .arg("--python")
                    .arg(env_dir.join("bin").join("python"))
                    .arg(package_dir);
                run_command_checked(&mut pip_cmd)?;
            } else {
                println!("Installing dependencies from {}...", manifest_name);
                let mut pip_cmd = Command::new(&uv_path);
                pip_cmd
                    .arg("pip")
                    .arg("install")
                    .arg("--python")
                    .arg(env_dir.join("bin").join("python"))
                    .arg("-r")
                    .arg(package_dir.join(manifest_name));
                run_command_checked(&mut pip_cmd)?;
            }

            Ok(())
        })();

        if install_result.is_err() {
            // Only remove if we just created it and it's dirty
            if env_dir.exists() && !env_dir.join("harbor-env.ok").is_file() {
                let _ = std::fs::remove_dir_all(env_dir);
            }
            return install_result;
        }

        // Write the sentinel file to mark the environment as valid.
        std::fs::write(env_dir.join("harbor-env.ok"), b"")?;
        Ok(())
    }
}

/// Node.js runtime manager implementing [`RuntimeManager`].
pub struct NodeRuntimeManager;

#[derive(serde::Deserialize)]
struct NodeRelease {
    version: String,
}

impl NodeRuntimeManager {
    /// Validate that the range constraint contains no unsupported npm-isms.
    fn validate_constraint_syntax(&self, constraint: &str) -> Result<(), RuntimeError> {
        let c = constraint.trim();
        if c.contains("||") {
            return Err(RuntimeError::UnsupportedConstraintSyntax {
                constraint: constraint.to_string(),
                reason: "logical OR operator (||) is not supported".to_string(),
            });
        }
        if c.contains(" - ") {
            return Err(RuntimeError::UnsupportedConstraintSyntax {
                constraint: constraint.to_string(),
                reason: "hyphen ranges are not supported".to_string(),
            });
        }
        if c.chars().any(|ch| ch.is_whitespace()) {
            let parts: Vec<&str> = c.split_whitespace().collect();
            if parts.len() > 1 {
                return Err(RuntimeError::UnsupportedConstraintSyntax {
                    constraint: constraint.to_string(),
                    reason: "space-separated multi-ranges or spaces between operator and version are not supported".to_string(),
                });
            }
        }
        // Reject wildcard components (18.x, 18.*, bare x) without rejecting
        // legitimate uses of the letter x, e.g. a prerelease tag `-xyz`.
        if c.contains(".x") || c.contains(".X") || c.contains('*') || c == "x" || c == "X" {
            return Err(RuntimeError::UnsupportedConstraintSyntax {
                constraint: constraint.to_string(),
                reason: "wildcard range syntax (.x, .X or *) is not supported".to_string(),
            });
        }
        Ok(())
    }

    /// Check if system Node and npm exist and satisfy constraint.
    fn find_system_node(&self, req: &semver::VersionReq) -> Option<PathBuf> {
        let node_bin = find_in_path("node")?;
        let npm_bin = find_in_path("npm")?;

        // Run node --version
        let mut cmd = Command::new(&node_bin);
        cmd.arg("--version");
        let output = cmd.output().ok()?;
        if !output.status.success() {
            return None;
        }
        let version_str = String::from_utf8_lossy(&output.stdout);
        let version_str = version_str.trim().trim_start_matches('v');
        let version = semver::Version::parse(version_str).ok()?;

        if req.matches(&version) {
            // Also verify npm runs
            let mut npm_cmd = Command::new(&npm_bin);
            npm_cmd.arg("--version");
            let npm_output = npm_cmd.output().ok()?;
            if npm_output.status.success() {
                return node_bin.parent().map(|p| p.to_path_buf());
            }
        }

        None
    }

    /// Look for already downloaded Node version matching the requirement.
    fn find_cached_node(&self, home: &HarborHome, req: &semver::VersionReq) -> Option<PathBuf> {
        let node_dir = home.runtimes().join("node");
        if !node_dir.is_dir() {
            return None;
        }

        let entries = std::fs::read_dir(node_dir).ok()?;
        for entry in entries {
            let entry = entry.ok()?;
            let name = entry.file_name().into_string().ok()?;
            if name.starts_with('v') {
                let ver_str = name.trim_start_matches('v');
                if let Ok(version) = semver::Version::parse(ver_str) {
                    if req.matches(&version) {
                        let bin_dir = entry.path().join("bin");
                        let node_bin = bin_dir.join("node");
                        if node_bin.is_file() {
                            return Some(bin_dir);
                        }
                    }
                }
            }
        }

        None
    }

    /// Get platform and architecture for Node.js URL.
    fn get_platform_arch(&self) -> Result<(&'static str, &'static str), RuntimeError> {
        let os = if cfg!(target_os = "macos") {
            "darwin"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else {
            return Err(RuntimeError::Validation("Unsupported OS for Node.js auto-installation".to_string()));
        };

        let arch = if cfg!(target_arch = "x86_64") {
            "x64"
        } else if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            return Err(RuntimeError::Validation("Unsupported architecture for Node.js auto-installation".to_string()));
        };

        Ok((os, arch))
    }

    /// Query nodejs.org for available releases.
    fn query_releases(&self) -> Result<Vec<NodeRelease>, RuntimeError> {
        let mut cmd = Command::new("curl");
        cmd.arg("-sSf").arg("https://nodejs.org/dist/index.json");
        let output = cmd.output().map_err(RuntimeError::Io)?;
        if !output.status.success() {
            return Err(RuntimeError::Network("Failed to fetch Node.js releases index".to_string()));
        }
        let json_str = String::from_utf8_lossy(&output.stdout);
        let releases: Vec<NodeRelease> = serde_json::from_str(&json_str)
            .map_err(|e| RuntimeError::Network(format!("Failed to parse Node.js releases JSON: {}", e)))?;
        Ok(releases)
    }

    /// Match a constraint against a list of version strings.
    fn match_version(&self, req: &semver::VersionReq, versions: &[String]) -> Option<String> {
        let mut matched: Vec<semver::Version> = Vec::new();
        for v_str in versions {
            let clean = v_str.trim_start_matches('v');
            if let Ok(ver) = semver::Version::parse(clean) {
                if req.matches(&ver) {
                    matched.push(ver);
                }
            }
        }
        matched.sort();
        matched.last().map(|v| format!("v{}", v))
    }

    /// Download and install a specific Node.js version.
    fn download_node(&self, home: &HarborHome, version: &str) -> Result<PathBuf, RuntimeError> {
        println!("Downloading Node.js version {}...", version);
        let (os, arch) = self.get_platform_arch()?;
        let archive_name = format!("node-{}-{}-{}.tar.gz", version, os, arch);
        let url = format!("https://nodejs.org/dist/{}/{}", version, archive_name);
        let sha_url = format!("https://nodejs.org/dist/{}/SHASUMS256.txt", version);

        let tmp_archive = home.tmp().join(&archive_name);
        let tmp_sha = home.tmp().join(format!("SHASUMS256-{}.txt", version));

        // Ensure directories exist
        std::fs::create_dir_all(home.tmp())?;
        std::fs::create_dir_all(home.runtimes().join("node"))?;

        // 1. Download tarball
        let mut curl_tar = Command::new("curl");
        curl_tar.arg("-LsSf").arg(&url).arg("-o").arg(&tmp_archive);
        run_command_checked(&mut curl_tar)?;

        // 2. Download SHA
        let mut curl_sha = Command::new("curl");
        curl_sha.arg("-LsSf").arg(&sha_url).arg("-o").arg(&tmp_sha);
        run_command_checked(&mut curl_sha)?;

        // 3. Read expected SHA
        let sha_content = std::fs::read_to_string(&tmp_sha)?;
        let mut expected_sha = None;
        for line in sha_content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 && parts[1] == archive_name {
                expected_sha = Some(parts[0].to_string());
                break;
            }
        }

        let expected_sha = expected_sha.ok_or(RuntimeError::IntegrityMismatch)?;

        // 4. Compute actual SHA
        let actual_sha = compute_sha256(&tmp_archive)?;
        if actual_sha != expected_sha {
            return Err(RuntimeError::IntegrityMismatch);
        }

        // 5. Extract tarball
        println!("Extracting Node.js tarball...");
        let tar_file = std::fs::File::open(&tmp_archive)?;
        let tar_decoder = flate2::read::GzDecoder::new(tar_file);
        let mut archive = tar::Archive::new(tar_decoder);
        
        let extract_dest = home.runtimes().join("node");
        archive.unpack(&extract_dest)?;

        // The tarball extracts into a subfolder node-v{version}-{os}-{arch}
        let extracted_folder = extract_dest.join(format!("node-{}-{}-{}", version, os, arch));
        let final_folder = extract_dest.join(version);

        if final_folder.exists() {
            std::fs::remove_dir_all(&final_folder)?;
        }
        std::fs::rename(extracted_folder, &final_folder)?;

        // Clean up temporary files
        let _ = std::fs::remove_file(tmp_archive);
        let _ = std::fs::remove_file(tmp_sha);

        let bin_dir = final_folder.join("bin");
        if !bin_dir.join("node").is_file() {
            return Err(RuntimeError::Process("Node.js binary not found after extraction".to_string()));
        }

        Ok(bin_dir)
    }
}

impl RuntimeManager for NodeRuntimeManager {
    fn ensure_runtime(&self, home: &HarborHome, constraint: &str) -> Result<PathBuf, RuntimeError> {
        self.validate_constraint_syntax(constraint)?;
        let req = semver::VersionReq::parse(constraint).map_err(|e| {
            RuntimeError::InvalidConstraint {
                constraint: constraint.to_string(),
                err: e.to_string(),
            }
        })?;

        // 1. Check system node
        if let Some(bin_dir) = self.find_system_node(&req) {
            return Ok(bin_dir);
        }

        // 2. Check cached node
        if let Some(bin_dir) = self.find_cached_node(home, &req) {
            return Ok(bin_dir);
        }

        // 3. Query nodejs.org
        let mut fetched_ok = false;
        let mut matched_version = None;
        if let Ok(releases) = self.query_releases() {
            fetched_ok = true;
            let versions: Vec<String> = releases.into_iter().map(|r| r.version).collect();
            matched_version = self.match_version(&req, &versions);
        }

        // 4. Fallback if network failed
        if !fetched_ok {
            println!("Network request failed. Using fallback Node.js release list...");
            let fallback_list: Vec<String> = vec![
                "v22.2.0".to_string(),
                "v20.11.1".to_string(),
                "v18.20.0".to_string(),
            ];
            matched_version = self.match_version(&req, &fallback_list);
        }

        let version = matched_version.ok_or_else(|| {
            RuntimeError::NoSatisfyingVersion { constraint: constraint.to_string() }
        })?;

        // 5. Check if matching version is already cached
        let cached_bin = home.runtimes().join("node").join(&version).join("bin");
        if cached_bin.join("node").is_file() {
            return Ok(cached_bin);
        }

        // 6. Download and install
        let bin_dir = self.download_node(home, &version)?;
        Ok(bin_dir)
    }

    /// Builds the env as symlinks to the package files plus a real
    /// `node_modules/`, keeping the extracted package cache pristine.
    ///
    /// LAUNCH CONTRACT (Phase 8): because the app sources inside the env are
    /// symlinks, Node MUST be launched from the env dir with
    /// `--preserve-symlinks --preserve-symlinks-main`, otherwise module
    /// resolution follows the symlinks' real paths back into the package
    /// cache and never finds `<env_dir>/node_modules`. Verified working
    /// end-to-end on Node 26.
    fn install_dependencies(
        &self,
        home: &HarborHome,
        package_dir: &Path,
        env_dir: &Path,
        manifest: &Manifest,
        bin_dir: &Path,
    ) -> Result<(), RuntimeError> {
        if env_dir.join("harbor-env.ok").is_file() {
            return Ok(());
        }

        // Staging directory to avoid half-complete states
        let rand_val = chrono::Utc::now().timestamp_millis();
        let stage_dir = home.tmp().join(format!("node-stage-{}-{}", manifest.name, rand_val));
        if stage_dir.exists() {
            std::fs::remove_dir_all(&stage_dir)?;
        }
        std::fs::create_dir_all(&stage_dir)?;

        let install_res = (|| {
            // 1. Symlink all top-level package files except node_modules
            let entries = std::fs::read_dir(package_dir)?;
            for entry in entries {
                let entry = entry?;
                let file_name = entry.file_name();
                if file_name == "node_modules" {
                    continue;
                }
                let src_path = entry.path();
                let dst_path = stage_dir.join(&file_name);
                create_symlink(&src_path, &dst_path)?;
            }

            // 2. Run npm install inside staging directory
            println!("Installing Node.js dependencies in staging environment...");
            let is_lock = stage_dir.join("package-lock.json").is_file();
            let npm_binary = bin_dir.join("npm");
            let mut npm_cmd = Command::new(&npm_binary);
            if is_lock {
                npm_cmd.arg("ci");
            } else {
                npm_cmd.arg("install");
            }
            npm_cmd.current_dir(&stage_dir);

            // Prepend Node's bin_dir to PATH so that nested node calls work
            if let Some(old_path) = std::env::var_os("PATH") {
                let mut new_paths = vec![bin_dir.to_path_buf()];
                new_paths.extend(std::env::split_paths(&old_path));
                let new_path_os = std::env::join_paths(new_paths).map_err(|e| {
                    RuntimeError::Process(format!("Failed to build PATH: {}", e))
                })?;
                npm_cmd.env("PATH", new_path_os);
            } else {
                npm_cmd.env("PATH", bin_dir);
            }

            run_command_checked(&mut npm_cmd)?;

            // 3. Write sentinel inside the staging folder before renaming
            std::fs::write(stage_dir.join("harbor-env.ok"), b"")?;
            Ok(())
        })();

        if install_res.is_err() {
            let _ = std::fs::remove_dir_all(&stage_dir);
            return install_res;
        }

        // 4. Rename with race-handling
        if env_dir.exists() && env_dir.join("harbor-env.ok").is_file() {
            // Lost race but other won cleanly. Clean up stage.
            let _ = std::fs::remove_dir_all(&stage_dir);
            return Ok(());
        }

        if env_dir.exists() {
            std::fs::remove_dir_all(env_dir)?;
        }

        // The version dir's parent (envs/local/<name>/) does not exist on a
        // first run; rename() does not create it.
        if let Some(parent) = env_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }

        if let Err(e) = std::fs::rename(&stage_dir, env_dir) {
            // Check if concurrently populated by another process
            if env_dir.exists() && env_dir.join("harbor-env.ok").is_file() {
                let _ = std::fs::remove_dir_all(&stage_dir);
                return Ok(());
            }
            return Err(RuntimeError::Io(e));
        }

        Ok(())
    }
}

/// Dynamic dispatch to resolve a [`RuntimeManager`] for the declared language.
pub fn get_runtime_manager(language: Language) -> Box<dyn RuntimeManager> {
    match language {
        Language::Python => Box::new(PythonRuntimeManager),
        Language::Node => Box::new(NodeRuntimeManager),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test double demonstrating that the `RuntimeManager` interface is extensible
    /// and that a new language can be added cleanly via the trait boundary.
    struct MockRustRuntimeManager;

    impl RuntimeManager for MockRustRuntimeManager {
        fn ensure_runtime(&self, _home: &HarborHome, _constraint: &str) -> Result<PathBuf, RuntimeError> {
            Ok(PathBuf::from("/mock/rust/bin"))
        }

        fn install_dependencies(
            &self,
            _home: &HarborHome,
            _package_dir: &Path,
            _env_dir: &Path,
            _manifest: &Manifest,
            _bin_dir: &Path,
        ) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    #[test]
    fn test_extensible_dispatch_with_mock_manager() {
        let home = HarborHome::at("/tmp/mock-home");
        let manager: Box<dyn RuntimeManager> = Box::new(MockRustRuntimeManager);
        let bin_path = manager.ensure_runtime(&home, ">=1.75.0").unwrap();
        assert_eq!(bin_path, PathBuf::from("/mock/rust/bin"));
    }

    #[test]
    fn test_node_constraint_validation() {
        let mgr = NodeRuntimeManager;
        assert!(mgr.validate_constraint_syntax(">=18").is_ok());
        assert!(mgr.validate_constraint_syntax("^20.11").is_ok());

        assert!(mgr.validate_constraint_syntax("16 || 18").is_err());
        assert!(mgr.validate_constraint_syntax("16.0.0 - 18.0.0").is_err());
        assert!(mgr.validate_constraint_syntax(">=16 <20").is_err());
        assert!(mgr.validate_constraint_syntax("18.x").is_err());
    }

    #[test]
    fn test_node_version_matching() {
        let mgr = NodeRuntimeManager;
        let req = semver::VersionReq::parse(">=18").unwrap();
        let list = vec!["v16.0.0".to_string(), "v18.5.0".to_string(), "v20.11.1".to_string()];
        let matched = mgr.match_version(&req, &list);
        assert_eq!(matched, Some("v20.11.1".to_string()));

        let req_precise = semver::VersionReq::parse("^18.20.0").unwrap();
        let matched_precise = mgr.match_version(&req_precise, &list);
        assert_eq!(matched_precise, None);
    }

    #[test]
    fn test_node_reverse_symlink_installation() {
        let dir = tempfile::tempdir().unwrap();
        let pkg_dir = dir.path().join("pkg");
        let env_dir = dir.path().join("env");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::create_dir_all(&env_dir).unwrap();

        // Write some package files
        std::fs::write(pkg_dir.join("package.json"), b"{}").unwrap();
        std::fs::create_dir(pkg_dir.join("src")).unwrap();
        std::fs::write(pkg_dir.join("src").join("index.js"), b"console.log(1)").unwrap();

        // Perform reverse-symlinking
        let entries = std::fs::read_dir(&pkg_dir).unwrap();
        for entry in entries {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let src = entry.path();
            let dst = env_dir.join(&name);
            create_symlink(&src, &dst).unwrap();
        }

        // Verify symlinks inside env_dir point to pkg_dir
        assert!(env_dir.join("package.json").is_file());
        assert!(env_dir.join("src").join("index.js").is_file());

        // Check symlink source
        let symlink_metadata = std::fs::symlink_metadata(env_dir.join("package.json")).unwrap();
        assert!(symlink_metadata.file_type().is_symlink());
    }
}

