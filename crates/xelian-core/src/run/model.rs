//! Ollama model management (SPEC.md §9.9, §18).
//!
//! If a package declares `primary-model` and the model is not already
//! available locally, Xelian auto-installs Ollama (if absent) and pulls the
//! model before launch.
//!
//! All progress/status output goes to stderr: for MCP packages, stdout is the
//! child's JSON-RPC stdio transport and must stay untouched.

use std::path::Path;
use std::process::Command;
use thiserror::Error;

use crate::cache::XelianHome;
use crate::run::runtime::{binary_reports_version, find_in_path, run_command_checked};

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("I/O error during model operation: {0}")]
    Io(#[from] std::io::Error),

    #[error("failed to execute ollama command: {0}")]
    Process(String),

    #[error("ollama installation failed: {0}")]
    InstallFailed(String),
}

/// Find the `ollama` binary in PATH or at common locations.
fn find_ollama() -> Option<std::path::PathBuf> {
    find_in_path("ollama").or_else(|| {
        // Common install locations that may not be on a minimal PATH.
        for candidate in &["/usr/local/bin/ollama", "/opt/homebrew/bin/ollama"] {
            let p = Path::new(candidate);
            if p.is_file() {
                return Some(p.to_path_buf());
            }
        }
        None
    })
}

/// True when `name` from `ollama list` satisfies the declared `model`.
///
/// `ollama list` names always carry a tag (`llama3:latest`). A declared model
/// without a tag matches any tag of the same model; a declared model with a
/// tag must match exactly. Plain prefix matching is wrong here: `llama3` must
/// not be satisfied by `llama3.1:8b` or `llama30-custom:latest`.
fn model_name_matches(declared: &str, name: &str) -> bool {
    if name == declared {
        return true;
    }
    if declared.contains(':') {
        return false;
    }
    match name.split_once(':') {
        Some((base, _tag)) => base == declared,
        None => false,
    }
}

/// Check if a specific model is already available in Ollama.
fn model_is_available(ollama: &Path, model: &str) -> Result<bool, ModelError> {
    let mut cmd = Command::new(ollama);
    cmd.arg("list");
    let output = cmd.output().map_err(ModelError::Io)?;
    if !output.status.success() {
        return Err(ModelError::Process(
            "ollama list failed — is the Ollama daemon running?".to_string(),
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The first line is a column header (NAME  ID  SIZE  MODIFIED).
    Ok(stdout.lines().skip(1).any(|line| {
        line.split_whitespace()
            .next()
            .is_some_and(|name| model_name_matches(model, name))
    }))
}

/// Install Ollama automatically (SPEC.md §9.9, §18).
///
/// Uses the official install script, which supports Linux and macOS.
/// Returns the path to a responding `ollama` binary.
fn install_ollama(home: &XelianHome) -> Result<std::path::PathBuf, ModelError> {
    eprintln!("Ollama not found. Installing Ollama automatically...");

    if !cfg!(target_os = "macos") && !cfg!(target_os = "linux") {
        return Err(ModelError::InstallFailed(
            "Unsupported platform for automatic Ollama installation. \
             Please install Ollama manually from https://ollama.com"
                .to_string(),
        ));
    }

    let install_script = home.tmp().join("ollama-install.sh");
    std::fs::create_dir_all(home.tmp())?;

    // Download the official install script.
    let mut curl = Command::new("curl");
    curl.args(["-fsSL", "https://ollama.com/install.sh", "-o"])
        .arg(&install_script);
    run_command_checked(&mut curl).map_err(|e| {
        ModelError::InstallFailed(format!("failed to download install script: {e}"))
    })?;

    // Run the install script, streaming its progress to the user (stderr;
    // stdout must stay clean for MCP stdio).
    let mut sh = Command::new("sh");
    sh.arg(&install_script);
    sh.stdout(std::process::Stdio::inherit());
    sh.stderr(std::process::Stdio::inherit());
    let sh_status = sh
        .status()
        .map_err(|e| ModelError::InstallFailed(format!("failed to run install script: {e}")))?;

    if !sh_status.success() {
        return Err(ModelError::InstallFailed(
            "Ollama install script failed (may need sudo)".to_string(),
        ));
    }
    let _ = std::fs::remove_file(&install_script);

    // Poll until the binary appears and responds, instead of a blind sleep:
    // the daemon can take a moment to come up after installation.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if let Some(path) = find_ollama() {
            if binary_reports_version(&path) {
                return Ok(path);
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(ModelError::InstallFailed(
                "Ollama install completed but the binary is not responding — \
                 try starting the Ollama daemon manually"
                    .to_string(),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

/// Ensure the `primary-model` declared in the manifest is available before
/// launch (SPEC.md §9.9 step 10, §18).
///
/// Steps:
/// 1. If no `primary-model` is declared, do nothing.
/// 2. Ensure Ollama is installed and running (§9.9).
/// 3. If the model is not already present, pull it (§9.9).
///
/// Models are cached by Ollama in its own store; Xelian does not duplicate
/// the cache in `~/.xelian/models/` in V1 (the directory exists but is
/// reserved for future use).
pub fn ensure_model(primary_model: Option<&str>, home: &XelianHome) -> Result<(), ModelError> {
    let model = match primary_model {
        Some(m) if !m.is_empty() => m,
        _ => return Ok(()),
    };

    // Step 1: find or install Ollama.
    let ollama = match find_ollama() {
        Some(path) if binary_reports_version(&path) => path,
        _ => install_ollama(home)?,
    };

    // Step 2: check if the model is already available.
    if model_is_available(&ollama, model)? {
        return Ok(());
    }

    // Step 3: pull the model, streaming progress to the user on stderr
    // (stdout must stay clean for MCP stdio).
    eprintln!("Downloading model {model}... (this may take a while)");
    let mut pull = Command::new(&ollama);
    pull.arg("pull").arg(model);
    pull.stdout(std::process::Stdio::inherit());
    pull.stderr(std::process::Stdio::inherit());
    let pull_status = pull
        .status()
        .map_err(|e| ModelError::Process(format!("failed to run ollama pull: {e}")))?;

    if !pull_status.success() {
        return Err(ModelError::Process(format!(
            "ollama pull {model} failed — is the Ollama daemon running?"
        )));
    }

    eprintln!("Model {model} ready.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_model_does_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let home = XelianHome::at(tmp.path());
        ensure_model(None, &home).unwrap();
        ensure_model(Some(""), &home).unwrap();
    }

    #[test]
    fn find_ollama_at_known_locations() {
        // This is mostly a structural test — the actual paths may or may not
        // exist on the test runner. We just verify the function doesn't panic.
        let _ = find_ollama();
    }

    #[test]
    fn untagged_declared_model_matches_any_tag_of_the_same_base() {
        assert!(model_name_matches("llama3", "llama3:latest"));
        assert!(model_name_matches("llama3", "llama3:8b"));
        assert!(model_name_matches("llama3", "llama3"));
    }

    #[test]
    fn declared_model_is_not_satisfied_by_a_prefix_collision() {
        assert!(!model_name_matches("llama3", "llama3.1:8b"));
        assert!(!model_name_matches("llama3", "llama30-custom:latest"));
        assert!(!model_name_matches("codellama", "codellama2:latest"));
    }

    #[test]
    fn tagged_declared_model_requires_exact_match() {
        assert!(model_name_matches("llama3:8b", "llama3:8b"));
        assert!(!model_name_matches("llama3:8b", "llama3:latest"));
        assert!(!model_name_matches("llama3:8b", "llama3"));
    }
}
