//! Tauri commands for configuration endpoints.
//!
//! This module provides commands that replace the Python server's config API,
//! including default prompt sections and available providers.

use serde::Serialize;
use tauri::AppHandle;

#[cfg(desktop)]
use tauri_plugin_store::StoreExt;

// ============================================================================
// Default Prompt Sections
// ============================================================================

/// Main prompt section - Core rules, punctuation, new lines
pub const MAIN_PROMPT_DEFAULT: &str = r#"You are a dictation formatting assistant. Your task is to format transcribed speech.

## Core Rules
- Remove filler words (um, uh, err, erm, etc.)
- Use punctuation where appropriate
- Capitalize sentences properly
- Keep the original meaning and tone intact
- Do NOT add any new information or change the intent
- Do NOT condense, summarize, or make sentences more concise - preserve the speaker's full expression
- Do NOT answer questions - if the user dictates a question, output the cleaned question, not an answer
- Do NOT respond conversationally or engage with the content - you are a text processor, not a conversational assistant
- Output ONLY the cleaned text, nothing else - no explanations, no quotes, no prefixes

### Good Example
Input: "um so basically I was like thinking we should uh you know update the readme file"
Output: "So basically, I was thinking we should update the readme file."

### Bad Examples

1. Condensing/summarizing (preserve full expression):
   Input: "I really think that we should probably consider maybe going to the store to pick up some groceries"
   Bad: "We should go grocery shopping."
   Good: "I really think that we should probably consider going to the store to pick up some groceries."

2. Answering questions (just clean the question):
   Input: "what is the capital of France"
   Bad: "The capital of France is Paris."
   Good: "What is the capital of France?"

