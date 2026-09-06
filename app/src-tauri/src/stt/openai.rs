//! OpenAI STT provider implementation.
//!
//! Supports three modes:
//! - Legacy Whisper API (whisper-1) - uses /v1/audio/transcriptions
//! - Audio chat models (e.g., gpt-4o-audio-preview) - uses /v1/responses with audio input
//! - Realtime transcription (gpt-4o-realtime-transcribe, gpt-4o-mini-realtime-transcribe) -
//!   uses WebSocket wss://api.openai.com/v1/realtime for concurrent streaming.
//!   These are separate model entries that map to OpenAI's transcription models
//!   (gpt-4o-transcribe, gpt-4o-mini-transcribe) within the realtime session.
//!
//! Realtime transcription docs:
//! - Guide: https://platform.openai.com/docs/guides/realtime-transcription
//! - WebSocket: https://platform.openai.com/docs/guides/realtime-websocket
//! - Client events: https://platform.openai.com/docs/api-reference/realtime-client-events

mod diarization;
mod realtime;

use super::http;
use super::language;
use super::openai_compat;
use super::streaming::StreamingSttSession;
use super::{AudioFormat, SttError, SttProvider};
use crate::request_log::RequestLogStore;
use crate::settings::ProxySettings;
use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;

/// OpenAI STT provider for speech-to-text
pub struct OpenAiSttProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
    default_prompt: Option<String>,
    default_language: Option<String>,
    api_base_url: String,
    request_log_store: Option<RequestLogStore>,
    proxy_settings: ProxySettings,
}

impl OpenAiSttProvider {
    const WHISPER_PROMPT_MAX_CHARS: usize = 224;
    const DEFAULT_OPENAI_API_BASE_URL: &'static str = "https://api.openai.com";

    /// Create a new OpenAI STT provider
    ///
    /// # Arguments
    /// * `api_key` - OpenAI API key
    /// * `model` - Model to use:
    ///   - "gpt-4o-audio-preview" (default) - GPT-4o with audio input
    ///   - "gpt-4o-mini-audio-preview" - Smaller/faster GPT-4o audio
    ///   - "whisper-1" - Legacy Whisper API
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(
        api_key: String,
        model: Option<String>,
        language: Option<String>,
        default_prompt: Option<String>,
    ) -> Self {
        let client = crate::network::build_plain_http_client_with_timeout(Duration::from_secs(120));

        Self {
            client,
            api_key,
            model: model.unwrap_or_else(|| "gpt-4o-audio-preview".to_string()),
            default_prompt: default_prompt
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            default_language: Self::normalize_language(language),
            api_base_url: Self::DEFAULT_OPENAI_API_BASE_URL.to_string(),
            request_log_store: None,
            proxy_settings: ProxySettings::default(),
        }
    }

    /// Create a new provider with a custom HTTP client
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_client(
        client: reqwest::Client,
        api_key: String,
        model: Option<String>,
        language: Option<String>,
        default_prompt: Option<String>,
    ) -> Self {
        Self {
            client,
            api_key,
            model: model.unwrap_or_else(|| "gpt-4o-audio-preview".to_string()),
            default_prompt: default_prompt
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            default_language: Self::normalize_language(language),
            api_base_url: Self::DEFAULT_OPENAI_API_BASE_URL.to_string(),
            request_log_store: None,
            proxy_settings: ProxySettings::default(),
        }
    }

