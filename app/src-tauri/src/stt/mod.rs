//! Speech-to-Text (STT) provider abstraction and implementations.
//!
//! This module provides a trait-based abstraction for STT providers,
//! allowing easy switching between different speech recognition services.

mod aquavoice;
mod assemblyai;
mod deepgram;
mod elevenlabs;
mod fireworks;
mod groq;
mod http;
pub(crate) mod language;
mod openai;
mod openai_compat;
mod retry;
pub(crate) mod simulated_streaming;
mod speechmatics;
pub(crate) mod streaming;
pub(crate) mod websocket_transport;
mod whisper_server;

#[cfg(feature = "local-whisper")]
mod whisper;

pub use aquavoice::AquavoiceSttProvider;
pub use assemblyai::AssemblyAiSttProvider;
pub use deepgram::DeepgramSttProvider;
pub use elevenlabs::ElevenLabsSttProvider;
pub use fireworks::FireworksSttProvider;
pub use groq::GroqSttProvider;
pub use openai::OpenAiSttProvider;
#[allow(unused_imports)]
pub use retry::is_retryable_error;
pub use retry::{with_retry_report, RetryConfig, RetryTelemetry};
pub use speechmatics::SpeechmaticsSttProvider;
pub use streaming::StreamingSttSession;
pub use whisper_server::WhisperServerSttProvider;

#[cfg(feature = "local-whisper")]
pub use whisper::{LocalWhisperConfig, LocalWhisperProvider, WhisperModel};

#[cfg(feature = "local-whisper")]
pub use whisper::{
    get_local_whisper_backend_status, LocalWhisperBackendStatus, LocalWhisperComputeBackend,
};

use async_trait::async_trait;
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
pub struct SpeakerSegment {
    pub speaker: String,
    pub text: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
    /// Speaker identity is local to this upload, not the whole meeting.
    pub part: u32,
}

#[derive(Debug, Clone, Default)]
pub struct SttTranscript {
    pub text: String,
    pub segments: Vec<SpeakerSegment>,
}

pub fn speaker_document(text: &str, segments: &[SpeakerSegment]) -> String {
    if segments.is_empty() {
        return text.to_owned();
    }
    let multiple_parts = segments.iter().any(|s| s.part != segments[0].part);
    let mut document = String::new();
    let mut part = None;
    for segment in segments {
        if part != Some(segment.part) {
            if !document.is_empty() {
                document.push_str("\n\n");
            }
            if multiple_parts {
                document.push_str(&format!(
                    "— Part {} · speaker labels restart —\n\n",
                    segment.part
                ));
            }
            part = Some(segment.part);
        } else if !document.is_empty() {
            document.push_str("\n\n");
        }
        document.push_str(&format!(
            "Speaker {}\n{}",
            segment.speaker,
            segment.text.trim()
        ));
    }
    document
}

/// Audio format information for STT processing
#[derive(Debug, Clone)]
pub struct AudioFormat {
    #[cfg_attr(not(test), allow(dead_code))]
    pub sample_rate: u32,
    #[cfg_attr(not(test), allow(dead_code))]
    pub channels: u8,
    #[cfg_attr(not(test), allow(dead_code))]
    pub encoding: AudioEncoding,
}

impl Default for AudioFormat {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            channels: 1,
            encoding: AudioEncoding::Wav,
        }
    }
}

/// Supported audio encoding formats
#[derive(Debug, Clone, Copy)]
pub enum AudioEncoding {
    Wav,
    #[cfg_attr(not(test), allow(dead_code))]
    Pcm16,
}

/// Errors that can occur during STT operations
#[derive(Debug, thiserror::Error)]
pub enum SttError {
    #[error("Network error: {0}")]
    NetworkMessage(String),

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("API error: {0}")]
    Api(String),

    #[error("Audio processing error: {0}")]
    Audio(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Timeout: transcription took too long")]
    Timeout,
}

/// Trait for Speech-to-Text providers
#[async_trait]
pub trait SttProvider: Send + Sync {
    /// Transcribe audio data to text
    ///
    /// # Arguments
    /// * `audio` - Raw audio bytes (typically WAV format)
    /// * `format` - Information about the audio format
    ///
    /// # Returns
    /// The transcribed text, or an error if transcription fails
    async fn transcribe(&self, audio: &[u8], format: &AudioFormat) -> Result<String, SttError>;

