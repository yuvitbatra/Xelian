//! Credential management for the Harbor registry (SPEC.md §14.4, §13.7, §13.8).
//!
//! This module handles reading, writing, and deleting the stored registry
//! credential in `~/.harbor/credentials.toml`, which is a top-level cache-
//! adjacent file that MUST survive `harbor rm --all` (§11.3).

use std::fs;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use serde::{Deserialize, Serialize};

use crate::cache::HarborHome;

/// The default registry URL used when none is configured.
pub const DEFAULT_REGISTRY_URL: &str = "http://localhost:8000";

/// The stored credential: a token, the username it belongs to, and the
/// registry URL it was obtained from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredCredentials {
    pub token: String,
    pub username: String,
    pub registry_url: String,
}

/// Errors from credential operations.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("not logged in — run `harbor login` first")]
    NotLoggedIn,

    #[error("failed to read credentials: {0}")]
    Io(#[from] io::Error),

    #[error("failed to parse credentials.toml: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("failed to serialize credentials: {0}")]
    Serialize(#[from] toml::ser::Error),
}

/// Read the stored credential from `~/.harbor/credentials.toml`.
///
/// Returns `None` if the file does not exist or is unreadable.
pub fn read_credentials(home: &HarborHome) -> Result<Option<StoredCredentials>, CredentialError> {
    let path = home.credentials_path();
    if !path.is_file() {
        return Ok(None);
    }
    let contents = fs::read_to_string(path)?;
    // Strip any trailing newlines / whitespace before parsing — TOML is
    // sensitive to trailing content after values.
    let creds: StoredCredentials = toml::from_str(contents.trim())?;
    Ok(Some(creds))
}

/// Write the credential to `~/.harbor/credentials.toml` with `0600`
/// permissions (Unix) or equivalent restrictive permissions (other platforms).
///
/// Creates the parent `~/.harbor/` directory if it does not exist.
pub fn write_credentials(
    home: &HarborHome,
    creds: &StoredCredentials,
) -> Result<(), CredentialError> {
    let path = home.credentials_path();

    // Ensure the parent directory exists.
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let toml_str = toml::to_string(creds)?;

    // Write atomically via temp file + rename to avoid partial writes.
    let tmp_path = path.with_extension("credentials.tmp");
    {
        let mut f = fs::File::create(&tmp_path)?;
        f.write_all(toml_str.as_bytes())?;
        f.flush()?;

        // Set permissions to 0600 on Unix.
        #[cfg(unix)]
        {
            let mut perms = f.metadata()?.permissions();
            perms.set_mode(0o600);
            f.set_permissions(perms)?;
        }

        // On non-Unix platforms (e.g., Windows), rely on the default file
        // permissions which are typically user-restricted.
    }
    fs::rename(&tmp_path, &path)?;

    // Also set permissions on the final path (rename may not preserve them).
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&path, perms)?;
    }

    Ok(())
}

/// Remove the stored credential from `~/.harbor/credentials.toml`.
///
/// Does nothing if the file does not exist.
pub fn delete_credentials(home: &HarborHome) -> Result<(), CredentialError> {
    let path = home.credentials_path();
    if path.is_file() {
        fs::remove_file(&path)?;
    }
    // Also clean up any stale temp file.
    let tmp_path = path.with_extension("credentials.tmp");
    if tmp_path.is_file() {
        let _ = fs::remove_file(&tmp_path);
    }
    Ok(())
}

/// Determine the registry URL to use, following priority:
///
/// 1. The URL stored in `credentials.toml` (authenticated session).
/// 2. The `HARBOR_REGISTRY_URL` environment variable.
/// 3. The compile-time default (`http://localhost:8000`).
///
/// Use `read_credentials` first; if credentials exist, use their URL.
/// Otherwise fall back to env var → default.
pub fn resolve_registry_url(home: &HarborHome) -> String {
    // Check stored credentials first.
    if let Ok(Some(creds)) = read_credentials(home) {
        return creds.registry_url;
    }
    // Fall back to env var.
    std::env::var("HARBOR_REGISTRY_URL").unwrap_or_else(|_| DEFAULT_REGISTRY_URL.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::tempdir;

    /// Serialize tests that read/write `HARBOR_REGISTRY_URL` so they do not
    /// race on the process-global env var.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn test_home() -> (HarborHome, tempfile::TempDir) {
        let tmp = tempdir().unwrap();
        let home = HarborHome::at(tmp.path());
        (home, tmp)
    }

    #[test]
    fn no_credentials_file_returns_none() {
        let (home, _tmp) = test_home();
        let creds = read_credentials(&home).unwrap();
        assert!(creds.is_none());
    }

    #[test]
    fn write_then_read_roundtrip() {
        let (home, _tmp) = test_home();
        let creds = StoredCredentials {
            token: "test-token-123".into(),
            username: "testuser".into(),
            registry_url: "http://localhost:8000".into(),
        };

        write_credentials(&home, &creds).unwrap();

        let read_back = read_credentials(&home).unwrap().expect("should be Some");
        assert_eq!(read_back.token, "test-token-123");
        assert_eq!(read_back.username, "testuser");
        assert_eq!(read_back.registry_url, "http://localhost:8000");
    }

    #[test]
    fn credentials_file_has_0600_permissions_on_unix() {
        let (home, _tmp) = test_home();
        let creds = StoredCredentials {
            token: "t".into(),
            username: "u".into(),
            registry_url: "http://localhost:8000".into(),
        };

        write_credentials(&home, &creds).unwrap();

        #[cfg(unix)]
        {
            let meta = fs::metadata(home.credentials_path()).unwrap();
            let mode = meta.permissions().mode();
            // Only the owner should have read/write (0600 = 0o600).
            assert_eq!(
                mode & 0o777,
                0o600,
                "expected 0600 permissions, got {:o}",
                mode
            );
        }
    }

    #[test]
    fn delete_credentials_removes_file() {
        let (home, _tmp) = test_home();
        let creds = StoredCredentials {
            token: "t".into(),
            username: "u".into(),
            registry_url: "http://localhost:8000".into(),
        };

        write_credentials(&home, &creds).unwrap();
        assert!(home.credentials_path().is_file());

        delete_credentials(&home).unwrap();
        assert!(!home.credentials_path().exists());
    }

    #[test]
    fn delete_on_nonexistent_file_is_noop() {
        let (home, _tmp) = test_home();
        delete_credentials(&home).unwrap();
        // Should not error.
    }

    #[test]
    fn resolve_url_from_credentials_takes_priority() {
        let (home, _tmp) = test_home();
        let creds = StoredCredentials {
            token: "t".into(),
            username: "u".into(),
            registry_url: "https://registry.example.com".into(),
        };
        write_credentials(&home, &creds).unwrap();

        let url = resolve_registry_url(&home);
        assert_eq!(url, "https://registry.example.com");
    }

    #[test]
    fn resolve_url_falls_back_to_env_var() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (home, _tmp) = test_home();
        std::env::set_var("HARBOR_REGISTRY_URL", "https://custom.example.com");
        let url = resolve_registry_url(&home);
        std::env::remove_var("HARBOR_REGISTRY_URL");
        assert_eq!(url, "https://custom.example.com");
    }

    #[test]
    fn resolve_url_defaults_when_nothing_configured() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (home, _tmp) = test_home();
        std::env::remove_var("HARBOR_REGISTRY_URL");
        let url = resolve_registry_url(&home);
        assert_eq!(url, DEFAULT_REGISTRY_URL);
    }
}
