//! Package Intelligence — deterministic, offline inference about what a package
//! needs and how it runs (design `2026-07-25-smart-run-and-local-ui`).
//!
//! This is the keystone for the "smart run" experience: given an *extracted*
//! package directory, it reads the README and the actual source to infer which
//! LLM providers the code uses, which env vars it reads, whether it can run
//! free/locally, how it's meant to be driven (REPL vs one-shot vs server), and
//! any CLI subcommands — so the runtime can set up near-zero-config and show a
//! helpful banner.
//!
//! It works identically for registry packages and `xelian add <github>` repos
//! (which have no `xelian.toml`), because it operates on files, not the
//! manifest. The manifest, when available, only *enriches* the result (e.g. an
//! `mcp` package is a server).
//!
//! Everything here is heuristic and best-effort: inference only *adds*
//! affordances. If it finds nothing, the caller behaves exactly as before.
//! There is no LLM and no network access in v1 (design: offline/deterministic).

use std::collections::BTreeSet;
use std::path::Path;

use ignore::WalkBuilder;

use super::provider::Provider;
use crate::manifest::{Manifest, PackageType};

/// How a package expects to be driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStyle {
    /// Loops reading input — a chat/REPL agent.
    Repl,
    /// Parses argv, does one thing, exits — a command-line tool.
    OneShot,
    /// An MCP server speaking JSON-RPC over stdio.
    Server,
}

/// Structured, cached insights about an extracted package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageInsights {
    /// LLM providers the code appears to use, free/local first.
    pub providers: Vec<Provider>,
    /// Env vars the code actually reads (deduped, sorted).
    pub required_env: Vec<String>,
    /// True when the package can run without a paid key (no provider, or Ollama).
    pub runs_free_local: bool,
    /// How the package is meant to be driven.
    pub run_style: RunStyle,
    /// CLI subcommands inferred from argparse/click/commander definitions.
    pub subcommands: Vec<String>,
    /// A short usage/quickstart excerpt pulled from the README.
    pub usage: Option<String>,
}

impl PackageInsights {
    /// True when the package appears to need an LLM (so the runtime should
    /// offer a provider menu before launch).
    pub fn needs_model(&self) -> bool {
        !self.providers.is_empty()
    }
}

/// Source-file extensions worth scanning for provider/env/subcommand hints.
const SOURCE_EXTS: &[&str] = &["py", "js", "ts", "mjs", "cjs", "tsx", "jsx"];

/// Skip individual files larger than this (generated/vendored blobs).
const MAX_FILE_BYTES: u64 = 512 * 1024;
/// Stop scanning once this much source has been read (bounded work).
const MAX_TOTAL_BYTES: usize = 4 * 1024 * 1024;

/// Inspect an extracted package directory and infer its insights.
///
/// `manifest` is optional; when present it enriches the result (an `mcp`
/// package is always a [`RunStyle::Server`], and declared `[environment]` vars
/// are folded into `required_env`).
pub fn inspect_package(dir: &Path, manifest: Option<&Manifest>) -> PackageInsights {
    let mut providers: Vec<Provider> = Vec::new();
    let mut env: BTreeSet<String> = BTreeSet::new();
    let mut subcommands: BTreeSet<String> = BTreeSet::new();
    let mut looks_repl = false;

    // Fold manifest-declared env vars in first (authoritative when present).
    if let Some(m) = manifest {
        for name in m.environment.keys() {
            env.insert(name.clone());
            if let Some(p) = Provider::from_env_var(name) {
                push_unique(&mut providers, p);
            }
        }
    }

    let mut scanned = 0usize;
    let walker = WalkBuilder::new(dir)
        .hidden(false)
        .git_ignore(true)
        .require_git(false)
        .build();

    for entry in walker.flatten() {
        if scanned >= MAX_TOTAL_BYTES {
            break;
        }
        let path = entry.path();
        if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        let is_source = ext.as_deref().is_some_and(|e| SOURCE_EXTS.contains(&e));
        if !is_source {
            continue;
        }
        if entry
            .metadata()
            .map(|m| m.len() > MAX_FILE_BYTES)
            .unwrap_or(true)
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        scanned += text.len();

        scan_source(
            &text,
            &mut providers,
            &mut env,
            &mut subcommands,
            &mut looks_repl,
        );
    }

    // Providers implied by env vars we collected but didn't see imported.
    for name in &env {
        if let Some(p) = Provider::from_env_var(name) {
            push_unique(&mut providers, p);
        }
    }
    order_providers(&mut providers);

    let usage = read_usage(dir);

    let run_style = if matches!(manifest.map(|m| m.package_type), Some(PackageType::Mcp)) {
        RunStyle::Server
    } else if !subcommands.is_empty() {
        RunStyle::OneShot
    } else if looks_repl {
        RunStyle::Repl
    } else {
        // Default to REPL: agents without an obvious CLI are chat-driven, and a
        // warm REPL is the friendlier default for the user.
        RunStyle::Repl
    };

    let runs_free_local = providers.is_empty() || providers.contains(&Provider::Ollama);

    PackageInsights {
        providers,
        required_env: env.into_iter().collect(),
        runs_free_local,
        run_style,
        subcommands: subcommands.into_iter().collect(),
        usage,
    }
}

