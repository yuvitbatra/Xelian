//! LLM provider model for the "smart run" experience (design
//! `2026-07-25-smart-run-and-local-ui`).
//!
//! Xelian historically treats every credential as an opaque named env var
//! (see [`crate::secrets`]). This module adds a small, explicit notion of
//! *provider* so the runtime can offer a clean menu — "run this against local
//! Ollama for free, or paste an OpenAI/Anthropic key" — and know exactly which
//! env vars to set for each choice.
//!
//! The abstraction is deliberately tiny (design principle: keep it simple). A
//! provider is just: a display name, the env var that holds its API key (if
//! any), an optional base-URL var, and whether it runs free/locally. Detection
//! is heuristic and lives here so both the inference layer ([`super::inspect`])
//! and the CLI can share one source of truth.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

use thiserror::Error;

use super::inspect::PackageInsights;
use crate::secrets::{SecretStore, SecretsError};

/// An LLM provider an agent can be pointed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// Local, free, no API key. Detected as a running daemon on port 11434.
    Ollama,
    OpenAI,
    Anthropic,
}

impl Provider {
    /// Every provider Xelian knows about, in menu order (free/local first).
    pub fn all() -> [Provider; 3] {
        [Provider::Ollama, Provider::OpenAI, Provider::Anthropic]
    }

    /// Human-facing name shown in prompts and the UI.
    pub fn display_name(&self) -> &'static str {
        match self {
            Provider::Ollama => "Ollama",
            Provider::OpenAI => "OpenAI",
            Provider::Anthropic => "Anthropic",
        }
    }

    /// The env var that holds this provider's API key, or `None` for a
    /// key-less local provider (Ollama).
    pub fn key_var(&self) -> Option<&'static str> {
        match self {
            Provider::Ollama => None,
            Provider::OpenAI => Some("OPENAI_API_KEY"),
            Provider::Anthropic => Some("ANTHROPIC_API_KEY"),
        }
    }

    /// The env var an OpenAI/Anthropic-compatible client reads to override its
    /// base URL. Used to redirect a client at a local Ollama endpoint.
    pub fn base_url_var(&self) -> Option<&'static str> {
        match self {
            Provider::Ollama => None,
            Provider::OpenAI => Some("OPENAI_BASE_URL"),
            Provider::Anthropic => Some("ANTHROPIC_BASE_URL"),
        }
    }

    /// True when the provider runs locally with no key or cost (the "type
    /// nothing" path).
    pub fn is_local_free(&self) -> bool {
        matches!(self, Provider::Ollama)
    }

    /// Map an env-var name a package reads to the provider it implies, if any.
    pub fn from_env_var(name: &str) -> Option<Provider> {
        match name.to_ascii_uppercase().as_str() {
            "OPENAI_API_KEY" | "OPENAI_BASE_URL" | "OPENAI_API_BASE" => Some(Provider::OpenAI),
            "ANTHROPIC_API_KEY" | "ANTHROPIC_BASE_URL" => Some(Provider::Anthropic),
            "OLLAMA_HOST" | "OLLAMA_BASE_URL" => Some(Provider::Ollama),
            _ => None,
        }
    }

    /// Map an imported module/package name to the provider it implies, if any.
    /// Handles the common SDK and LangChain integration names.
    pub fn from_import(module: &str) -> Option<Provider> {
        let m = module.to_ascii_lowercase();
        // Take the top-level package (`langchain_openai.chat_models` -> ...).
        let head = m.split(['.', '/']).next().unwrap_or(&m);
        match head {
            "openai" | "langchain_openai" => Some(Provider::OpenAI),
            "anthropic" | "langchain_anthropic" => Some(Provider::Anthropic),
            "ollama" | "langchain_ollama" => Some(Provider::Ollama),
            _ => None,
        }
    }
}

/// Ollama's OpenAI-compatible endpoint, used to redirect an OpenAI-SDK client
/// at a local model for the free path.
pub const OLLAMA_OPENAI_BASE_URL: &str = "http://localhost:11434/v1";

/// Detect a *running* Ollama daemon (not just the installed binary) by hitting
/// its HTTP API with a short timeout. Returns `false` on any error/timeout, so
/// a missing or slow daemon simply means "not available" rather than blocking.
pub fn ollama_running() -> bool {
    let agent = ureq::builder().timeout(Duration::from_millis(400)).build();
    agent.get("http://localhost:11434/api/tags").call().is_ok()
}

