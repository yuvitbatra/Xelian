//! HTTP client for the Harbor registry API (SPEC.md §14.8).
//!
//! Wraps `ureq` to provide typed methods for registry operations:
//! authentication, publishing, etc.

use std::io::{Read, Write};
use std::path::Path;

use serde::Deserialize;
use thiserror::Error;

use crate::auth::StoredCredentials;

/// Response from `POST /auth/token`.
#[derive(Debug, Clone, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub username: String,
}

/// Response from `POST /packages`.
#[derive(Debug, Clone, Deserialize)]
pub struct PublishResponse {
    pub ok: bool,
    pub name: String,
    pub version: String,
}

/// One version record as returned by `GET /packages/{owner}/{package}`.
#[derive(Debug, Clone, Deserialize)]
pub struct VersionRecordResponse {
    pub version: String,
    pub checksum: String,
    pub published_at: String,
    pub yanked: Option<bool>,
}

/// Response from `GET /packages/{owner}/{package}` (SPEC.md §14.8).
#[derive(Debug, Clone, Deserialize)]
pub struct PackageInfoResponse {
    pub owner: String,
    pub name: String,
    pub latest_version: Option<String>,
    pub description: String,
    pub package_type: String,
    pub language: String,
    pub runtime: String,
    pub entrypoint: String,
    pub license: String,
    pub permissions: Vec<String>,
    pub features: Vec<String>,
    pub versions: Vec<VersionRecordResponse>,
}

/// Error response from the registry.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiError {
    pub detail: Option<String>,
}

/// Errors that can occur during registry operations.
#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("network error: {0}")]
    Network(String),

    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("registry returned {status}: {message}")]
    Api { status: u16, message: String },

    #[error("failed to read file for upload: {0}")]
    FileIo(#[from] std::io::Error),

    #[error("failed to parse registry response: {0}")]
    ResponseParse(String),
}

/// A typed HTTP client for the Harbor registry API.
pub struct RegistryClient {
    /// Base URL of the registry (e.g. `http://localhost:8000`).
    pub base_url: String,
}

impl RegistryClient {
    /// Create a new client targeting the given registry base URL.
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Authenticate with username/password and return an auth token.
    ///
    /// POSTs `{"username": ..., "password": ...}` to `/auth/token`.
    pub fn login(&self, username: &str, password: &str) -> Result<LoginResponse, RegistryError> {
        let url = format!("{}/auth/token", self.base_url);
        let body = serde_json::json!({ "username": username, "password": password });

        let response = ureq::post(&url)
            .set("Content-Type", "application/json")
            .send_json(body)
            .map_err(map_ureq_error)?;

        response
            .into_json::<LoginResponse>()
            .map_err(|e| RegistryError::ResponseParse(e.to_string()))
    }

    /// Fetch metadata for a package from the registry (SPEC.md §14.8).
    ///
    /// Calls `GET /packages/{owner}/{package}` and returns the resolved
    /// package info including the latest non-yanked version's metadata.
    pub fn fetch_metadata(
        &self,
        owner: &str,
        name: &str,
    ) -> Result<PackageInfoResponse, RegistryError> {
        let url = format!("{}/packages/{}/{}", self.base_url, owner, name);

        let response = ureq::get(&url).call().map_err(map_ureq_error)?;

        response
            .into_json::<PackageInfoResponse>()
            .map_err(|e| RegistryError::ResponseParse(e.to_string()))
    }