    /// Override the API base URL (defaults to https://api.openai.com).
    ///
    /// This is primarily intended for deterministic contract tests (e.g., Wiremock).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_api_base_url(mut self, base_url: String) -> Self {
        self.api_base_url = base_url;
        self
    }

    fn api_base_url_trimmed(&self) -> &str {
        http::trim_base_url(&self.api_base_url)
    }

    fn transcriptions_url(&self) -> String {
        http::join_base_url(self.api_base_url_trimmed(), "/v1/audio/transcriptions")
    }

    fn responses_url(&self) -> String {
        http::join_base_url(self.api_base_url_trimmed(), "/v1/responses")
    }

    fn normalize_language(language: Option<String>) -> Option<String> {
        language::normalize_language_code(language)
    }

    pub fn with_request_log_store(mut self, store: Option<RequestLogStore>) -> Self {
        self.request_log_store = store;
        self
    }

    pub fn with_proxy_settings(mut self, proxy_settings: ProxySettings) -> Self {
        self.proxy_settings = proxy_settings;
        self
    }

    /// Whether this model uses the OpenAI Realtime Transcription API.
    ///
    /// Only our dedicated realtime model entries (`gpt-4o-realtime-transcribe`,
    /// `gpt-4o-mini-realtime-transcribe`) use the realtime WebSocket path.
    /// The underlying OpenAI transcription model is specified via
    /// [`realtime_transcription_model`](Self::realtime_transcription_model).
    fn supports_realtime_streaming(&self) -> bool {
        realtime::supports_realtime_streaming(self)
    }

    /// Start a concurrent streaming STT session via the OpenAI Realtime API.
    ///
    /// Audio is resampled to 24 kHz (the only rate supported by the Realtime API),
    /// encoded as PCM s16le, and sent as `input_audio_buffer.append` events.
    /// Server-side VAD detects speech turns and emits per-turn transcripts.
    async fn start_streaming_session(
        &self,
        sample_rate: u32,
    ) -> Result<StreamingSttSession, SttError> {
        realtime::start_streaming_session(self, sample_rate).await
    }

    /// Check if this model should use /v1/audio/transcriptions.
    ///
    /// Per OpenAI docs, `whisper-1` and the `*-transcribe` models are used via the
    /// dedicated transcription endpoint.  The realtime model entries also map to
    /// transcription models but go through the WS path instead.
    fn uses_transcriptions_endpoint(&self) -> bool {
        // Realtime-only models never go through the batch HTTP path.
        if self.supports_realtime_streaming() {
            return false;
        }
        self.model == "whisper-1"
            || self.model.contains("transcribe")
            || self.model.contains("whisper")
    }

    fn clamp_prompt_for_model(&self, prompt: Option<&str>) -> Option<String> {
        let prompt = prompt.map(str::trim).filter(|s| !s.is_empty())?;

        // Prompt support is only enabled for the dedicated transcription endpoint models.
        // If the user selected an OpenAI audio-chat model (Responses API path), ignore the prompt.
        if !self.uses_transcriptions_endpoint() {
            return None;
        }

        // Diarize models do not support the `prompt` parameter.
        if self.model.contains("diarize") {
            return None;
        }

        // OpenAI docs say Whisper only considers 224 tokens. Tokenization differs by language.
        // For a simple, predictable UX (and to match our UI), we clamp to 224 characters.
        if self.model == "whisper-1" && prompt.len() > Self::WHISPER_PROMPT_MAX_CHARS {
            return Some(
                prompt
                    .chars()
                    .take(Self::WHISPER_PROMPT_MAX_CHARS)
                    .collect(),
            );
        }

        Some(prompt.to_string())
    }

    /// Transcribe using the dedicated OpenAI transcription endpoint.
    async fn transcribe_audio_transcriptions(
        &self,
        audio: &[u8],
        prompt: Option<&str>,
    ) -> Result<String, SttError> {
        let endpoint = self.transcriptions_url();
        if self.model == "gpt-4o-transcribe-diarize" {
            return diarization::transcribe(self, audio)
                .await
                .map(|result| result.text);
        }
        let clamped_prompt = self.clamp_prompt_for_model(prompt);
        let language = self.default_language.as_deref();
        openai_compat::transcribe_wav_multipart_openai_compat(
            &self.client,
            "openai",
            "OpenAI Whisper API error",
            &endpoint,
            audio,
            &self.model,
            clamped_prompt.as_deref(),
            language,
            self.request_log_store.as_ref(),
            |rb| rb.bearer_auth(&self.api_key),
            SttError::Network,
        )
        .await
    }

    fn extract_responses_output_text(value: &serde_json::Value) -> Result<String, SttError> {
        if let Some(s) = value.get("output_text").and_then(|v| v.as_str()) {
            return Ok(s.to_string());
        }

        let output = value
            .get("output")
            .and_then(|v| v.as_array())
            .ok_or_else(|| SttError::Api("Responses API returned no 'output' array".to_string()))?;

        for item in output {
            if item.get("type").and_then(|t| t.as_str()) != Some("message") {
                continue;
            }

            let content = match item.get("content").and_then(|c| c.as_array()) {
                Some(c) => c,
                None => continue,
            };

            for part in content {
                match part.get("type").and_then(|t| t.as_str()) {
                    Some("refusal") => {
                        let refusal = part.get("refusal").and_then(|r| r.as_str()).unwrap_or("");
                        return Err(SttError::Api(format!("OpenAI refusal: {}", refusal)));
                    }
                    Some("output_text") => {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            return Ok(text.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }

        Err(SttError::Api(
            "Responses API returned no output_text content".to_string(),
        ))
    }

    /// Transcribe using the Responses API with audio input.
    async fn transcribe_responses_audio(
        &self,
        audio: &[u8],
        prompt: Option<&str>,
    ) -> Result<String, SttError> {
        use base64::{engine::general_purpose::STANDARD, Engine};

        // Encode audio as base64
        let audio_base64 = STANDARD.encode(audio);

        let mut instruction =
            "Transcribe this audio. Output only the transcribed text, nothing else.".to_string();
        if let Some(prompt) = self.clamp_prompt_for_model(prompt) {
            instruction.push_str("\n\nContext/prompt: ");
            instruction.push_str(&prompt);
        }
        if let Some(language) = self.default_language.as_deref() {
            instruction.push_str("\n\nLanguage: ");
            instruction.push_str(language);
        }

        let request_body = json!({
            "model": self.model,
            "input": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "input_audio",
                            "input_audio": {
                                "data": audio_base64,
                                "format": "wav"
                            }
                        },
                        {
                            "type": "text",
                            "text": instruction
                        }
                    ]
                }
            ],
            "text": {
                "format": {"type": "text"}
            }
        });

        if let Some(store) = &self.request_log_store {
            let request_json = json!({
                "provider": "openai",
                "endpoint": self.responses_url(),
                "body": {
                    "model": self.model,
                    "input": [
                        {
                            "role": "user",
                            "content": [
                                {
                                    "type": "input_audio",
                                    "input_audio": {
                                        "data": "<base64 audio omitted>",
                                        "format": "wav",
                                        "bytes": audio.len(),
                                        "base64_len": audio_base64.len(),
                                    }
                                },
                                {
                                    "type": "text",
                                    "text": instruction
                                }
                            ]
                        }
                    ],
                    "text": {
                        "format": {"type": "text"}
                    }
                }
            });

            store.with_current(|log| {
                log.stt_request_json = Some(request_json);
            });
        }

        let responses_url = self.responses_url();
        let response = crate::http::with_cloudflare_access_headers_if_target(
            self.client
                .post(&responses_url)
                .bearer_auth(&self.api_key)
                .json(&request_body),
            &responses_url,
        )
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                SttError::Timeout
            } else {
                SttError::Network(e)
            }
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(SttError::Api(format!(
                "OpenAI-compatible Responses API error (model={}, status={}): {}",
                self.model, status, error_text
            )));
        }

        let result: serde_json::Value = response.json().await?;

        if let Some(store) = &self.request_log_store {
            let result_for_log = result.clone();
            store.with_current(|log| {
                log.stt_response_json = Some(result_for_log);
            });
        }

        Self::extract_responses_output_text(&result)
    }

    /// Transcribe with an optional prompt.
    ///
    /// This is primarily used by the Settings "Test transcription" UI.
    pub async fn transcribe_with_prompt(
        &self,
        audio: &[u8],
        _format: &AudioFormat,
        prompt: Option<&str>,
    ) -> Result<String, SttError> {
        if self.uses_transcriptions_endpoint() {
            self.transcribe_audio_transcriptions(audio, prompt).await
        } else {
            self.transcribe_responses_audio(audio, prompt).await
        }
    }
}