    /// Providers without speaker metadata retain their existing implementation.
    async fn transcribe_detailed(
        &self,
        audio: &[u8],
        format: &AudioFormat,
    ) -> Result<SttTranscript, SttError> {
        Ok(SttTranscript {
            text: self.transcribe(audio, format).await?,
            segments: Vec::new(),
        })
    }

    /// Get the name of this provider
    #[cfg_attr(not(test), allow(dead_code))]
    fn name(&self) -> &'static str;

    /// Whether this provider supports concurrent streaming (audio sent during recording).
    ///
    /// When true, the pipeline can start a `StreamingSttSession` at recording start
    /// and feed audio chunks in real-time, resulting in near-instant transcription
    /// when recording stops.
    fn supports_streaming(&self) -> bool {
        false
    }

    /// Whether this provider *requires* streaming (i.e., has no batch fallback).
    ///
    /// When true and a streaming session fails, the pipeline should propagate the
    /// error instead of falling back to batch transcription.  This is used by
    /// realtime-only model entries (e.g., OpenAI's `gpt-4o-realtime-transcribe`).
    fn requires_streaming(&self) -> bool {
        false
    }

    /// Start a concurrent streaming session.
    ///
    /// Only valid when `supports_streaming()` returns true. The returned session
    /// accepts audio chunks via `audio_tx` and produces partial transcripts via
    /// `partial_rx`. Call `session.finalize()` to get the final transcript.
    ///
    /// `sample_rate` is the capture device's sample rate (e.g., 16000 or 44100).
    async fn start_streaming(&self, _sample_rate: u32) -> Result<StreamingSttSession, SttError> {
        Err(SttError::Config(
            "Streaming not supported by this provider".into(),
        ))
    }
}

/// Registry for managing multiple STT providers
pub struct SttRegistry {
    providers: std::collections::HashMap<String, Arc<dyn SttProvider>>,
    current: String,
}

impl SttRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            providers: std::collections::HashMap::new(),
            current: String::new(),
        }
    }

    /// Register a provider with the given name
    pub fn register(&mut self, name: &str, provider: Arc<dyn SttProvider>) {
        self.providers.insert(name.to_string(), provider);
        // If this is the first provider, set it as current
        if self.current.is_empty() {
            self.current = name.to_string();
        }
    }

    /// Set the current active provider
    pub fn set_current(&mut self, name: &str) -> Result<(), String> {
        if self.providers.contains_key(name) {
            self.current = name.to_string();
            Ok(())
        } else {
            Err(format!("Provider '{}' not found", name))
        }
    }

    /// Set the current provider name without requiring it to be registered.
    ///
    /// This is intended for UI/telemetry only. The pipeline uses its own provider
    /// cache and will lazily create providers as needed.
    pub fn set_current_name_for_ui(&mut self, name: &str) {
        self.current = name.to_string();
    }

    /// Get the current active provider
    #[allow(dead_code)]
    pub fn get_current(&self) -> Option<Arc<dyn SttProvider>> {
        self.providers.get(&self.current).cloned()
    }

    /// Get a provider by name
    #[allow(dead_code)]
    pub fn get(&self, name: &str) -> Option<Arc<dyn SttProvider>> {
        self.providers.get(name).cloned()
    }

    /// List all registered provider names
    #[allow(dead_code)]
    pub fn list_providers(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    /// Get the name of the current provider
    #[allow(dead_code)]
    pub fn current_name(&self) -> &str {
        &self.current
    }
}

impl Default for SttRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockProvider;

    #[async_trait]
    impl SttProvider for MockProvider {
        async fn transcribe(
            &self,
            _audio: &[u8],
            _format: &AudioFormat,
        ) -> Result<String, SttError> {
            Ok("test transcript".to_string())
        }

        fn name(&self) -> &'static str {
            "mock"
        }
    }

    #[test]
    fn test_registry_register_and_get() {
        let mut registry = SttRegistry::new();
        registry.register("mock", Arc::new(MockProvider));

        assert!(registry.get("mock").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_set_current() {
        let mut registry = SttRegistry::new();
        registry.register("mock", Arc::new(MockProvider));

        assert!(registry.set_current("mock").is_ok());
        assert!(registry.set_current("nonexistent").is_err());
    }
}