    /// Download a specific version's archive (SPEC.md §14.8).
    ///
    /// Calls `GET /download/{owner}/{package}/{version}` and returns the
    /// raw archive bytes.
    pub fn download_archive(
        &self,
        owner: &str,
        name: &str,
        version: &str,
    ) -> Result<Vec<u8>, RegistryError> {
        let url = format!("{}/download/{}/{}/{}", self.base_url, owner, name, version);

        let response = ureq::get(&url).call().map_err(map_ureq_error)?;

        let mut body: Vec<u8> = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut body)
            .map_err(|e| RegistryError::Network(e.to_string()))?;
        Ok(body)
    }

    /// Yank or unyank a specific version (SPEC.md §14.7, Phase 17 / H-170).
    ///
    /// Calls `PATCH /packages/{owner}/{package}/{version}` with `{"yanked": true/false}`.
    /// Requires authentication as the package owner (§14.4).
    pub fn yank(
        &self,
        creds: &StoredCredentials,
        owner: &str,
        name: &str,
        version: &str,
        yanked: bool,
    ) -> Result<(), RegistryError> {
        let url = format!("{}/packages/{}/{}/{}", self.base_url, owner, name, version);
        let body = serde_json::json!({ "yanked": yanked });

        match ureq::patch(&url)
            .set("Content-Type", "application/json")
            .set("Authorization", &format!("Bearer {}", creds.token))
            .send_json(body)
        {
            Ok(_) => Ok(()),
            Err(e) => Err(map_ureq_error(e)),
        }
    }

    /// Publish a package to the registry.
    ///
    /// Uploads the `.harbor` archive and its `harbor.lock` to
    /// `POST /packages` as multipart form data, authenticated with the
    /// provided token.
    pub fn publish(
        &self,
        creds: &StoredCredentials,
        owner: &str,
        name: &str,
        archive_path: &Path,
        lockfile_path: &Path,
    ) -> Result<PublishResponse, RegistryError> {
        let url = format!("{}/packages", self.base_url);

        // Read files into memory for upload.
        let archive_bytes = std::fs::read(archive_path)?;
        let lockfile_bytes = std::fs::read(lockfile_path)?;

        // Build multipart form body manually.
        let boundary = format!("harbor-upload-{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0));
        let mut body = Vec::new();

        // Text fields.
        let text_fields = vec![
            ("owner", owner),
            ("name", name),
        ];
        for (field_name, field_value) in &text_fields {
            write_field(&mut body, &boundary, field_name, None, field_value.as_bytes());
        }

        // Archive file.
        write_field(
            &mut body,
            &boundary,
            "archive",
            Some(("archive.harbor", "application/octet-stream")),
            &archive_bytes,
        );

        // Lockfile file.
        write_field(
            &mut body,
            &boundary,
            "lockfile",
            Some(("harbor.lock", "application/toml")),
            &lockfile_bytes,
        );

        // Closing boundary.
        write!(body, "--{boundary}--\r\n").unwrap();

        let content_type = format!("multipart/form-data; boundary={boundary}");

        let response = ureq::post(&url)
            .set("Content-Type", &content_type)
            .set("Authorization", &format!("Bearer {}", creds.token))
            .send_bytes(&body)
            .map_err(map_ureq_error)?;

        // Success is 2xx (201 Created); `ureq` surfaces every 4xx/5xx as
        // `Err(Error::Status(..))`, already handled by `map_ureq_error` above.
        response
            .into_json::<PublishResponse>()
            .map_err(|e| RegistryError::ResponseParse(e.to_string()))
    }
}

/// Convert a `ureq` request error into a typed [`RegistryError`].
///
/// `ureq` 2.x returns `Err(ureq::Error::Status(code, response))` for every
/// 4xx/5xx HTTP response (not an `Ok`), so status-code handling MUST live on
/// the error path — otherwise a 401/403/409/422 is silently misreported as a
/// generic network failure. Connection-level failures map to
/// [`RegistryError::Network`]; HTTP status failures map to
/// [`RegistryError::Auth`] (401) or [`RegistryError::Api`] (all others), with
/// the human-readable `detail` extracted from the JSON error body when present.
fn map_ureq_error(err: ureq::Error) -> RegistryError {
    match err {
        ureq::Error::Status(code, response) => {
            let message = response
                .into_string()
                .ok()
                .and_then(|body| serde_json::from_str::<ApiError>(&body).ok())
                .and_then(|e| e.detail)
                .unwrap_or_else(|| format!("registry returned status {code}"));
            if code == 401 {
                RegistryError::Auth(message)
            } else {
                RegistryError::Api {
                    status: code,
                    message,
                }
            }
        }
        ureq::Error::Transport(transport) => RegistryError::Network(transport.to_string()),
    }
}

