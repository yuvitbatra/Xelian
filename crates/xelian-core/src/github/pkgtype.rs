//! `package-type` inference for `xelian add` (SPEC.md §12.2 step 3).
//!
//! The distinction is not cosmetic: `agent` packages get an interactive REPL
//! with stdio attached to the terminal (§9.10.1), while `mcp` packages use
//! stdin/stdout as a JSON-RPC transport that must carry nothing but the
//! protocol (§9.10.2), and are the only type the gateway will accept. Typing
//! an MCP server as an agent — which is what a hardcoded default does —
//! mis-routes it at launch.
//!
//! Detection is by dependency first (authoritative: a package that depends on
//! an MCP SDK is an MCP server), then by name convention.

use std::path::Path;

use crate::manifest::{Language, PackageType};

/// Dependency names that identify an MCP server implementation.
const MCP_NODE_DEPS: &[&str] = &["@modelcontextprotocol/sdk", "fastmcp", "mcp-framework"];
const MCP_PYTHON_DEPS: &[&str] = &["mcp", "fastmcp", "mcp-server", "modelcontextprotocol"];

/// Infer whether a checkout is an MCP server or an agent.
pub fn infer(checkout: &Path, language: Language) -> PackageType {
    if depends_on_mcp_sdk(checkout, language) || name_suggests_mcp(checkout) {
        PackageType::Mcp
    } else {
        PackageType::Agent
    }
}

fn depends_on_mcp_sdk(checkout: &Path, language: Language) -> bool {
    match language {
        Language::Node => node_deps(checkout)
            .iter()
            .any(|d| MCP_NODE_DEPS.contains(&d.as_str())),
        Language::Python => python_deps(checkout)
            .iter()
            .any(|d| MCP_PYTHON_DEPS.contains(&d.as_str())),
    }
}

fn node_deps(checkout: &Path) -> Vec<String> {
    let Ok(contents) = std::fs::read_to_string(checkout.join("package.json")) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for section in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(map) = v.get(section).and_then(|d| d.as_object()) {
            out.extend(map.keys().cloned());
        }
    }
    out
}

/// Collect Python dependency names from `pyproject.toml` and
/// `requirements.txt`, stripped of version specifiers and extras.
fn python_deps(checkout: &Path) -> Vec<String> {
    let mut out = Vec::new();

    if let Ok(contents) = std::fs::read_to_string(checkout.join("pyproject.toml")) {
        if let Ok(v) = toml::from_str::<toml::Value>(&contents) {
            let deps = v
                .get("project")
                .and_then(|p| p.get("dependencies"))
                .and_then(|d| d.as_array());
            if let Some(deps) = deps {
                out.extend(deps.iter().filter_map(|d| d.as_str()).map(base_requirement));
            }
            // Poetry-style `[tool.poetry.dependencies]`.
            let poetry = v
                .get("tool")
                .and_then(|t| t.get("poetry"))
                .and_then(|p| p.get("dependencies"))
                .and_then(|d| d.as_table());
            if let Some(table) = poetry {
                out.extend(table.keys().map(|k| base_requirement(k)));
            }
        }
    }

    if let Ok(contents) = std::fs::read_to_string(checkout.join("requirements.txt")) {
        out.extend(
            contents
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('-'))
                .map(base_requirement),
        );
    }

    out
}

/// Reduce a PEP 508 requirement to its bare distribution name:
/// `mcp[cli]>=1.2,<2` → `mcp`.
fn base_requirement(req: &str) -> String {
    let name: String = req
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .collect();
    name.to_lowercase().replace('_', "-")
}

/// Whether the project's own declared name marks it as an MCP server.
///
/// Uses the declared package name rather than the directory name: an import
/// of `.../servers/tree/main/src/github` lands in a directory called `github`,
/// which says nothing, while its `package.json` name
/// (`@modelcontextprotocol/server-github`) does.
fn name_suggests_mcp(checkout: &Path) -> bool {
    let mut names: Vec<String> = Vec::new();

    if let Ok(c) = std::fs::read_to_string(checkout.join("package.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&c) {
            if let Some(n) = v.get("name").and_then(|n| n.as_str()) {
                names.push(n.to_lowercase());
            }
        }
    }
    if let Ok(c) = std::fs::read_to_string(checkout.join("pyproject.toml")) {
        if let Ok(v) = toml::from_str::<toml::Value>(&c) {
            if let Some(n) = v
                .get("project")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
            {
                names.push(n.to_lowercase());
            }
        }
    }

    // `@` is a separator too, so the scope of `@modelcontextprotocol/server-x`
    // is matched as a segment rather than as `@modelcontextprotocol`.
    names.iter().any(|n| {
        n.split(['/', '-', '_', '.', '@'])
            .any(|segment| segment == "mcp" || segment == "modelcontextprotocol")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(dir: &Path, name: &str, contents: &str) {
        fs::write(dir.join(name), contents).unwrap();
    }

    #[test]
    fn node_mcp_sdk_dependency_means_mcp() {
        let d = tempdir().unwrap();
        write(
            d.path(),
            "package.json",
            r#"{"name":"whatever","dependencies":{"@modelcontextprotocol/sdk":"^1.0.0"}}"#,
        );
        assert_eq!(infer(d.path(), Language::Node), PackageType::Mcp);
    }

    #[test]
    fn python_mcp_dependency_means_mcp() {
        let d = tempdir().unwrap();
        write(
            d.path(),
            "pyproject.toml",
            "[project]\nname = \"thing\"\ndependencies = [\"mcp[cli]>=1.2\", \"httpx\"]\n",
        );
        assert_eq!(infer(d.path(), Language::Python), PackageType::Mcp);
    }

    #[test]
    fn python_requirements_txt_mcp_dependency_means_mcp() {
        let d = tempdir().unwrap();
        write(d.path(), "requirements.txt", "httpx\nmcp>=1.0\n");
        assert_eq!(infer(d.path(), Language::Python), PackageType::Mcp);
    }

    #[test]
    fn scoped_server_name_means_mcp() {
        let d = tempdir().unwrap();
        write(
            d.path(),
            "package.json",
            r#"{"name":"@modelcontextprotocol/server-github"}"#,
        );
        assert_eq!(infer(d.path(), Language::Node), PackageType::Mcp);
    }

    #[test]
    fn mcp_in_the_name_means_mcp() {
        let d = tempdir().unwrap();
        write(d.path(), "package.json", r#"{"name":"firecrawl-mcp"}"#);
        assert_eq!(infer(d.path(), Language::Node), PackageType::Mcp);
    }

    #[test]
    fn a_plain_agent_stays_an_agent() {
        let d = tempdir().unwrap();
        write(
            d.path(),
            "pyproject.toml",
            "[project]\nname = \"crewai\"\ndependencies = [\"pydantic\", \"openai\"]\n",
        );
        assert_eq!(infer(d.path(), Language::Python), PackageType::Agent);
    }

    #[test]
    fn substring_mcp_does_not_false_positive() {
        // "mcpherson" must not read as MCP: matching is on name segments.
        let d = tempdir().unwrap();
        write(d.path(), "package.json", r#"{"name":"mcpherson-agent"}"#);
        assert_eq!(infer(d.path(), Language::Node), PackageType::Agent);
    }

    #[test]
    fn requirement_names_are_normalized() {
        assert_eq!(base_requirement("mcp[cli]>=1.2,<2"), "mcp");
        assert_eq!(base_requirement("  Flask == 2.0 "), "flask");
        assert_eq!(base_requirement("some_pkg"), "some-pkg");
    }
}