3. Responding conversationally (format, don't engage):
   Input: "hey how are you doing today"
   Bad: "I'm doing well, thank you for asking!"
   Good: "Hey, how are you doing today?"

4. Adding information (keep original intent only):
   Input: "send the email to john"
   Bad: "Send the email to John as soon as possible."
   Good: "Send the email to John."

## Punctuation
Convert spoken punctuation to symbols:
- "comma" = ,
- "period" or "full stop" = .
- "question mark" = ?
- "exclamation point" or "exclamation mark" = !
- "dash" = -
- "em dash" = —
- "quotation mark" or "quote" or "end quote" = "
- "colon" = :
- "semicolon" = ;
- "open parenthesis" or "open paren" = (
- "close parenthesis" or "close paren" = )

Example:
Input: "I can't wait exclamation point Let's meet at seven period"
Output: "I can't wait! Let's meet at seven."

## New Line and Paragraph
- "new line" = Insert a line break
- "new paragraph" = Insert a paragraph break (blank line)

Example:
Input: "Hello, new line, world, new paragraph, bye"
Output: "Hello
world

bye""#;

/// Advanced prompt section - Backtrack corrections and list formatting
pub const ADVANCED_PROMPT_DEFAULT: &str = r#"## Backtrack Corrections
When the speaker corrects themselves mid-sentence, use only the corrected version:
- "actually" signals a correction: "at 2 actually 3" = "at 3"
- "scratch that" removes the previous phrase: "cookies scratch that brownies" = "brownies"
- "wait" or "I mean" signal corrections: "on Monday wait Tuesday" = "on Tuesday"
- Natural restatements: "as a gift... as a present" = "as a present"

Examples:
- "Let's do coffee at 2 actually 3" = "Let's do coffee at 3."
- "I'll bring cookies scratch that brownies" = "I'll bring brownies."
- "Send it to John I mean Jane" = "Send it to Jane."

## List Formats
When sequence words are detected, format as a numbered or bulleted list:
- Triggers: "one", "two", "three" or "first", "second", "third"
- Capitalize each list item

Example:
- "My goals are one finish the report two send the presentation three review feedback" =
  "My goals are:
  1. Finish the report
  2. Send the presentation
  3. Review feedback""#;

/// Dictionary prompt section - Personal word mappings
pub const DICTIONARY_PROMPT_DEFAULT: &str = r#"## Personal Dictionary
Apply these corrections for technical terms, proper nouns, and custom words.

Entries can be in various formats - interpret flexibly:
- Explicit mappings: "ant row pic = Anthropic"
- Single terms to recognize: Just "LLM" (correct phonetic mismatches)
- Natural descriptions: "The name 'Claude' should always be capitalized"

When you hear terms that sound like entries below, use the correct spelling/form.

### Entries:
Tambourine
LLM
ant row pick = Anthropic
Claude
Pipecat
Tauri"#;

/// Response containing default prompt sections
#[derive(Debug, Serialize)]
pub struct DefaultSectionsResponse {
    pub main: String,
    pub advanced: String,
    pub dictionary: String,
}

/// Get default prompts for each section
#[tauri::command]
pub fn get_default_sections() -> DefaultSectionsResponse {
    DefaultSectionsResponse {
        main: MAIN_PROMPT_DEFAULT.to_string(),
        advanced: ADVANCED_PROMPT_DEFAULT.to_string(),
        dictionary: DICTIONARY_PROMPT_DEFAULT.to_string(),
    }
}

// ============================================================================
// Available Providers
// ============================================================================

/// Information about a provider
#[derive(Debug, Serialize)]
pub struct ProviderInfo {
    pub value: String,
    pub label: String,
    pub is_local: bool,
}

/// Response listing available providers
#[derive(Debug, Serialize)]
pub struct AvailableProvidersResponse {
    pub stt: Vec<ProviderInfo>,
    pub llm: Vec<ProviderInfo>,
}

/// STT provider definitions
const STT_PROVIDERS: &[(&str, &str, bool)] = &[
    ("openai", "OpenAI Whisper", false),
    ("groq", "Groq", false),
    ("deepgram", "Deepgram", false),
    ("whisper", "Local Whisper", true),
];

/// LLM provider definitions
const LLM_PROVIDERS: &[(&str, &str, bool)] = &[
    ("openai", "OpenAI", false),
    ("anthropic", "Anthropic", false),
    ("groq", "Groq", false),
    ("ollama", "Ollama (Local)", true),
];

/// Helper to check if an API key is configured in the store
#[cfg(desktop)]
fn has_api_key(app: &AppHandle, key: &str) -> bool {
    app.store("settings.json")
        .ok()
        .and_then(|store| store.get(key))
        .and_then(|v| v.as_str().map(|s| !s.is_empty()))
        .unwrap_or(false)
}

/// Get list of available STT and LLM providers (those with API keys configured)
#[cfg(desktop)]
#[tauri::command]
pub fn get_available_providers(app: AppHandle) -> AvailableProvidersResponse {
    let mut stt_providers = Vec::new();
    let mut llm_providers = Vec::new();

    // Check which STT providers have API keys
    for (id, label, is_local) in STT_PROVIDERS {
        let key_name = format!("{}_api_key", id);
        // Local providers don't need API keys, remote ones do
        if *is_local || has_api_key(&app, &key_name) {
            stt_providers.push(ProviderInfo {
                value: id.to_string(),
                label: label.to_string(),
                is_local: *is_local,
            });
        }
    }

    // Check which LLM providers have API keys
    for (id, label, is_local) in LLM_PROVIDERS {
        let key_name = format!("{}_api_key", id);
        // Local providers don't need API keys, remote ones do
        if *is_local || has_api_key(&app, &key_name) {
            llm_providers.push(ProviderInfo {
                value: id.to_string(),
                label: label.to_string(),
                is_local: *is_local,
            });
        }
    }

    AvailableProvidersResponse {
        stt: stt_providers,
        llm: llm_providers,
    }
}

/// Stub for non-desktop platforms
#[cfg(not(desktop))]
#[tauri::command]
pub fn get_available_providers(_app: AppHandle) -> AvailableProvidersResponse {
    AvailableProvidersResponse {
        stt: vec![],
        llm: vec![],
    }
}

// ============================================================================
// Pipeline Configuration Updates
// ============================================================================

/// Update the pipeline configuration when settings change
/// This re-initializes the STT provider based on current settings
#[cfg(desktop)]
#[tauri::command]
pub fn sync_pipeline_config(app: AppHandle) -> Result<(), String> {
    use crate::pipeline::{PipelineConfig, SharedPipeline};
    use crate::stt::RetryConfig;
    use tauri::Manager;

    // Read STT settings from store
    let stt_provider: String = app
        .store("settings.json")
        .ok()
        .and_then(|store| store.get("stt_provider"))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_else(|| "groq".to_string());

    // Read STT model from store
    let stt_model: Option<String> = app
        .store("settings.json")
        .ok()
        .and_then(|store| store.get("stt_model"))
        .and_then(|v| serde_json::from_value(v).ok());

    // Get the appropriate API key based on provider
    let stt_api_key: String = {
        let key_name = format!("{}_api_key", stt_provider);
        app.store("settings.json")
            .ok()
            .and_then(|store| store.get(&key_name))
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    };

    // Read all available STT API keys (for per-profile provider overrides at runtime)
    let mut stt_api_keys: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for provider in ["openai", "groq", "deepgram"] {
        let key_name = format!("{}_api_key", provider);
        let key: String = app
            .store("settings.json")
            .ok()
            .and_then(|store| store.get(&key_name))
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        if !key.is_empty() {
            stt_api_keys.insert(provider.to_string(), key);
        }
    }

    // Read STT timeout from store (seconds)
    let stt_timeout_seconds: f64 = app
        .store("settings.json")
        .ok()
        .and_then(|store| store.get("stt_timeout_seconds"))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or(60.0);

    // Read LLM settings from store
    // NOTE: If the user has not selected an LLM provider yet, keep LLM disabled.
    let rewrite_llm_enabled: bool = app
        .store("settings.json")
        .ok()
        .and_then(|store| store.get("rewrite_llm_enabled"))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or(false);

    let llm_provider: Option<String> = app
        .store("settings.json")
        .ok()
        .and_then(|store| store.get("llm_provider"))
        .and_then(|v| serde_json::from_value(v).ok());

    let llm_model: Option<String> = app
        .store("settings.json")
        .ok()
        .and_then(|store| store.get("llm_model"))
        .and_then(|v| serde_json::from_value(v).ok());

    let llm_api_key: String = llm_provider
        .as_deref()
        .map(|provider| {
            let key_name = format!("{}_api_key", provider);
            app.store("settings.json")
                .ok()
                .and_then(|store| store.get(&key_name))
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default()
        })
        .unwrap_or_default();

    // IMPORTANT: `enabled` is only the global toggle. The effective provider/key is resolved
    // per transcription based on the active profile.
    let llm_enabled = rewrite_llm_enabled;

    // Read all available LLM API keys (for per-profile provider overrides at runtime)
    let mut llm_api_keys: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for provider in ["openai", "anthropic", "groq"] {
        let key_name = format!("{}_api_key", provider);
        let key: String = app
            .store("settings.json")
            .ok()
            .and_then(|store| store.get(&key_name))
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        if !key.is_empty() {
            llm_api_keys.insert(provider.to_string(), key);
        }
    }

    // Read rewrite prompt sections + per-program profiles from store
    let cleanup_prompt_sections: Option<crate::settings::CleanupPromptSectionsSetting> = app
        .store("settings.json")
        .ok()
        .and_then(|store| store.get("cleanup_prompt_sections"))
        .and_then(|v| serde_json::from_value(v).ok());

    let base_prompts: crate::llm::PromptSections = cleanup_prompt_sections
        .as_ref()
        .map(|o| o.apply_to(&crate::llm::PromptSections::default()))
        .unwrap_or_else(crate::llm::PromptSections::default);

    let rewrite_program_prompt_profiles: Vec<crate::settings::RewriteProgramPromptProfile> = app
        .store("settings.json")
        .ok()
        .and_then(|store| store.get("rewrite_program_prompt_profiles"))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    let program_prompt_profiles: Vec<crate::llm::ProgramPromptProfile> =
        rewrite_program_prompt_profiles
            .into_iter()
            .map(|p| crate::llm::ProgramPromptProfile {
                id: p.id,
                name: p.name,
                program_paths: p.program_paths,
                prompts: p
                    .cleanup_prompt_sections
                    .as_ref()
                    .map(|o| o.apply_to(&base_prompts))
                    .unwrap_or_else(|| base_prompts.clone()),
                rewrite_llm_enabled: p.rewrite_llm_enabled,
                stt_provider: p.stt_provider,
                stt_model: p.stt_model,
                stt_timeout_seconds: p.stt_timeout_seconds,
                llm_provider: p.llm_provider,
                llm_model: p.llm_model,
            })
            .collect();

    // Read VAD settings from store
    let vad_settings: VadSettings = app
        .store("settings.json")
        .ok()
        .and_then(|store| store.get("vad_settings"))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    let config = PipelineConfig {
        stt_provider: stt_provider.clone(),
        stt_api_key,
        stt_api_keys,
        stt_model: stt_model.clone(),
        max_duration_secs: 300.0,
        retry_config: RetryConfig::default(),
        vad_config: vad_settings.to_vad_auto_stop_config(),
        transcription_timeout: std::time::Duration::from_secs_f64(stt_timeout_seconds),
        max_recording_bytes: 50 * 1024 * 1024, // 50MB
        llm_config: crate::llm::LlmConfig {
            enabled: llm_enabled,
            provider: llm_provider.clone().unwrap_or_else(|| "openai".to_string()),
            api_key: llm_api_key,
            model: llm_model.clone(),
            prompts: base_prompts,
            program_prompt_profiles,
            ..Default::default()
        },
        llm_api_keys,
    };

    // Update the pipeline
    if let Some(pipeline) = app.try_state::<SharedPipeline>() {
        pipeline
            .update_config(config)
            .map_err(|e| format!("Failed to update pipeline config: {}", e))?;
        log::info!(
            "Pipeline config synced - STT: {} ({}), LLM: {} ({}), VAD: {}",
            stt_provider,
            stt_model.as_deref().unwrap_or("default"),
            llm_provider.clone().unwrap_or_else(|| "disabled".to_string()),
            llm_model.as_deref().unwrap_or("default"),
            vad_settings.enabled
        );
    }

    Ok(())
}

/// Stub for non-desktop platforms
#[cfg(not(desktop))]
#[tauri::command]
pub fn sync_pipeline_config(_app: AppHandle) -> Result<(), String> {
    Ok(())
}

// ============================================================================
// VAD Settings
// ============================================================================

use crate::settings::VadSettings;

/// Get current VAD settings from the store
#[cfg(desktop)]
#[tauri::command]
pub fn get_vad_settings(app: AppHandle) -> VadSettings {
    app.store("settings.json")
        .ok()
        .and_then(|store| store.get("vad_settings"))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// Stub for non-desktop platforms
#[cfg(not(desktop))]
#[tauri::command]
pub fn get_vad_settings(_app: AppHandle) -> VadSettings {
    VadSettings::default()
}

/// Save VAD settings to the store
#[cfg(desktop)]
#[tauri::command]
pub fn set_vad_settings(app: AppHandle, settings: VadSettings) -> Result<(), String> {
    let store = app
        .store("settings.json")
        .map_err(|e| format!("Failed to get store: {}", e))?;

    store
        .set(
            "vad_settings",
            serde_json::to_value(&settings).map_err(|e| format!("Failed to serialize: {}", e))?,
        );

    store
        .save()
        .map_err(|e| format!("Failed to save store: {}", e))?;

    log::info!("VAD settings updated: enabled={}, auto_stop={}", settings.enabled, settings.auto_stop);
    Ok(())
}

/// Stub for non-desktop platforms
#[cfg(not(desktop))]
#[tauri::command]
pub fn set_vad_settings(_app: AppHandle, _settings: VadSettings) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_default_sections() {
        let response = get_default_sections();
        assert!(!response.main.is_empty());
        assert!(!response.advanced.is_empty());
        assert!(!response.dictionary.is_empty());
        assert!(response.main.contains("dictation formatting"));
    }
}