/// Scan a single source file's text, updating the accumulators in place.
fn scan_source(
    text: &str,
    providers: &mut Vec<Provider>,
    env: &mut BTreeSet<String>,
    subcommands: &mut BTreeSet<String>,
    looks_repl: &mut bool,
) {
    // --- env var reads ---
    for anchor in [
        "os.getenv(",
        "os.environ.get(",
        "os.environ[",
        "getenv(",
        "process.env[",
    ] {
        for name in extract_quoted_after(text, anchor) {
            if is_env_name(&name) {
                env.insert(name);
            }
        }
    }
    // `process.env.NAME` (dotted access, no quotes)
    for name in extract_ident_after(text, "process.env.") {
        if is_env_name(&name) {
            env.insert(name);
        }
    }

    // --- provider imports ---
    for module in extract_imports(text) {
        if let Some(p) = Provider::from_import(&module) {
            push_unique(providers, p);
        }
    }

    // --- CLI subcommands (argparse / click / commander) ---
    for anchor in ["add_parser(", ".command(", "add_command("] {
        for name in extract_quoted_after(text, anchor) {
            if is_subcommand_name(&name) {
                subcommands.insert(name);
            }
        }
    }

    // --- REPL heuristic: an input loop ---
    if (text.contains("while True") && (text.contains("input(") || text.contains("sys.stdin")))
        || text.contains("readline") && text.contains("createInterface")
    {
        *looks_repl = true;
    }
}

/// Extract the first quoted string literal immediately following each
/// occurrence of `anchor` (handles both `"` and `'`).
fn extract_quoted_after(text: &str, anchor: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut search = text;
    while let Some(pos) = search.find(anchor) {
        let after = &search[pos + anchor.len()..];
        let after = after.trim_start();
        if let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') {
            let rest = &after[1..];
            if let Some(end) = rest.find(quote) {
                out.push(rest[..end].to_string());
            }
        }
        search = &search[pos + anchor.len()..];
    }
    out
}

/// Extract identifier tokens immediately following each occurrence of `anchor`
/// (e.g. `process.env.OPENAI_API_KEY`).
fn extract_ident_after(text: &str, anchor: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut search = text;
    while let Some(pos) = search.find(anchor) {
        let after = &search[pos + anchor.len()..];
        let ident: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !ident.is_empty() {
            out.push(ident);
        }
        search = &search[pos + anchor.len()..];
    }
    out
}

/// Collect imported module names from Python (`import x`, `from x import`) and
/// JS/TS (`from 'x'`, `require('x')`) source.
fn extract_imports(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("from ") {
            if let Some(module) = rest.split_whitespace().next() {
                out.push(module.to_string());
            }
        } else if let Some(rest) = l.strip_prefix("import ") {
            if let Some(module) = rest.split([' ', ',', ';']).next() {
                out.push(module.to_string());
            }
        }
    }
    // JS/TS `from '...'` and `require('...')`.
    out.extend(extract_quoted_after(text, "from "));
    out.extend(extract_quoted_after(text, "require("));
    out
}

/// Pull a usage/quickstart excerpt out of a README, if one exists.
fn read_usage(dir: &Path) -> Option<String> {
    let readme = ["README.md", "README.MD", "readme.md", "Readme.md"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())?;
    let text = std::fs::read_to_string(readme).ok()?;
    extract_usage_section(&text)
}

/// Given README markdown, return the first "usage"/"quickstart"/"getting
/// started"/"example" section (heading + body up to the next same-or-higher
/// heading), capped in length. Falls back to the first fenced code block.
fn extract_usage_section(md: &str) -> Option<String> {
    const KEYS: &[&str] = &[
        "usage",
        "quickstart",
        "quick start",
        "getting started",
        "example",
        "run",
    ];
    let lines: Vec<&str> = md.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some(level) = heading_level(trimmed) {
            let title = trimmed.trim_start_matches('#').trim().to_ascii_lowercase();
            if KEYS.iter().any(|k| title.contains(k)) {
                // Collect until the next heading of the same or higher level.
                let mut body = String::new();
                for next in &lines[i..] {
                    let nt = next.trim_start();
                    if body.is_empty() {
                        body.push_str(next);
                        body.push('\n');
                        continue;
                    }
                    if let Some(nl) = heading_level(nt) {
                        if nl <= level {
                            break;
                        }
                    }
                    body.push_str(next);
                    body.push('\n');
                    if body.len() > 1500 {
                        break;
                    }
                }
                return Some(body.trim_end().to_string());
            }
        }
    }

    // Fallback: first fenced code block.
    let start = md.find("```")?;
    let rest = &md[start + 3..];
    let end = rest.find("```")?;
    let block = rest[..end].trim();
    if block.is_empty() {
        None
    } else {
        Some(format!("```\n{block}\n```"))
    }
}

