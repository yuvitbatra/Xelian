//! Global secrets store (`~/.xelian/secrets.toml`).
//!
//! When a package declares a required environment variable (e.g. an API key)
//! and it isn't already in the environment, Xelian asks for it once, then keeps
//! it here — keyed by variable name — so every package that needs the same
//! variable reuses it instead of re-prompting. Written `0600`, and always under
//! `~/.xelian/`, never inside a checksum-verified package cache.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SecretsError {
    #[error("failed to read secrets store: {0}")]
    Read(String),

    #[error("failed to parse secrets store: {0}")]
    Parse(String),

    #[error("failed to write secrets store: {0}")]
    Write(String),
}

/// The persisted `[secrets]` table: `VAR_NAME = "value"`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SecretStore {
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

impl SecretStore {
    /// Load the store, treating a missing file as an empty store.
    pub fn load(path: &Path) -> Result<Self, SecretsError> {
        match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s).map_err(|e| SecretsError::Parse(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(SecretsError::Read(e.to_string())),
        }
    }

    /// The stored value for `key`, if any.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.secrets.get(key).map(String::as_str)
    }

    /// Set (or replace) a value.
    pub fn set(&mut self, key: &str, value: &str) {
        self.secrets.insert(key.to_string(), value.to_string());
    }

    /// Persist the store to `path` with `0600` permissions, creating parents.
    pub fn save(&self, path: &Path) -> Result<(), SecretsError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SecretsError::Write(e.to_string()))?;
        }
        let body = toml::to_string(self).map_err(|e| SecretsError::Write(e.to_string()))?;
        std::fs::write(path, body).map_err(|e| SecretsError::Write(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Best-effort: a key store must not be world-readable.
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

/// Whether a variable name looks like a secret (so a prompt can note it will be
/// stored). Purely cosmetic — every entered required value is stored the same.
pub fn looks_secret(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    [
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "API",
        "AUTH",
        "CREDENTIAL",
    ]
    .iter()
    .any(|m| n.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::load(&dir.path().join("secrets.toml")).unwrap();
        assert!(store.get("ANYTHING").is_none());
    }

    #[test]
    fn set_save_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.toml");
        let mut store = SecretStore::default();
        store.set("OPENAI_API_KEY", "sk-123");
        store.save(&path).unwrap();

        let reloaded = SecretStore::load(&path).unwrap();
        assert_eq!(reloaded.get("OPENAI_API_KEY"), Some("sk-123"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn secret_name_detection() {
        assert!(looks_secret("OPENAI_API_KEY"));
        assert!(looks_secret("anthropic_token"));
        assert!(looks_secret("DB_PASSWORD"));
        assert!(!looks_secret("PORT"));
        assert!(!looks_secret("NODE_ENV"));
    }
}