/// Default local model to serve via Ollama when the free path is chosen for an
/// agent that doesn't declare its own model.
pub const DEFAULT_OLLAMA_MODEL: &str = "llama3.2";

/// Errors from interactive provider setup.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("failed to read input: {0}")]
    Io(String),
    #[error(transparent)]
    Secrets(#[from] SecretsError),
}

/// The outcome of provider setup: what to inject into the environment and,
/// for the free/local path, a model to ensure before launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSetup {
    pub chosen: Provider,
    /// Env pairs to inject so the agent talks to the chosen provider.
    pub env: Vec<(String, String)>,
    /// A model to ensure via Ollama before launch (free/local path only).
    pub ollama_model: Option<String>,
}

/// Paid providers a package uses whose key isn't available yet.
fn unconfigured_paid<F: Fn(&str) -> bool>(insights: &PackageInsights, is_set: F) -> Vec<Provider> {
    insights
        .providers
        .iter()
        .copied()
        .filter(|p| p.key_var().is_some_and(|kv| !is_set(kv)))
        .collect()
}

/// Whether the free local (Ollama) path is honestly viable for this package.
/// Ollama exposes an OpenAI-compatible API, so redirection works for OpenAI
/// (or a natively Ollama-using) agent, but not for an Anthropic-only one.
fn ollama_offer(insights: &PackageInsights) -> bool {
    insights.providers.contains(&Provider::OpenAI) || insights.providers.contains(&Provider::Ollama)
}

/// Env pairs that redirect an OpenAI-compatible client at a local Ollama model.
fn ollama_redirect_env(model: &str) -> Vec<(String, String)> {
    vec![
        (
            "OPENAI_BASE_URL".to_string(),
            OLLAMA_OPENAI_BASE_URL.to_string(),
        ),
        ("OPENAI_API_KEY".to_string(), "ollama".to_string()),
        ("OPENAI_MODEL".to_string(), model.to_string()),
    ]
}

fn read_line() -> Result<String, ProviderError> {
    let mut s = String::new();
    std::io::stdin()
        .read_line(&mut s)
        .map_err(|e| ProviderError::Io(e.to_string()))?;
    Ok(s.trim().to_string())
}

