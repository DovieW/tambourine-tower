//! LLM providers for text formatting.
//!
//! This module provides LLM-based text formatting for dictation transcripts.
//! It supports multiple providers (OpenAI, Anthropic, Ollama) and uses
//! configurable prompts to clean up and format transcribed speech.

mod anthropic;
mod groq;
mod ollama;
mod openai;
mod prompts;

pub use anthropic::AnthropicLlmProvider;
pub use groq::GroqLlmProvider;
pub use ollama::OllamaLlmProvider;
pub use openai::OpenAiLlmProvider;
pub use prompts::{
    combine_prompt_sections, PromptSections, ADVANCED_PROMPT_DEFAULT, DICTIONARY_PROMPT_DEFAULT,
    MAIN_PROMPT_DEFAULT,
};

use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

/// Default timeout for LLM API requests
pub const DEFAULT_LLM_TIMEOUT: Duration = Duration::from_secs(30);

/// Errors that can occur during LLM operations
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("API error: {0}")]
    Api(String),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Request timed out after {0:?}")]
    Timeout(Duration),

    #[error("No API key configured for provider: {0}")]
    NoApiKey(String),

    #[error("Provider not available: {0}")]
    ProviderNotAvailable(String),
}

/// Trait for LLM providers that can format text
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Complete a prompt and return the response
    async fn complete(&self, system_prompt: &str, user_message: &str) -> Result<String, LlmError>;

    /// Get the provider name
    fn name(&self) -> &'static str;

    /// Get the current model being used
    fn model(&self) -> &str;
}

/// Registry of available LLM providers
pub struct LlmRegistry {
    providers: Vec<Arc<dyn LlmProvider>>,
    current: String,
}

impl LlmRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            current: String::new(),
        }
    }

    /// Register a provider
    pub fn register(&mut self, provider: Arc<dyn LlmProvider>) {
        let name = provider.name().to_string();
        self.providers.push(provider);
        if self.current.is_empty() {
            self.current = name;
        }
    }

    /// Set the current provider by name
    pub fn set_current(&mut self, name: &str) -> bool {
        if self.providers.iter().any(|p| p.name() == name) {
            self.current = name.to_string();
            true
        } else {
            false
        }
    }

    /// Get the current provider
    pub fn current(&self) -> Option<Arc<dyn LlmProvider>> {
        self.providers
            .iter()
            .find(|p| p.name() == self.current)
            .cloned()
    }

    /// Get a provider by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn LlmProvider>> {
        self.providers.iter().find(|p| p.name() == name).cloned()
    }

    /// List all registered provider names
    pub fn list_providers(&self) -> Vec<String> {
        self.providers.iter().map(|p| p.name().to_string()).collect()
    }

    /// Get the current provider name
    pub fn current_name(&self) -> &str {
        &self.current
    }
}

impl Default for LlmRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for LLM formatting
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// Whether LLM formatting is enabled
    pub enabled: bool,
    /// The provider to use (openai, anthropic, ollama)
    pub provider: String,
    /// API key for cloud providers
    pub api_key: String,
    /// Model to use (e.g., "gpt-4o-mini", "claude-3-haiku", "llama3")
    pub model: Option<String>,
    /// Base URL for Ollama (default: http://localhost:11434)
    pub ollama_url: Option<String>,
    /// Prompt sections configuration
    pub prompts: PromptSections,
    /// Request timeout
    pub timeout: Duration,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "openai".to_string(),
            api_key: String::new(),
            model: None,
            ollama_url: None,
            prompts: PromptSections::default(),
            timeout: DEFAULT_LLM_TIMEOUT,
        }
    }
}

/// Format text using an LLM provider
pub async fn format_text(
    provider: &dyn LlmProvider,
    transcript: &str,
    prompts: &PromptSections,
) -> Result<String, LlmError> {
    if transcript.trim().is_empty() {
        return Ok(String::new());
    }

    let system_prompt = combine_prompt_sections(prompts);
    let result = provider.complete(&system_prompt, transcript).await?;

    Ok(result.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_registry() {
        let registry = LlmRegistry::new();
        assert!(registry.current().is_none());
        assert!(registry.list_providers().is_empty());
    }

    #[test]
    fn test_llm_config_default() {
        let config = LlmConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.provider, "openai");
        assert_eq!(config.timeout, DEFAULT_LLM_TIMEOUT);
    }
}
