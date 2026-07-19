//! The `xelian.toml` manifest: structs, parsing, and validation (SPEC.md §6).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::errors::{ManifestError, ValidationError, ValidationWarning};

/// Closed enum of permission scopes (SPEC.md §16.1). A manifest declaring a
/// `permissions` value outside this list MUST be rejected.
pub const ALLOWED_PERMISSIONS: &[&str] = &[
    "filesystem",
    "network",
    "camera",
    "microphone",
    "clipboard",
    "location",
    "notifications",
];

/// Closed list of capability tags (SPEC.md §17). A manifest declaring a
/// `features` value outside this list produces a warning, not an error.
pub const ALLOWED_FEATURES: &[&str] = &[
    "vision",
    "voice",
    "streaming",
    "memory",
    "tools",
    "reasoning",
    "multimodal",
    "embeddings",
];

/// Recognized operating system identifiers (SPEC.md §6.2, §9.6.1).
pub const ALLOWED_OS: &[&str] = &["linux", "macos", "windows"];

/// `package-type` (SPEC.md §6.1, §5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageType {
    Agent,
    Mcp,
}

impl std::fmt::Display for PackageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageType::Agent => write!(f, "agent"),
            PackageType::Mcp => write!(f, "mcp"),
        }
    }
}

/// `language` (SPEC.md §6.1) — determines which runtime manager Xelian invokes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Python,
    Node,
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Language::Python => write!(f, "python"),
            Language::Node => write!(f, "node"),
        }
    }
}

/// `[author]` table (SPEC.md §6.1.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    pub name: String,
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
}

/// `[dependencies]` table (SPEC.md §6.1.2) — a pointer to the package's
/// native dependency manifest/lockfile, never a redeclaration of versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependencies {
    pub manifest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lockfile: Option<String>,
}

/// One entry in `[environment]` (SPEC.md §6.2.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVarSpec {
    #[serde(default)]
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// The full `xelian.toml` manifest (SPEC.md §6).
///
/// Unknown TOML keys are accepted (never `deny_unknown_fields`) for
/// forward-compatibility, and are never interpreted — see §6.3 for the
/// `[config]` escape hatch specifically.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Manifest {
    // --- required (§6.1) ---
    pub spec_version: u64,
    pub name: String,
    pub version: String,
    pub description: String,
    pub package_type: PackageType,
    pub language: Language,
    /// Native-ecosystem runtime constraint string. Captured verbatim; Xelian
    /// MUST NOT parse or validate it (§6.1) — that's delegated to `uv`/`npm`.
    pub runtime: String,
    pub entrypoint: String,
    pub author: Author,
    pub license: String,
    pub dependencies: Dependencies,
    pub permissions: Vec<String>,
    pub features: Vec<String>,

    // --- optional (§6.2) ---
    #[serde(default)]
    pub os: Vec<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub documentation: Option<String>,
    pub port: Option<u16>,
    pub primary_model: Option<String>,
    #[serde(default)]
    pub environment: BTreeMap<String, EnvVarSpec>,
    #[serde(default)]
    pub commands: BTreeMap<String, String>,
    #[serde(default)]
    pub tags: Vec<String>,

    /// `[config]` (§6.3) — opaque, never validated or interpreted by Xelian.
    #[serde(default)]
    pub config: toml::Table,
}

impl Manifest {
    /// Parse a `xelian.toml` document from its string contents.
    ///
    /// This performs schema-level parsing only (missing required fields or a
    /// malformed enum value produce a distinct `toml` error naming the
    /// field/value). It does NOT run the semantic checks in
    /// [`validate_manifest`] (spec-version support, SemVer, closed enums,
    /// naming rules, env conflicts) — call that separately.
    pub fn from_toml_str(s: &str) -> Result<Manifest, ManifestError> {
        let manifest: Manifest = toml::from_str(s)?;
        Ok(manifest)
    }
}