/// Offer a smart provider menu before launch when a package needs an LLM and
/// nothing is configured (design `2026-07-25-smart-run-and-local-ui`).
///
/// Returns `None` — deferring to the existing env-var flow — when the package
/// needs no model, a key is already available, or stdin isn't a terminal.
/// Otherwise prints a menu to stderr (stdout is reserved for MCP stdio) and
/// returns the user's choice: the free local path (Ollama, no key) or a pasted
/// key (stored for reuse).
pub fn setup_provider_interactive(
    insights: &PackageInsights,
    secrets_path: &Path,
) -> Result<Option<ProviderSetup>, ProviderError> {
    if !insights.needs_model() {
        return Ok(None);
    }
    let store = SecretStore::load(secrets_path)?;
    let is_set = |var: &str| std::env::var(var).is_ok() || store.get(var).is_some();
    let paid = unconfigured_paid(insights, is_set);
    if paid.is_empty() {
        // Free-only (Ollama/no LLM) or already configured — nothing to ask.
        return Ok(None);
    }
    if !std::io::stdin().is_terminal() {
        // Non-interactive: let the existing required-var flow decide.
        return Ok(None);
    }

    let offer_ollama = ollama_offer(insights);
    let mut options: Vec<Provider> = Vec::new();
    eprintln!();
    eprintln!("This agent needs a model. How do you want to run it?");
    let mut idx = 1;
    if offer_ollama {
        let status = if ollama_running() {
            " (running)"
        } else {
            " (will start)"
        };
        eprintln!("  [{idx}] Ollama — local, free{status}");
        options.push(Provider::Ollama);
        idx += 1;
    }
    for p in &paid {
        eprintln!("  [{idx}] {} — paste API key", p.display_name());
        options.push(*p);
        idx += 1;
    }
    eprint!("Choose [1]: ");
    let _ = std::io::stderr().flush();

    let choice = read_line()?;
    let sel = if choice.is_empty() {
        1
    } else {
        choice.parse::<usize>().unwrap_or(1)
    };
    let chosen = options
        .get(sel.saturating_sub(1))
        .copied()
        .unwrap_or(options[0]);

    if chosen.is_local_free() {
        let model = DEFAULT_OLLAMA_MODEL.to_string();
        eprintln!("→ Using local Ollama ({model}). No key needed.");
        return Ok(Some(ProviderSetup {
            chosen,
            env: ollama_redirect_env(&model),
            ollama_model: Some(model),
        }));
    }

    let key_var = chosen.key_var().expect("paid provider has a key var");
    eprint!("Paste your {} key ({key_var}): ", chosen.display_name());
    let _ = std::io::stderr().flush();
    let value = read_line()?;
    if value.is_empty() {
        return Ok(None);
    }
    let mut store = SecretStore::load(secrets_path)?;
    store.set(key_var, &value);
    store.save(secrets_path)?;
    eprintln!("→ Saved. You won't be asked again (switch later with `xelian key use`).");
    Ok(Some(ProviderSetup {
        chosen,
        env: vec![(key_var.to_string(), value)],
        ollama_model: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_is_the_free_local_provider() {
        assert!(Provider::Ollama.is_local_free());
        assert!(Provider::Ollama.key_var().is_none());
        assert!(!Provider::OpenAI.is_local_free());
        assert_eq!(Provider::OpenAI.key_var(), Some("OPENAI_API_KEY"));
        assert_eq!(Provider::Anthropic.key_var(), Some("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn detects_provider_from_env_var_names() {
        assert_eq!(
            Provider::from_env_var("OPENAI_API_KEY"),
            Some(Provider::OpenAI)
        );
        assert_eq!(
            Provider::from_env_var("anthropic_api_key"),
            Some(Provider::Anthropic)
        );
        assert_eq!(
            Provider::from_env_var("OLLAMA_HOST"),
            Some(Provider::Ollama)
        );
        assert_eq!(Provider::from_env_var("DATABASE_URL"), None);
    }

    #[test]
    fn detects_provider_from_import_names() {
        assert_eq!(Provider::from_import("openai"), Some(Provider::OpenAI));
        assert_eq!(
            Provider::from_import("langchain_openai.chat_models"),
            Some(Provider::OpenAI)
        );
        assert_eq!(
            Provider::from_import("anthropic"),
            Some(Provider::Anthropic)
        );
        assert_eq!(Provider::from_import("ollama"), Some(Provider::Ollama));
        assert_eq!(Provider::from_import("requests"), None);
    }

    #[test]
    fn all_lists_free_provider_first() {
        assert_eq!(Provider::all()[0], Provider::Ollama);
    }

    use super::super::inspect::{PackageInsights, RunStyle};

    fn insights_with(providers: Vec<Provider>) -> PackageInsights {
        PackageInsights {
            providers,
            required_env: vec![],
            runs_free_local: false,
            run_style: RunStyle::Repl,
            subcommands: vec![],
            usage: None,
        }
    }

    #[test]
    fn unconfigured_paid_excludes_configured_and_free() {
        let insights = insights_with(vec![Provider::OpenAI, Provider::Ollama]);
        // Nothing set → OpenAI is unconfigured; Ollama (free) never appears.
        let paid = unconfigured_paid(&insights, |_| false);
        assert_eq!(paid, vec![Provider::OpenAI]);
        // OPENAI key present → no paid setup needed.
        let paid = unconfigured_paid(&insights, |v| v == "OPENAI_API_KEY");
        assert!(paid.is_empty());
    }

    #[test]
    fn ollama_offered_only_for_openai_compatible() {
        assert!(ollama_offer(&insights_with(vec![Provider::OpenAI])));
        assert!(ollama_offer(&insights_with(vec![Provider::Ollama])));
        // Anthropic-only: Ollama can't honestly serve it.
        assert!(!ollama_offer(&insights_with(vec![Provider::Anthropic])));
    }

    #[test]
    fn ollama_redirect_env_points_at_local_endpoint() {
        let env = ollama_redirect_env("llama3.2");
        assert!(env.contains(&(
            "OPENAI_BASE_URL".to_string(),
            OLLAMA_OPENAI_BASE_URL.to_string()
        )));
        assert!(env.contains(&("OPENAI_API_KEY".to_string(), "ollama".to_string())));
        assert!(env.contains(&("OPENAI_MODEL".to_string(), "llama3.2".to_string())));
    }
}
