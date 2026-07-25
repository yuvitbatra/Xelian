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

use std::time::Duration;

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
}
