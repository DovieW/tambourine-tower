//! Tauri commands for LLM formatting configuration.

use crate::llm::{
    LlmConfig, PromptSections, ADVANCED_PROMPT_DEFAULT, DICTIONARY_PROMPT_DEFAULT,
    MAIN_PROMPT_DEFAULT,
};
use crate::llm::{
    format_text, AnthropicLlmProvider, GroqLlmProvider, LlmProvider, OllamaLlmProvider,
    OpenAiLlmProvider,
};
use crate::pipeline::SharedPipeline;
use std::sync::Arc;
use std::time::Duration;
use tauri::State;

/// Error type for LLM commands
#[derive(Debug, serde::Serialize)]
pub struct LlmCommandError {
    pub message: String,
}

impl From<String> for LlmCommandError {
    fn from(message: String) -> Self {
        Self { message }
    }
}

/// LLM configuration payload from frontend
#[derive(Debug, serde::Deserialize)]
pub struct LlmConfigPayload {
    /// Whether LLM formatting is enabled
    pub enabled: bool,
    /// Provider name: "openai", "anthropic", or "ollama"
    pub provider: String,
    /// API key (not needed for ollama)
    pub api_key: Option<String>,
    /// Model to use (optional, uses provider default if not specified)
    pub model: Option<String>,
    /// Base URL for Ollama (optional)
    pub ollama_url: Option<String>,
    /// Timeout in seconds (optional, default 30)
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, serde::Serialize)]
pub struct TestLlmRewriteResponse {
    pub output: String,
    pub provider_used: String,
    pub model_used: String,
}

#[derive(Debug, serde::Serialize)]
pub struct LlmCompleteResponse {
    pub output: String,
    pub provider_used: String,
    pub model_used: String,
}

fn create_llm_provider(config: &LlmConfig) -> Arc<dyn LlmProvider> {
    match config.provider.as_str() {
        "anthropic" => {
            let provider = if let Some(model) = &config.model {
                AnthropicLlmProvider::with_model(config.api_key.clone(), model.clone())
            } else {
                AnthropicLlmProvider::new(config.api_key.clone())
            };
            Arc::new(provider.with_timeout(config.timeout))
        }
        "groq" => {
            let provider = if let Some(model) = &config.model {
                GroqLlmProvider::with_model(config.api_key.clone(), model.clone())
            } else {
                GroqLlmProvider::new(config.api_key.clone())
            };
            Arc::new(provider.with_timeout(config.timeout))
        }
        "ollama" => {
            let provider = OllamaLlmProvider::with_url(
                config
                    .ollama_url
                    .clone()
                    .unwrap_or_else(|| "http://localhost:11434".to_string()),
                config.model.clone(),
            );
            Arc::new(provider.with_timeout(config.timeout))
        }
        _ => {
            // Default to OpenAI
            let provider = if let Some(model) = &config.model {
                OpenAiLlmProvider::with_model(config.api_key.clone(), model.clone())
            } else {
                OpenAiLlmProvider::new(config.api_key.clone())
            };
            Arc::new(provider.with_timeout(config.timeout))
        }
    }
}

fn create_llm_provider_without_timeout(config: &LlmConfig) -> Arc<dyn LlmProvider> {
    match config.provider.as_str() {
        "anthropic" => {
            let provider = if let Some(model) = &config.model {
                AnthropicLlmProvider::with_model(config.api_key.clone(), model.clone())
            } else {
                AnthropicLlmProvider::new(config.api_key.clone())
            };
            Arc::new(provider.without_timeout())
        }
        "groq" => {
            let provider = if let Some(model) = &config.model {
                GroqLlmProvider::with_model(config.api_key.clone(), model.clone())
            } else {
                GroqLlmProvider::new(config.api_key.clone())
            };
            Arc::new(provider.without_timeout())
        }
        "ollama" => {
            let provider = OllamaLlmProvider::with_url(
                config
                    .ollama_url
                    .clone()
                    .unwrap_or_else(|| "http://localhost:11434".to_string()),
                config.model.clone(),
            );
            Arc::new(provider.without_timeout())
        }
        _ => {
            // Default to OpenAI
            let provider = if let Some(model) = &config.model {
                OpenAiLlmProvider::with_model(config.api_key.clone(), model.clone())
            } else {
                OpenAiLlmProvider::new(config.api_key.clone())
            };
            Arc::new(provider.without_timeout())
        }
    }
}