/// Markdown ATX heading level (`#` count) if `line` is a heading, else `None`.
fn heading_level(line: &str) -> Option<usize> {
    if !line.starts_with('#') {
        return None;
    }
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        Some(hashes)
    } else {
        None
    }
}

/// A plausible env-var name: uppercase-ish, reasonable length, no spaces.
fn is_env_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.chars().any(|c| c.is_ascii_uppercase())
}

/// A plausible CLI subcommand name (not a flag, not a path, short).
fn is_subcommand_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':')
}

fn push_unique(providers: &mut Vec<Provider>, p: Provider) {
    if !providers.contains(&p) {
        providers.push(p);
    }
}

/// Order providers free/local-first, then by [`Provider::all`] order.
fn order_providers(providers: &mut [Provider]) {
    let rank = |p: &Provider| {
        Provider::all()
            .iter()
            .position(|x| x == p)
            .unwrap_or(usize::MAX)
    };
    providers.sort_by_key(rank);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, contents: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn detects_openai_python_agent_env_and_provider() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "main.py",
            r#"
import os
import openai

key = os.getenv("OPENAI_API_KEY")
model = os.environ.get("MODEL_NAME")
"#,
        );
        let insights = inspect_package(tmp.path(), None);
        assert!(insights.providers.contains(&Provider::OpenAI));
        assert!(insights
            .required_env
            .contains(&"OPENAI_API_KEY".to_string()));
        assert!(insights.required_env.contains(&"MODEL_NAME".to_string()));
        assert!(insights.needs_model());
        assert!(!insights.runs_free_local);
    }

    #[test]
    fn detects_node_process_env() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "index.js",
            r#"
const key = process.env.ANTHROPIC_API_KEY;
const other = process.env["DB_URL"];
const { Anthropic } = require("anthropic");
"#,
        );
        let insights = inspect_package(tmp.path(), None);
        assert!(insights.providers.contains(&Provider::Anthropic));
        assert!(insights
            .required_env
            .contains(&"ANTHROPIC_API_KEY".to_string()));
        assert!(insights.required_env.contains(&"DB_URL".to_string()));
    }

    #[test]
    fn detects_argparse_subcommands_as_oneshot() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "cli.py",
            r#"
import argparse
p = argparse.ArgumentParser()
sub = p.add_subparsers()
sub.add_parser("encode")
sub.add_parser("decode")
"#,
        );
        let insights = inspect_package(tmp.path(), None);
        assert_eq!(insights.run_style, RunStyle::OneShot);
        assert!(insights.subcommands.contains(&"encode".to_string()));
        assert!(insights.subcommands.contains(&"decode".to_string()));
    }

    #[test]
    fn free_local_when_ollama_or_no_provider() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "app.py", "import ollama\nollama.chat()\n");
        let insights = inspect_package(tmp.path(), None);
        assert!(insights.providers.contains(&Provider::Ollama));
        assert!(insights.runs_free_local);

        let tmp2 = TempDir::new().unwrap();
        write(tmp2.path(), "app.py", "print('hello, no llm here')\n");
        let insights2 = inspect_package(tmp2.path(), None);
        assert!(insights2.providers.is_empty());
        assert!(insights2.runs_free_local);
        assert!(!insights2.needs_model());
    }

    #[test]
    fn detects_repl_input_loop() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "chat.py",
            "while True:\n    line = input('> ')\n    print(line)\n",
        );
        let insights = inspect_package(tmp.path(), None);
        assert_eq!(insights.run_style, RunStyle::Repl);
    }

    #[test]
    fn extracts_usage_section_from_readme() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "README.md",
            "# Cool Tool\n\nIntro text.\n\n## Usage\n\nRun `tool encode foo`.\n\n## License\n\nMIT\n",
        );
        let insights = inspect_package(tmp.path(), None);
        let usage = insights.usage.expect("usage section");
        assert!(usage.contains("Usage"));
        assert!(usage.contains("tool encode foo"));
        assert!(!usage.contains("License"));
    }

    #[test]
    fn usage_falls_back_to_first_code_block() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "README.md",
            "# Title\n\nNo usage heading here.\n\n```\nxelian run me\n```\n",
        );
        let insights = inspect_package(tmp.path(), None);
        assert!(insights.usage.unwrap().contains("xelian run me"));
    }
}
