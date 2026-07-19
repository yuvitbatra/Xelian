//! Error types for manifest parsing and validation (SPEC.md §6, §8.1 step 1).

use thiserror::Error;

/// Errors that can occur while parsing `xelian.toml` into a [`crate::manifest::Manifest`].
#[derive(Debug, Error)]
pub enum ManifestError {
    /// The TOML itself is malformed, or a required field is missing/mistyped.
    /// `toml`'s own deserialization errors already name the offending field.
    #[error("failed to parse xelian.toml: {0}")]
    Parse(#[from] toml::de::Error),
}

/// A hard validation failure (SPEC.md §8.1 step 1). Each variant names the
/// specific rule that failed and the offending value, so the message is
/// actionable on its own.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("unsupported spec-version {found}: this xelian binary supports {supported:?}")]
    UnsupportedSpecVersion { found: u64, supported: Vec<u64> },

    #[error("invalid version {version:?}: must be valid SemVer 2.0.0 ({reason})")]
    InvalidSemVer { version: String, reason: String },

    #[error("invalid permission {value:?}: must be one of {allowed:?}")]
    InvalidPermission {
        value: String,
        allowed: &'static [&'static str],
    },

    #[error("unrecognized os {value:?}: must be one of {allowed:?}")]
    UnrecognizedOs {
        value: String,
        allowed: &'static [&'static str],
    },

    #[error(
        "invalid package name {name:?}: names must contain only lowercase ASCII letters, digits, '_', and '-'"
    )]
    InvalidNameCharset { name: String },

    #[error(
        "invalid package name {name:?}: names must be between 3 and 64 characters (got {len})"
    )]
    InvalidNameLength { name: String, len: usize },

    #[error(
        "environment variable {var:?} declares both required = true and a default value; a required variable must not have a default"
    )]
    EnvRequiredWithDefault { var: String },
}

/// A non-fatal validation warning (SPEC.md §17): unrecognized feature tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationWarning {
    UnrecognizedFeature {
        value: String,
        allowed: &'static [&'static str],
    },
}

impl std::fmt::Display for ValidationWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationWarning::UnrecognizedFeature { value, allowed } => write!(
                f,
                "unrecognized feature {value:?}: expected one of {allowed:?} (informational only, not rejected)"
            ),
        }
    }
}