/// Prompt configuration payload from frontend
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct PromptConfigPayload {
    /// Custom main prompt (null to use default)
    pub main_custom: Option<String>,
    /// Whether advanced section is enabled
    pub advanced_enabled: bool,
    /// Custom advanced prompt (null to use default)
    pub advanced_custom: Option<String>,
    /// Whether dictionary section is enabled
    pub dictionary_enabled: bool,
    /// Custom dictionary prompt (null to use default)
    pub dictionary_custom: Option<String>,
}

impl From<PromptConfigPayload> for PromptSections {
    fn from(payload: PromptConfigPayload) -> Self {
        Self {
            main_custom: payload.main_custom,
            advanced_enabled: payload.advanced_enabled,
            advanced_custom: payload.advanced_custom,
            dictionary_enabled: payload.dictionary_enabled,
            dictionary_custom: payload.dictionary_custom,
        }
    }
}

impl From<PromptSections> for PromptConfigPayload {
    fn from(sections: PromptSections) -> Self {
        Self {
            main_custom: sections.main_custom,
            advanced_enabled: sections.advanced_enabled,
            advanced_custom: sections.advanced_custom,
            dictionary_enabled: sections.dictionary_enabled,
            dictionary_custom: sections.dictionary_custom,
        }
    }
}

/// Get the default prompt templates
#[tauri::command]
pub fn get_llm_default_prompts() -> DefaultPromptsResponse {
    DefaultPromptsResponse {
        main: MAIN_PROMPT_DEFAULT.to_string(),
        advanced: ADVANCED_PROMPT_DEFAULT.to_string(),
        dictionary: DICTIONARY_PROMPT_DEFAULT.to_string(),
    }
}

/// Response containing default prompts
#[derive(Debug, serde::Serialize)]
pub struct DefaultPromptsResponse {
    pub main: String,
    pub advanced: String,
    pub dictionary: String,
}

/// Get available LLM providers
#[tauri::command]
pub fn get_llm_providers() -> Vec<LlmProviderInfo> {
    vec![
        LlmProviderInfo {
            id: "openai".to_string(),
            name: "OpenAI".to_string(),
            requires_api_key: true,
            default_model: "gpt-4o-mini".to_string(),
            models: vec![
                "gpt-4.1".to_string(),
                "gpt-4.1-mini".to_string(),
                "gpt-4.1-nano".to_string(),
                "gpt-4o-mini".to_string(),
                "gpt-4o".to_string(),
                "gpt-4-turbo".to_string(),
                "gpt-3.5-turbo".to_string(),
            ],
        },
        LlmProviderInfo {
            id: "anthropic".to_string(),
            name: "Anthropic".to_string(),
            requires_api_key: true,
            default_model: "claude-3-haiku-20240307".to_string(),
            models: vec![
                "claude-sonnet-4-5".to_string(),
                "claude-haiku-4-5".to_string(),
                "claude-opus-4-5".to_string(),
                "claude-3-haiku-20240307".to_string(),
                "claude-3-sonnet-20240229".to_string(),
                "claude-3-opus-20240229".to_string(),
                "claude-3-5-sonnet-20241022".to_string(),
            ],
        },
        LlmProviderInfo {
            id: "ollama".to_string(),
            name: "Ollama".to_string(),
            requires_api_key: false,
            default_model: "llama3.2".to_string(),
            models: vec![
                "llama3.2".to_string(),
                "llama3.1".to_string(),
                "mistral".to_string(),
                "codellama".to_string(),
            ],
        },
    ]
}