#[async_trait]
impl SttProvider for OpenAiSttProvider {
    async fn transcribe_detailed(
        &self,
        audio: &[u8],
        format: &AudioFormat,
    ) -> Result<super::SttTranscript, SttError> {
        if self.model == "gpt-4o-transcribe-diarize" {
            diarization::transcribe(self, audio).await
        } else {
            Ok(super::SttTranscript {
                text: self.transcribe(audio, format).await?,
                segments: Vec::new(),
            })
        }
    }
    async fn transcribe(&self, audio: &[u8], _format: &AudioFormat) -> Result<String, SttError> {
        if self.supports_realtime_streaming() {
            return Err(SttError::Config(format!(
                "Model '{}' is realtime-only and cannot be used for batch transcription",
                self.model
            )));
        }
        self.transcribe_with_prompt(audio, _format, self.default_prompt.as_deref())
            .await
    }

    fn name(&self) -> &'static str {
        "openai"
    }

    fn supports_streaming(&self) -> bool {
        self.supports_realtime_streaming()
    }

    fn requires_streaming(&self) -> bool {
        self.supports_realtime_streaming()
    }

    async fn start_streaming(&self, sample_rate: u32) -> Result<StreamingSttSession, SttError> {
        self.start_streaming_session(sample_rate).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = OpenAiSttProvider::new("test-key".to_string(), None, None, None);
        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.model, "gpt-4o-audio-preview");
    }

    #[test]
    fn test_provider_with_custom_model() {
        let provider = OpenAiSttProvider::new(
            "test-key".to_string(),
            Some("whisper-1".to_string()),
            None,
            None,
        );
        assert_eq!(provider.model, "whisper-1");
    }

    #[test]
    fn test_is_chat_audio_model() {
        let provider = OpenAiSttProvider::new("test-key".to_string(), None, None, None);
        assert!(!provider.uses_transcriptions_endpoint());

        let provider = OpenAiSttProvider::new(
            "test-key".to_string(),
            Some("gpt-4o-mini-audio-preview".to_string()),
            None,
            None,
        );
        assert!(!provider.uses_transcriptions_endpoint());

        let provider = OpenAiSttProvider::new(
            "test-key".to_string(),
            Some("gpt-audio".to_string()),
            None,
            None,
        );
        assert!(!provider.uses_transcriptions_endpoint());

        let provider = OpenAiSttProvider::new(
            "test-key".to_string(),
            Some("gpt-audio-mini".to_string()),
            None,
            None,
        );
        assert!(!provider.uses_transcriptions_endpoint());

        // Batch transcription models use the transcriptions endpoint.
        let provider = OpenAiSttProvider::new(
            "test-key".to_string(),
            Some("whisper-1".to_string()),
            None,
            None,
        );
        assert!(provider.uses_transcriptions_endpoint());

        // Whisper-family model ids (used by some OpenAI-compatible gateways/providers)
        // should also use the transcriptions endpoint.
        let provider = OpenAiSttProvider::new(
            "test-key".to_string(),
            Some("whisper-large-v3-turbo".to_string()),
            None,
            None,
        );
        assert!(provider.uses_transcriptions_endpoint());

        let provider = OpenAiSttProvider::new(
            "test-key".to_string(),
            Some("gpt-4o-transcribe".to_string()),
            None,
            None,
        );
        assert!(provider.uses_transcriptions_endpoint());

        // Realtime models do NOT use the transcriptions endpoint
        // (they go through the WebSocket path instead).
        let provider = OpenAiSttProvider::new(
            "test-key".to_string(),
            Some("gpt-4o-realtime-transcribe".to_string()),
            None,
            None,
        );
        assert!(!provider.uses_transcriptions_endpoint());

        let provider = OpenAiSttProvider::new(
            "test-key".to_string(),
            Some("gpt-4o-mini-realtime-transcribe".to_string()),
            None,
            None,
        );
        assert!(!provider.uses_transcriptions_endpoint());
    }

    #[test]
    fn test_supports_realtime_streaming() {
        // Models that support realtime streaming (dedicated realtime entries).
        for model in &[
            "gpt-4o-realtime-transcribe",
            "gpt-4o-mini-realtime-transcribe",
        ] {
            let provider =
                OpenAiSttProvider::new("test-key".to_string(), Some(model.to_string()), None, None);
            assert!(
                provider.supports_realtime_streaming(),
                "Expected {} to support realtime streaming",
                model
            );
            assert!(provider.supports_streaming());
            assert!(provider.requires_streaming());
        }

        // Models that do NOT support realtime streaming (batch-only or non-transcription).
        for model in &[
            "gpt-4o-transcribe",
            "gpt-4o-mini-transcribe",
            "whisper-1",
            "gpt-4o-audio-preview",
            "gpt-4o-mini-audio-preview",
            "gpt-audio",
        ] {
            let provider =
                OpenAiSttProvider::new("test-key".to_string(), Some(model.to_string()), None, None);
            assert!(
                !provider.supports_realtime_streaming(),
                "Expected {} to NOT support realtime streaming",
                model
            );
            assert!(!provider.supports_streaming());
            assert!(!provider.requires_streaming());
        }
    }

    #[test]
    fn test_realtime_transcription_model() {
        let provider = OpenAiSttProvider::new(
            "test-key".to_string(),
            Some("gpt-4o-realtime-transcribe".to_string()),
            None,
            None,
        );
        assert_eq!(
            realtime::realtime_transcription_model(&provider),
            "gpt-4o-transcribe"
        );

        let provider = OpenAiSttProvider::new(
            "test-key".to_string(),
            Some("gpt-4o-mini-realtime-transcribe".to_string()),
            None,
            None,
        );
        assert_eq!(
            realtime::realtime_transcription_model(&provider),
            "gpt-4o-mini-transcribe"
        );
    }

    #[test]
    fn test_realtime_ws_url() {
        // The URL uses intent=transcription, regardless of which
        // transcription model the provider was configured with.
        let provider = OpenAiSttProvider::new(
            "test-key".to_string(),
            Some("gpt-4o-realtime-transcribe".to_string()),
            None,
            None,
        );
        let url = realtime::realtime_ws_url(&provider).unwrap();
        assert_eq!(url, "wss://api.openai.com/v1/realtime?intent=transcription");

        // Custom base URL
        let provider = provider.with_api_base_url("http://localhost:8080".to_string());
        let url = realtime::realtime_ws_url(&provider).unwrap();
        assert_eq!(url, "ws://localhost:8080/v1/realtime?intent=transcription");
    }
}