/// Run the full semantic validation pipeline for an already-parsed manifest
/// (SPEC.md §8.1 step 1, §9.6). Returns accumulated non-fatal warnings on
/// success; returns the first hard error encountered on failure.
pub fn validate_manifest(m: &Manifest) -> Result<Vec<ValidationWarning>, ValidationError> {
    // spec-version (§8.1 step 1, §9.6)
    if !crate::SUPPORTED_SPEC_VERSIONS.contains(&m.spec_version) {
        return Err(ValidationError::UnsupportedSpecVersion {
            found: m.spec_version,
            supported: crate::SUPPORTED_SPEC_VERSIONS.to_vec(),
        });
    }

    // version must be valid SemVer 2.0.0 (§19.1)
    semver::Version::parse(&m.version).map_err(|e| ValidationError::InvalidSemVer {
        version: m.version.clone(),
        reason: e.to_string(),
    })?;

    // permissions: closed enum, hard error (§16.1)
    for p in &m.permissions {
        if !ALLOWED_PERMISSIONS.contains(&p.as_str()) {
            return Err(ValidationError::InvalidPermission {
                value: p.clone(),
                allowed: ALLOWED_PERMISSIONS,
            });
        }
    }

    // os: recognized set, hard error (§9.6.1)
    for o in &m.os {
        if !ALLOWED_OS.contains(&o.as_str()) {
            return Err(ValidationError::UnrecognizedOs {
                value: o.clone(),
                allowed: ALLOWED_OS,
            });
        }
    }

    // name: charset + length (§19.3)
    let valid_charset = !m.name.is_empty()
        && m
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if !valid_charset {
        return Err(ValidationError::InvalidNameCharset { name: m.name.clone() });
    }
    if m.name.len() < 3 || m.name.len() > 64 {
        return Err(ValidationError::InvalidNameLength {
            name: m.name.clone(),
            len: m.name.len(),
        });
    }

    // environment: required = true with a default is a conflict (§6.2.1)
    for (var, spec) in &m.environment {
        if spec.required && spec.default.is_some() {
            return Err(ValidationError::EnvRequiredWithDefault { var: var.clone() });
        }
    }

    // features: closed list, warning only (§17)
    let mut warnings = Vec::new();
    for f in &m.features {
        if !ALLOWED_FEATURES.contains(&f.as_str()) {
            warnings.push(ValidationWarning::UnrecognizedFeature {
                value: f.clone(),
                allowed: ALLOWED_FEATURES,
            });
        }
    }

    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal, fully valid manifest string. Individual tests patch one
    /// field/line at a time via `.replace(...)` to isolate the rule under test.
    fn valid_toml() -> String {
        r#"
spec-version = 1
name = "my-agent"
version = "1.0.0"
description = "A test agent."
package-type = "agent"
language = "python"
runtime = ">=3.11,<4"
entrypoint = "src/main.py"
license = "MIT"
permissions = ["network"]
features = ["tools"]

[author]
name = "Jane Doe"
email = "jane@example.com"

[dependencies]
manifest = "pyproject.toml"
lockfile = "uv.lock"
"#
        .to_string()
    }

    fn parse_and_validate(toml_str: &str) -> Result<Vec<ValidationWarning>, String> {
        let manifest = Manifest::from_toml_str(toml_str).map_err(|e| e.to_string())?;
        validate_manifest(&manifest).map_err(|e| e.to_string())
    }

    // ---- H-010: struct/parse ----

    #[test]
    fn parses_minimal_valid_manifest() {
        let m = Manifest::from_toml_str(&valid_toml()).expect("should parse");
        assert_eq!(m.spec_version, 1);
        assert_eq!(m.name, "my-agent");
        assert_eq!(m.package_type, PackageType::Agent);
        assert_eq!(m.language, Language::Python);
        assert_eq!(m.author.name, "Jane Doe");
        assert_eq!(m.dependencies.manifest, "pyproject.toml");
        assert!(m.os.is_empty());
        assert!(m.environment.is_empty());
        assert!(m.config.is_empty());
    }

    #[test]
    fn missing_required_field_is_distinct_error() {
        let bad = valid_toml().replace("name = \"my-agent\"\n", "");
        let err = Manifest::from_toml_str(&bad).unwrap_err().to_string();
        assert!(
            err.contains("name"),
            "error should name the missing field, got: {err}"
        );
    }

    #[test]
    fn invalid_package_type_is_distinct_error() {
        let bad = valid_toml().replace("package-type = \"agent\"", "package-type = \"bogus\"");
        let err = Manifest::from_toml_str(&bad);
        assert!(err.is_err());
    }

    #[test]
    fn unknown_top_level_keys_are_accepted() {
        let with_unknown = format!("{}\nsome_future_field = \"whatever\"\n", valid_toml());
        Manifest::from_toml_str(&with_unknown).expect("unknown keys must not be rejected");
    }

    #[test]
    fn config_section_is_captured_opaquely() {
        let with_config = format!(
            "{}\n[config]\nmax_tokens = 4096\nsystem_prompt_file = \"prompts/system.md\"\n",
            valid_toml()
        );
        let m = Manifest::from_toml_str(&with_config).expect("should parse");
        assert_eq!(
            m.config.get("max_tokens").and_then(|v| v.as_integer()),
            Some(4096)
        );
    }

    // ---- H-011: spec-version + semver ----

    #[test]
    fn valid_spec_version_passes() {
        assert!(parse_and_validate(&valid_toml()).is_ok());
    }

    #[test]
    fn unsupported_spec_version_is_rejected() {
        let bad = valid_toml().replace("spec-version = 1", "spec-version = 99");
        let err = parse_and_validate(&bad).unwrap_err();
        assert!(err.contains("99"), "got: {err}");
    }

    #[test]
    fn valid_semver_passes() {
        let ok = valid_toml().replace("version = \"1.0.0\"", "version = \"2.3.4-beta.1+build5\"");
        assert!(parse_and_validate(&ok).is_ok());
    }

    #[test]
    fn invalid_semver_is_rejected() {
        let bad = valid_toml().replace("version = \"1.0.0\"", "version = \"not-a-version\"");
        let err = parse_and_validate(&bad).unwrap_err();
        assert!(err.contains("not-a-version"), "got: {err}");
    }

    // ---- H-012: closed-enum validation ----

    #[test]
    fn valid_permission_passes() {
        let ok = valid_toml().replace("permissions = [\"network\"]", "permissions = [\"filesystem\", \"network\"]");
        assert!(parse_and_validate(&ok).is_ok());
    }

    #[test]
    fn invalid_permission_is_rejected() {
        let bad = valid_toml().replace("permissions = [\"network\"]", "permissions = [\"telepathy\"]");
        let err = parse_and_validate(&bad).unwrap_err();
        assert!(err.contains("telepathy"), "got: {err}");
    }

    #[test]
    fn valid_os_passes() {
        // Inserted before [author] (a top-level key), not appended at the
        // end — appending after [dependencies] would land inside that table
        // (the same textual-nesting quirk as the golden §6.4 example) and
        // never reach `Manifest::os` at all.
        let ok = valid_toml().replace(
            "[author]",
            "os = [\"linux\", \"macos\"]\n\n[author]",
        );
        assert!(parse_and_validate(&ok).is_ok());
    }

    #[test]
    fn unrecognized_os_is_rejected() {
        let bad = valid_toml().replace(
            "[author]",
            "os = [\"amigaos\"]\n\n[author]",
        );
        let err = parse_and_validate(&bad).unwrap_err();
        assert!(err.contains("amigaos"), "got: {err}");
    }

    #[test]
    fn recognized_feature_produces_no_warning() {
        let warnings = parse_and_validate(&valid_toml()).unwrap();
        assert!(warnings.is_empty());
    }

    #[test]
    fn unrecognized_feature_produces_warning_not_error() {
        let with_bad_feature =
            valid_toml().replace("features = [\"tools\"]", "features = [\"tools\", \"telekinesis\"]");
        let warnings = parse_and_validate(&with_bad_feature).expect("must not be a hard error");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].to_string().contains("telekinesis"));
    }

    // ---- H-013: naming + environment conflict ----

    #[test]
    fn valid_name_passes() {
        let ok = valid_toml().replace("name = \"my-agent\"", "name = \"weather_agent-2\"");
        assert!(parse_and_validate(&ok).is_ok());
    }

    #[test]
    fn name_with_bad_charset_is_rejected() {
        let bad = valid_toml().replace("name = \"my-agent\"", "name = \"My Agent!\"");
        let err = parse_and_validate(&bad).unwrap_err();
        assert!(err.contains("My Agent!"), "got: {err}");
    }

    #[test]
    fn name_too_short_is_rejected() {
        let bad = valid_toml().replace("name = \"my-agent\"", "name = \"ab\"");
        let err = parse_and_validate(&bad).unwrap_err();
        assert!(err.contains("ab"), "got: {err}");
    }

    #[test]
    fn name_too_long_is_rejected() {
        let long_name = "a".repeat(65);
        let bad = valid_toml().replace("name = \"my-agent\"", &format!("name = \"{long_name}\""));
        let err = parse_and_validate(&bad).unwrap_err();
        assert!(err.contains("65"), "got: {err}");
    }

    #[test]
    fn environment_without_conflict_passes() {
        let ok = format!(
            "{}\n[environment]\nSERPAPI_KEY = {{ required = true }}\nDEBUG = {{ default = \"false\" }}\n",
            valid_toml()
        );
        assert!(parse_and_validate(&ok).is_ok());
    }

    #[test]
    fn environment_required_with_default_is_rejected() {
        let bad = format!(
            "{}\n[environment]\nOOPS_KEY = {{ required = true, default = \"x\" }}\n",
            valid_toml()
        );
        let err = parse_and_validate(&bad).unwrap_err();
        assert!(err.contains("OOPS_KEY"), "got: {err}");
    }

    // ---- H-014: golden test, full §6.4 example verbatim ----

    #[test]
    fn full_spec_example_parses_and_validates_with_zero_errors() {
        // Verbatim from SPEC.md §6.4, including the known quirk where
        // homepage/repository/primary-model/tags land inside [dependencies]
        // textually (and are therefore ignored as unknown fields there,
        // not applied to the top-level Manifest fields). That is the
        // documented acceptance criterion — not a bug to fix.
        let example = r#"
spec-version = 1
name = "research_assistant"
version = "1.2.0"
description = "An agent that searches papers and summarizes findings."
package-type = "agent"
language = "python"
runtime = "python>=3.11,<4"
entrypoint = "src/main.py"
license = "MIT"
permissions = ["network", "filesystem"]
features = ["tools", "streaming"]

[author]
name = "Jane Doe"
email = "jane@example.com"
homepage = "https://example.com"

[dependencies]
manifest = "pyproject.toml"
lockfile = "uv.lock"

homepage = "https://example.com/research-assistant"
repository = "https://github.com/janedoe/research-assistant"
primary-model = "llama3"
tags = ["research", "search", "summarization"]

[environment]
SERPAPI_KEY = { required = true }
DEBUG = { default = "false" }

[commands]
test = "pytest"
lint = "ruff check"

[config]
max_tokens = 4096
"#;

        let manifest = Manifest::from_toml_str(example).expect("golden example must parse");
        let warnings = validate_manifest(&manifest).expect("golden example must validate cleanly");
        assert!(warnings.is_empty());

        assert_eq!(manifest.name, "research_assistant");
        assert_eq!(manifest.dependencies.manifest, "pyproject.toml");
        assert_eq!(manifest.dependencies.lockfile.as_deref(), Some("uv.lock"));
        // Per the documented quirk: these were NOT top-level in the TOML
        // (they textually land inside [dependencies] and are ignored there).
        assert_eq!(manifest.primary_model, None);
        assert!(manifest.tags.is_empty());
        assert_eq!(
            manifest.environment.get("SERPAPI_KEY").map(|v| v.required),
            Some(true)
        );
        assert_eq!(manifest.commands.get("test").map(String::as_str), Some("pytest"));
        assert_eq!(
            manifest.config.get("max_tokens").and_then(|v| v.as_integer()),
            Some(4096)
        );
    }
}