/// Test LLM rewrite for the given transcript.
///
/// Uses the effective provider/model/prompts as configured in the pipeline config.
/// If `profile_id` matches a program prompt profile, its overrides are applied; otherwise
/// the Default profile is used.
#[tauri::command]
pub async fn test_llm_rewrite(
    pipeline: State<'_, SharedPipeline>,
    transcript: String,
    profile_id: Option<String>,
) -> Result<TestLlmRewriteResponse, LlmCommandError> {
    let config = pipeline.config();

    // IMPORTANT: This is a *test* endpoint. It intentionally ignores the
    // "Rewrite Transcription" enable toggle so users can validate prompts/
    // provider/model without changing runtime behavior.
    let (desired_provider, desired_model, prompts) = if let Some(id) = profile_id {
        if id != "default" {
            let profile = config
                .llm_config
                .program_prompt_profiles
                .iter()
                .find(|p| p.id == id)
                .ok_or_else(|| LlmCommandError::from(format!("Unknown profile_id: {}", id)))?;

            let provider = profile
                .llm_provider
                .clone()
                .unwrap_or_else(|| config.llm_config.provider.clone());
            let model = profile.llm_model.clone().or_else(|| config.llm_config.model.clone());

            (provider, model, profile.prompts.clone())
        } else {
            (
                config.llm_config.provider.clone(),
                config.llm_config.model.clone(),
                config.llm_config.prompts.clone(),
            )
        }
    } else {
        (
            config.llm_config.provider.clone(),
            config.llm_config.model.clone(),
            config.llm_config.prompts.clone(),
        )
    };

    let api_key = if desired_provider == "ollama" {
        String::new()
    } else {
        config
            .llm_api_keys
            .get(desired_provider.as_str())
            .cloned()
            .unwrap_or_default()
    };

    let provider_cfg = LlmConfig {
        enabled: true,
        provider: desired_provider,
        api_key,
        model: desired_model,
        ollama_url: config.llm_config.ollama_url.clone(),
        prompts: PromptSections::default(),
        program_prompt_profiles: Vec::new(),
        timeout: config.llm_config.timeout,
    };

    // This is a *test* endpoint: do not enforce request timeouts.
    let provider = create_llm_provider_without_timeout(&provider_cfg);
    let output = format_text(provider.as_ref(), &transcript, &prompts)
        .await
        .map_err(|e| LlmCommandError::from(e.to_string()))?;

    Ok(TestLlmRewriteResponse {
        output,
        provider_used: provider.name().to_string(),
        model_used: provider.model().to_string(),
    })
}

/// Run a one-off LLM completion with explicit provider/model and explicit prompts.
///
/// This is used by the History UI to send analysis instructions as the *system prompt*
/// and the transcript bundle as the *user prompt*.
#[tauri::command]
pub async fn llm_complete(
    pipeline: State<'_, SharedPipeline>,
    provider: String,
    model: Option<String>,
    system_prompt: String,
    user_prompt: String,
) -> Result<LlmCompleteResponse, LlmCommandError> {
    let config = pipeline.config();

    let desired_provider = provider;
    let desired_model = model;

    let api_key = if desired_provider == "ollama" {
        String::new()
    } else {
        config
            .llm_api_keys
            .get(desired_provider.as_str())
            .cloned()
            .unwrap_or_default()
    };

    if desired_provider != "ollama" && api_key.trim().is_empty() {
        return Err(LlmCommandError::from(format!(
            "No API key configured for provider: {}",
            desired_provider
        )));
    }

    let provider_cfg = LlmConfig {
        enabled: true,
        provider: desired_provider,
        api_key,
        model: desired_model,
        ollama_url: config.llm_config.ollama_url.clone(),
        prompts: PromptSections::default(),
        program_prompt_profiles: Vec::new(),
        timeout: config.llm_config.timeout,
    };

    let provider = create_llm_provider(&provider_cfg);
    let output = provider
        .complete(system_prompt.as_str(), user_prompt.as_str())
        .await
        .map_err(|e| LlmCommandError::from(e.to_string()))?;

    Ok(LlmCompleteResponse {
        output: output.trim().to_string(),
        provider_used: provider.name().to_string(),
        model_used: provider.model().to_string(),
    })
}