/// Write a multipart form-data field to the body buffer.
fn write_field(
    body: &mut Vec<u8>,
    boundary: &str,
    field_name: &str,
    file_meta: Option<(&str, &str)>, // (filename, content_type)
    data: &[u8],
) {
    write!(body, "--{boundary}\r\n").unwrap();
    match file_meta {
        Some((filename, content_type)) => {
            write!(
                body,
                "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\n",
                field_name, filename
            )
            .unwrap();
            write!(body, "Content-Type: {}\r\n\r\n", content_type).unwrap();
        }
        None => {
            write!(
                body,
                "Content-Disposition: form-data; name=\"{}\"\r\n\r\n",
                field_name
            )
            .unwrap();
        }
    }
    body.extend_from_slice(data);
    write!(body, "\r\n").unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid .harbor archive in memory for testing.
    #[allow(dead_code)]
    fn build_test_archive(version: &str, name: &str) -> (Vec<u8>, Vec<u8>) {
        let manifest_toml = format!(
            r#"
spec-version = 1
name = "{name}"
version = "{version}"
description = "Test"
package-type = "agent"
language = "python"
runtime = ">=3.11"
entrypoint = "src/main.py"
license = "MIT"
permissions = []
features = []

[author]
name = "Test"
email = "test@example.com"

[dependencies]
manifest = "pyproject.toml"
"#
        );

        let mut archive_buf = Vec::new();
        {
            let encoder =
                flate2::write::GzEncoder::new(&mut archive_buf, flate2::Compression::default());
            let mut tar_builder = tar::Builder::new(encoder);

            let mut add_file = |path: &str, content: &[u8]| {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(content.len() as u64);
                header.set_mode(0o644);
                header.set_mtime(0);
                header.set_uid(0);
                header.set_gid(0);
                header.set_cksum();
                tar_builder
                    .append_data(&mut header, path, content)
                    .unwrap();
            };

            add_file("harbor.toml", manifest_toml.as_bytes());
            add_file("README.md", b"# Test\n");
            add_file("LICENSE", b"MIT\n");

            let encoder = tar_builder.into_inner().unwrap();
            encoder.finish().unwrap();
        }

        let checksum = crate::checksum::sha256_hex(&archive_buf);
        let lockfile_toml = format!(
            r#"
spec-version = 1
harbor-version = "0.1.0"
package-version = "{version}"
generated-at = "2026-07-17T00:00:00Z"
native-manifest = "pyproject.toml"
native-lockfile = "uv.lock"
native-lock-checksum = "sha256:0000000000000000000000000000000000000000000000000000000000000000"
package-checksum = "{checksum}"
"#
        );

        (archive_buf, lockfile_toml.as_bytes().to_vec())
    }

    #[test]
    fn client_constructs_correct_urls() {
        let client = RegistryClient::new("http://localhost:8000");
        assert_eq!(client.base_url, "http://localhost:8000");

        let client2 = RegistryClient::new("http://localhost:8000/");
        assert_eq!(client2.base_url, "http://localhost:8000");
    }

    #[test]
    fn build_multipart_body_is_well_formed() {
        // Test the multipart body builder by constructing one and verifying
        // it contains the required boundary markers.
        let boundary = "test-boundary-123";
        let mut body = Vec::new();

        write_field(
            &mut body,
            boundary,
            "owner",
            None,
            b"testuser",
        );
        write_field(
            &mut body,
            boundary,
            "archive",
            Some(("pkg.harbor", "application/octet-stream")),
            b"fake-archive-bytes",
        );
        write!(body, "--{boundary}--\r\n").unwrap();

        let as_str = String::from_utf8(body).unwrap();
        assert!(as_str.contains("--test-boundary-123"));
        assert!(as_str.contains("Content-Disposition: form-data; name=\"owner\""));
        assert!(as_str.contains(
            "Content-Disposition: form-data; name=\"archive\"; filename=\"pkg.harbor\""
        ));
        assert!(as_str.contains("Content-Type: application/octet-stream"));
        assert!(as_str.contains("fake-archive-bytes"));
        assert!(as_str.contains("--test-boundary-123--"));
    }

    #[test]
    fn login_with_nonexistent_server_returns_network_error() {
        let client = RegistryClient::new("http://127.0.0.1:1");
        let result = client.login("admin", "admin");
        assert!(
            matches!(result, Err(RegistryError::Network(_))),
            "expected Network error, got: {result:?}"
        );
    }
}