/// LLM provider information for the frontend
#[derive(Debug, serde::Serialize)]
pub struct LlmProviderInfo {
    pub id: String,
    pub name: String,
    pub requires_api_key: bool,
    pub default_model: String,
    pub models: Vec<String>,
}

/// Update LLM configuration on the pipeline
#[tauri::command]
pub fn update_llm_config(
    pipeline: State<'_, SharedPipeline>,
    config: LlmConfigPayload,
) -> Result<(), LlmCommandError> {
    // Get current pipeline config and update just the LLM portion
    // Note: This is a simplified approach - in a full implementation,
    // we'd want to preserve other config and only update LLM settings
    let llm_config = LlmConfig {
        enabled: config.enabled,
        provider: config.provider,
        api_key: config.api_key.unwrap_or_default(),
        model: config.model,
        ollama_url: config.ollama_url,
        prompts: PromptSections::default(),
        program_prompt_profiles: Vec::new(),
        timeout: Duration::from_secs(config.timeout_secs.unwrap_or(30)),
    };

    // Get current config from pipeline and update LLM portion
    let current_config = get_current_pipeline_config(&pipeline)?;
    let new_config = crate::pipeline::PipelineConfig {
        llm_config,
        ..current_config
    };

    pipeline
        .update_config(new_config)
        .map_err(|e| LlmCommandError::from(e.to_string()))?;

    log::info!("LLM configuration updated");
    Ok(())
}

/// Update LLM prompt configuration
#[tauri::command]
pub fn update_llm_prompts(
    pipeline: State<'_, SharedPipeline>,
    prompts: PromptConfigPayload,
) -> Result<(), LlmCommandError> {
    let current_config = get_current_pipeline_config(&pipeline)?;
    let mut llm_config = current_config.llm_config.clone();
    llm_config.prompts = prompts.into();

    let new_config = crate::pipeline::PipelineConfig {
        llm_config,
        ..current_config
    };

    pipeline
        .update_config(new_config)
        .map_err(|e| LlmCommandError::from(e.to_string()))?;

    log::info!("LLM prompts updated");
    Ok(())
}

/// Get current LLM configuration
#[tauri::command]
pub fn get_llm_config(pipeline: State<'_, SharedPipeline>) -> Result<LlmConfigResponse, LlmCommandError> {
    let config = get_current_pipeline_config(&pipeline)?;
    Ok(LlmConfigResponse {
        enabled: config.llm_config.enabled,
        provider: config.llm_config.provider,
        model: config.llm_config.model,
        ollama_url: config.llm_config.ollama_url,
        timeout_secs: config.llm_config.timeout.as_secs(),
        prompts: config.llm_config.prompts.into(),
    })
}

/// Response containing current LLM configuration
#[derive(Debug, serde::Serialize)]
pub struct LlmConfigResponse {
    pub enabled: bool,
    pub provider: String,
    pub model: Option<String>,
    pub ollama_url: Option<String>,
    pub timeout_secs: u64,
    pub prompts: PromptConfigPayload,
}

/// Helper to get current pipeline config (placeholder - needs proper implementation)
fn get_current_pipeline_config(
    _pipeline: &State<'_, SharedPipeline>,
) -> Result<crate::pipeline::PipelineConfig, LlmCommandError> {
    // Note: The current SharedPipeline doesn't expose config reading
    // For now, return default config. In a full implementation, we'd
    // add a get_config() method to SharedPipeline.
    Ok(crate::pipeline::PipelineConfig::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_llm_providers() {
        let providers = get_llm_providers();
        assert_eq!(providers.len(), 3);
        assert!(providers.iter().any(|p| p.id == "openai"));
        assert!(providers.iter().any(|p| p.id == "anthropic"));
        assert!(providers.iter().any(|p| p.id == "ollama"));
    }

    #[test]
    fn test_get_default_prompts() {
        let prompts = get_llm_default_prompts();
        assert!(!prompts.main.is_empty());
        assert!(!prompts.advanced.is_empty());
        assert!(!prompts.dictionary.is_empty());
    }
}
