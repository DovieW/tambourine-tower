//! Recording pipeline module that orchestrates audio capture → STT → LLM formatting → typing.
//!
//! This module provides the core pipeline for voice dictation, managing the
//! flow from audio recording through transcription to text output.
//!
//! ## Pipeline Hardening (Phase 5)
//! - Cancellation tokens for aborting in-flight tasks
//! - Timeouts on STT requests
//! - Bounded buffer sizes
//! - Proper error recovery (failures don't wedge the pipeline)
//! - Explicit state machine with guards
//!
//! ## LLM Formatting (Phase 6)
//! - Optional LLM-based text formatting after STT
//! - Multiple provider support (OpenAI, Anthropic, Ollama)
//! - Configurable prompts for dictation cleanup

use crate::audio_capture::{AudioCapture, AudioCaptureError, AudioCaptureEvent, VadAutoStopConfig};
use crate::llm::{
    format_text, AnthropicLlmProvider, GroqLlmProvider, LlmConfig, LlmError, LlmProvider,
    OllamaLlmProvider, OpenAiLlmProvider,
};
use crate::stt::{AudioFormat, RetryConfig, SttError, SttRegistry, with_retry};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Default timeout for STT transcription requests
const DEFAULT_TRANSCRIPTION_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum WAV file size in bytes (50MB) to prevent memory issues
const MAX_WAV_SIZE_BYTES: usize = 50 * 1024 * 1024;

/// Errors that can occur in the recording pipeline
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("Audio capture error: {0}")]
    AudioCapture(#[from] AudioCaptureError),

    #[error("STT error: {0}")]
    Stt(#[from] SttError),

    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),

    #[error("No STT provider configured")]
    NoProvider,

    #[error("Pipeline is already recording")]
    AlreadyRecording,

    #[error("Pipeline is not recording")]
    NotRecording,

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Lock error: {0}")]
    Lock(String),

    #[error("Operation cancelled")]
    Cancelled,

    #[error("Transcription timeout after {0:?}")]
    Timeout(Duration),

    #[error("Recording too large: {0} bytes exceeds limit of {1} bytes")]
    RecordingTooLarge(usize, usize),
}

/// Pipeline state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineState {
    /// Pipeline is idle, ready to start recording
    Idle,
    /// Pipeline is actively recording audio
    Recording,
    /// Pipeline is transcribing recorded audio
    Transcribing,
    /// Pipeline encountered an error (recoverable - can start new recording)
    Error,
}

impl PipelineState {
    /// Check if this state allows starting a new recording
    pub fn can_start_recording(&self) -> bool {
        matches!(self, PipelineState::Idle | PipelineState::Error)
    }

    /// Check if this state allows stopping a recording
    pub fn can_stop_recording(&self) -> bool {
        matches!(self, PipelineState::Recording)
    }

    /// Check if this state allows cancellation
    pub fn can_cancel(&self) -> bool {
        matches!(self, PipelineState::Recording | PipelineState::Transcribing)
    }
}

/// Events emitted by the pipeline
#[derive(Debug, Clone)]
pub enum PipelineEvent {
    /// Recording has started
    RecordingStarted,
    /// Recording has stopped
    RecordingStopped,
    /// Transcription is in progress
    TranscriptionStarted,
    /// Final transcript received
    TranscriptReady(String),
    /// An error occurred
    Error(String),
}

/// Outcome of the optional LLM formatting step.
#[derive(Debug, Clone)]
pub enum LlmOutcome {
    /// LLM step was not attempted (not configured or disabled).
    NotAttempted,
    /// LLM step completed successfully and returned formatted text.
    Succeeded,
    /// LLM step timed out and the pipeline fell back to the raw STT transcript.
    TimedOut,
    /// LLM step failed and the pipeline fell back to the raw STT transcript.
    Failed(String),
}

/// Detailed result for a transcription request.
///
/// This separates the raw STT transcript from the final output (which may
/// include LLM formatting and/or fallbacks).
#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    /// Raw transcript as returned from the STT provider (before any LLM formatting).
    pub stt_text: String,
    /// Final output text returned by the pipeline.
    /// If LLM formatting was disabled, this will match `stt_text`.
    /// If LLM formatting failed/timed out, this falls back to `stt_text`.
    pub final_text: String,
    /// Duration of the STT phase (including retries), in milliseconds.
    pub stt_duration_ms: u64,
    /// Duration of the LLM phase (including timeout/fallback), in milliseconds.
    pub llm_duration_ms: Option<u64>,
    /// Outcome of the LLM phase.
    pub llm_outcome: LlmOutcome,
}

impl TranscriptionResult {
    pub fn llm_attempted(&self) -> bool {
        !matches!(self.llm_outcome, LlmOutcome::NotAttempted)
    }
}

/// Configuration for the recording pipeline
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Maximum recording duration in seconds
    pub max_duration_secs: f32,
    /// STT provider to use
    pub stt_provider: String,
    /// API key for the STT provider
    pub stt_api_key: String,
    /// Optional model override for STT
    pub stt_model: Option<String>,
    /// Retry configuration for STT requests
    pub retry_config: RetryConfig,
    /// VAD auto-stop configuration
    pub vad_config: VadAutoStopConfig,
    /// Timeout for transcription requests
    pub transcription_timeout: Duration,
    /// Maximum recording size in bytes (0 = no limit beyond default)
    pub max_recording_bytes: usize,
    /// LLM formatting configuration
    pub llm_config: LlmConfig,
    /// Path to local Whisper model (for local-whisper feature)
    #[cfg(feature = "local-whisper")]
    pub whisper_model_path: Option<std::path::PathBuf>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            max_duration_secs: 300.0, // 5 minutes max
            stt_provider: "groq".to_string(),
            stt_api_key: String::new(),
            stt_model: None,
            retry_config: RetryConfig::default(),
            vad_config: VadAutoStopConfig::default(),
            transcription_timeout: DEFAULT_TRANSCRIPTION_TIMEOUT,
            max_recording_bytes: MAX_WAV_SIZE_BYTES,
            llm_config: LlmConfig::default(),
            #[cfg(feature = "local-whisper")]
            whisper_model_path: None,
        }
    }
}

/// Internal state for the recording pipeline
struct PipelineInner {
    audio_capture: AudioCapture,
    stt_registry: SttRegistry,
    llm_provider: Option<Arc<dyn LlmProvider>>,
    state: PipelineState,
    config: PipelineConfig,
    /// Cancellation token for the current operation
    cancel_token: Option<CancellationToken>,
}

impl PipelineInner {
    fn new(config: PipelineConfig) -> Self {
        let audio_capture = AudioCapture::with_vad_config(config.vad_config.clone());
        let mut inner = Self {
            audio_capture,
            stt_registry: SttRegistry::new(),
            llm_provider: None,
            state: PipelineState::Idle,
            config: config.clone(),
            cancel_token: None,
        };
        inner.initialize_providers(&config);
        inner
    }

    fn initialize_providers(&mut self, config: &PipelineConfig) {
        // Initialize STT providers
        match config.stt_provider.as_str() {
            "openai" if !config.stt_api_key.is_empty() => {
                let provider = crate::stt::OpenAiSttProvider::new(
                    config.stt_api_key.clone(),
                    config.stt_model.clone(),
                );
                self.stt_registry.register("openai", Arc::new(provider));
            }
            "groq" if !config.stt_api_key.is_empty() => {
                let provider = crate::stt::GroqSttProvider::new(
                    config.stt_api_key.clone(),
                    config.stt_model.clone(),
                );
                self.stt_registry.register("groq", Arc::new(provider));
            }
            "deepgram" if !config.stt_api_key.is_empty() => {
                let provider = crate::stt::DeepgramSttProvider::new(
                    config.stt_api_key.clone(),
                    config.stt_model.clone(),
                );
                self.stt_registry.register("deepgram", Arc::new(provider));
            }
            #[cfg(feature = "local-whisper")]
            "local-whisper" => {
                // Local whisper doesn't need an API key
                if let Some(model_path) = &config.whisper_model_path {
                    match crate::stt::LocalWhisperProvider::new(model_path.clone()) {
                        Ok(provider) => {
                            self.stt_registry
                                .register("local-whisper", Arc::new(provider));
                            log::info!("Local Whisper provider initialized");
                        }
                        Err(e) => {
                            log::error!("Failed to initialize local Whisper: {}", e);
                        }
                    }
                } else {
                    log::warn!("Local Whisper selected but no model path configured");
                }
            }
            _ => {
                if config.stt_api_key.is_empty() {
                    log::warn!(
                        "STT provider '{}' requires an API key",
                        config.stt_provider
                    );
                } else {
                    log::warn!("Unknown STT provider: {}", config.stt_provider);
                }
            }
        }
        let _ = self.stt_registry.set_current(&config.stt_provider);

        // Initialize LLM provider if enabled
        self.llm_provider = None;
        if config.llm_config.enabled && !config.llm_config.api_key.is_empty() {
            self.llm_provider = Some(create_llm_provider(&config.llm_config));
            log::info!(
                "LLM formatting enabled with provider: {}",
                config.llm_config.provider
            );
        } else if config.llm_config.enabled && config.llm_config.provider == "ollama" {
            // Ollama doesn't need an API key
            self.llm_provider = Some(create_llm_provider(&config.llm_config));
            log::info!("LLM formatting enabled with local Ollama");
        }
    }

    /// Reset to idle state, clearing any error condition
    fn reset_to_idle(&mut self) {
        self.state = PipelineState::Idle;
        self.cancel_token = None;
    }

    /// Transition to error state
    fn set_error(&mut self, msg: &str) {
        log::error!("Pipeline error: {}", msg);
        self.state = PipelineState::Error;
        self.cancel_token = None;
    }
}

/// Create an LLM provider based on configuration
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

/// Thread-safe wrapper for the recording pipeline
///
/// Uses standard Mutex to be Send + Sync for Tauri state management.
/// Provides robust error handling and cancellation support.
pub struct SharedPipeline {
    inner: Arc<Mutex<PipelineInner>>,
}

impl SharedPipeline {
    /// Create a new shared pipeline
    pub fn new(config: PipelineConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(PipelineInner::new(config))),
        }
    }

    /// Start recording
    ///
    /// Creates a new cancellation token for this recording session.
    pub fn start_recording(&self) -> Result<(), PipelineError> {
        let mut inner = self.inner.lock().map_err(|e| PipelineError::Lock(e.to_string()))?;

        // State guard: only allow starting from Idle or Error states
        if !inner.state.can_start_recording() {
            return Err(PipelineError::AlreadyRecording);
        }

        // Create a new cancellation token for this session
        let cancel_token = CancellationToken::new();
        inner.cancel_token = Some(cancel_token);

        let max_duration = inner.config.max_duration_secs;
        match inner.audio_capture.start(max_duration) {
            Ok(()) => {
                inner.state = PipelineState::Recording;
                log::info!("Pipeline: Recording started");
                Ok(())
            }
            Err(e) => {
                inner.set_error(&format!("Failed to start recording: {}", e));
                Err(PipelineError::AudioCapture(e))
            }
        }
    }

    /// Stop recording and return the raw WAV audio
    pub fn stop_recording(&self) -> Result<Vec<u8>, PipelineError> {
        let mut inner = self.inner.lock().map_err(|e| PipelineError::Lock(e.to_string()))?;

        if !inner.state.can_stop_recording() {
            return Err(PipelineError::NotRecording);
        }

        match inner.audio_capture.stop_and_get_wav() {
            Ok(wav_bytes) => {
                // Check size limit
                let max_bytes = inner.config.max_recording_bytes;
                if max_bytes > 0 && wav_bytes.len() > max_bytes {
                    inner.set_error(&format!(
                        "Recording too large: {} bytes",
                        wav_bytes.len()
                    ));
                    return Err(PipelineError::RecordingTooLarge(wav_bytes.len(), max_bytes));
                }

                inner.reset_to_idle();
                log::info!(
                    "Pipeline: Recording stopped, {} bytes captured",
                    wav_bytes.len()
                );
                Ok(wav_bytes)
            }
            Err(e) => {
                inner.set_error(&format!("Failed to stop recording: {}", e));
                Err(PipelineError::AudioCapture(e))
            }
        }
    }

    /// Stop recording and transcribe the audio, returning a detailed result.
    ///
    /// This is the main end-to-end function for voice dictation.
    /// Includes:
    /// - Automatic retry with exponential backoff on transient failures
    /// - Timeout protection
    /// - Cancellation support
    /// - Proper error recovery
    /// - Optional LLM formatting
    pub async fn stop_and_transcribe_detailed(
        &self,
    ) -> Result<TranscriptionResult, PipelineError> {
        // Phase 1: Stop recording and prepare for transcription (synchronous, holds lock briefly)
        let (wav_bytes, stt_provider, llm_provider, llm_prompts, retry_config, timeout, cancel_token) = {
            let mut inner = self.inner.lock().map_err(|e| PipelineError::Lock(e.to_string()))?;

            if !inner.state.can_stop_recording() {
                return Err(PipelineError::NotRecording);
            }

            let wav_bytes = match inner.audio_capture.stop_and_get_wav() {
                Ok(bytes) => bytes,
                Err(e) => {
                    inner.set_error(&format!("Failed to stop recording: {}", e));
                    return Err(PipelineError::AudioCapture(e));
                }
            };

            // Check size limit
            let max_bytes = inner.config.max_recording_bytes;
            if max_bytes > 0 && wav_bytes.len() > max_bytes {
                inner.set_error(&format!("Recording too large: {} bytes", wav_bytes.len()));
                return Err(PipelineError::RecordingTooLarge(wav_bytes.len(), max_bytes));
            }

            inner.state = PipelineState::Transcribing;

            let stt_provider = inner
                .stt_registry
                .get_current()
                .ok_or_else(|| {
                    inner.set_error("No STT provider configured");
                    PipelineError::NoProvider
                })?;

            let llm_provider = inner.llm_provider.clone();
            let llm_prompts = inner.config.llm_config.prompts.clone();
            let retry_config = inner.config.retry_config.clone();
            let timeout = inner.config.transcription_timeout;
            let cancel_token = inner.cancel_token.clone().unwrap_or_else(CancellationToken::new);

            (wav_bytes, stt_provider, llm_provider, llm_prompts, retry_config, timeout, cancel_token)
        };

        log::info!(
            "Pipeline: Starting transcription ({} bytes, timeout {:?})",
            wav_bytes.len(),
            timeout
        );

        // Phase 2: Transcribe with retry logic (async, outside the lock)
        let format = AudioFormat::default();
        let wav_bytes_for_retry = wav_bytes.clone();

        // Wrap the transcription in a timeout and cancellation
        let transcription_future = async {
            with_retry(&retry_config, || {
                let provider = stt_provider.clone();
                let wav_bytes = wav_bytes_for_retry.clone();
                let format = format.clone();
                async move { provider.transcribe(&wav_bytes, &format).await }
            })
            .await
        };

        // Race between transcription, timeout, and cancellation
        let stt_start = std::time::Instant::now();
        let stt_result = tokio::select! {
            biased;

            // Cancellation takes priority
            _ = cancel_token.cancelled() => {
                log::info!("Pipeline: Transcription cancelled");
                Err(PipelineError::Cancelled)
            }

            // Timeout
            _ = tokio::time::sleep(timeout) => {
                log::warn!("Pipeline: Transcription timed out after {:?}", timeout);
                Err(PipelineError::Timeout(timeout))
            }

            // Actual transcription
            result = transcription_future => {
                result.map_err(PipelineError::from)
            }
        };

        let stt_text = match stt_result {
            Ok(t) => t,
            Err(e) => {
                let mut inner = self
                    .inner
                    .lock()
                    .map_err(|err| PipelineError::Lock(err.to_string()))?;
                if matches!(e, PipelineError::Cancelled) {
                    inner.reset_to_idle();
                } else {
                    inner.set_error(&e.to_string());
                }
                return Err(e);
            }
        };
        let stt_duration_ms = stt_start.elapsed().as_millis() as u64;
        log::info!("Pipeline: STT complete, {} chars", stt_text.len());

        // Phase 3: Optional LLM formatting
        let mut llm_duration_ms: Option<u64> = None;
        let mut llm_outcome: LlmOutcome = LlmOutcome::NotAttempted;

        let final_text = if let Some(llm) = llm_provider {
            log::info!("Pipeline: Applying LLM formatting");

            llm_outcome = LlmOutcome::Succeeded; // may be overwritten by fallback paths
            let llm_start = std::time::Instant::now();

            // Apply LLM formatting with timeout
            let llm_timeout = Duration::from_secs(30);
            let llm_result = tokio::select! {
                biased;

                _ = cancel_token.cancelled() => {
                    log::info!("Pipeline: LLM formatting cancelled");
                    Err(PipelineError::Cancelled)
                }

                _ = tokio::time::sleep(llm_timeout) => {
                    log::warn!("Pipeline: LLM formatting timed out, using raw transcript");
                    // On timeout, fall back to raw transcript instead of failing
                    llm_outcome = LlmOutcome::TimedOut;
                    Ok(stt_text.clone())
                }

                result = format_text(llm.as_ref(), &stt_text, &llm_prompts) => {
                    match result {
                        Ok(formatted) => {
                            log::info!("Pipeline: LLM formatted {} -> {} chars", stt_text.len(), formatted.len());
                            Ok(formatted)
                        }
                        Err(e) => {
                            log::warn!("Pipeline: LLM formatting failed ({}), using raw transcript", e);
                            // On error, fall back to raw transcript instead of failing
                            llm_outcome = LlmOutcome::Failed(e.to_string());
                            Ok(stt_text.clone())
                        }
                    }
                }
            };

            llm_duration_ms = Some(llm_start.elapsed().as_millis() as u64);

            match llm_result {
                Ok(text) => text,
                Err(PipelineError::Cancelled) => {
                    let mut inner = self.inner.lock().map_err(|e| PipelineError::Lock(e.to_string()))?;
                    inner.reset_to_idle();
                    return Err(PipelineError::Cancelled);
                }
                Err(_) => stt_text.clone(), // Fallback on other errors
            }
        } else {
            stt_text.clone()
        };

        // Phase 4: Update state to idle
        {
            let mut inner = self.inner.lock().map_err(|e| PipelineError::Lock(e.to_string()))?;
            inner.reset_to_idle();
            log::info!("Pipeline: Complete, {} chars output", final_text.len());
        }

        Ok(TranscriptionResult {
            stt_text,
            final_text,
            stt_duration_ms,
            llm_duration_ms,
            llm_outcome,
        })
    }

    /// Stop recording and transcribe the audio.
    ///
    /// Kept for backwards compatibility. Prefer `stop_and_transcribe_detailed`.
    pub async fn stop_and_transcribe(&self) -> Result<String, PipelineError> {
        self.stop_and_transcribe_detailed()
            .await
            .map(|r| r.final_text)
    }

    /// Update configuration
    ///
    /// Note: This will not affect an in-progress recording.
    pub fn update_config(&self, config: PipelineConfig) -> Result<(), PipelineError> {
        let mut inner = self.inner.lock().map_err(|e| PipelineError::Lock(e.to_string()))?;

        // Don't update config while recording - could cause issues
        if inner.state == PipelineState::Recording {
            log::warn!("Pipeline: Config update requested while recording, will take effect after current session");
        }

        inner.config = config.clone();
        inner.stt_registry = SttRegistry::new();
        inner.initialize_providers(&config);
        // Update VAD config on audio capture
        inner.audio_capture.set_vad_config(config.vad_config);
        log::info!("Pipeline configuration updated");
        Ok(())
    }

    /// Check if recording
    pub fn is_recording(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.state == PipelineState::Recording)
            .unwrap_or(false)
    }

    /// Poll for VAD events (non-blocking)
    ///
    /// Returns the next VAD event if one is available, or None if no events are pending.
    pub fn poll_vad_event(&self) -> Option<AudioCaptureEvent> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.audio_capture.poll_vad_event())
    }

    /// Check if VAD auto-stop is enabled
    pub fn is_vad_auto_stop_enabled(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.audio_capture.is_vad_auto_stop_enabled())
            .unwrap_or(false)
    }

    /// Cancel current operation
    ///
    /// This will:
    /// - Stop any ongoing recording
    /// - Signal cancellation to any in-flight transcription
    /// - Reset the pipeline to Idle state
    pub fn cancel(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if !inner.state.can_cancel() {
                log::debug!("Pipeline: Cancel requested but nothing to cancel (state: {:?})", inner.state);
                return;
            }

            // Signal cancellation to any async tasks
            if let Some(token) = inner.cancel_token.take() {
                token.cancel();
            }

            // Stop audio capture if recording
            if inner.state == PipelineState::Recording {
                inner.audio_capture.stop();
            }

            inner.reset_to_idle();
            log::info!("Pipeline: Cancelled and reset to idle");
        }
    }

    /// Force reset the pipeline to idle state
    ///
    /// Use this to recover from stuck states. Cancels any in-progress operations.
    pub fn force_reset(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            // Cancel any async tasks
            if let Some(token) = inner.cancel_token.take() {
                token.cancel();
            }

            // Force stop audio capture
            inner.audio_capture.stop();

            // Reset state
            inner.reset_to_idle();
            log::warn!("Pipeline: Force reset to idle");
        }
    }

    /// Get current state
    pub fn state(&self) -> PipelineState {
        self.inner
            .lock()
            .map(|inner| inner.state)
            .unwrap_or(PipelineState::Error)
    }

    /// Get the name of the current STT provider
    pub fn current_provider_name(&self) -> String {
        self.inner
            .lock()
            .map(|inner| inner.stt_registry.current_name().to_string())
            .unwrap_or_default()
    }

    /// Get a clone of the current pipeline configuration
    pub fn config(&self) -> PipelineConfig {
        self.inner
            .lock()
            .map(|inner| inner.config.clone())
            .unwrap_or_default()
    }

    /// Check if the pipeline is in an error state
    pub fn is_error(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.state == PipelineState::Error)
            .unwrap_or(true)
    }

    /// Get the cancellation token for external use (e.g., for coordinating with other async tasks)
    pub fn get_cancel_token(&self) -> Option<CancellationToken> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.cancel_token.clone())
    }
}

impl Default for SharedPipeline {
    fn default() -> Self {
        Self::new(PipelineConfig::default())
    }
}

impl Clone for SharedPipeline {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

// Ensure SharedPipeline is Send + Sync for Tauri state
unsafe impl Send for SharedPipeline {}
unsafe impl Sync for SharedPipeline {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_config_default() {
        let config = PipelineConfig::default();
        assert_eq!(config.max_duration_secs, 300.0);
        assert_eq!(config.stt_provider, "groq");
        assert_eq!(config.transcription_timeout, DEFAULT_TRANSCRIPTION_TIMEOUT);
        assert_eq!(config.max_recording_bytes, MAX_WAV_SIZE_BYTES);
    }

    #[test]
    fn test_shared_pipeline_creation() {
        let config = PipelineConfig {
            stt_api_key: "test-key".to_string(),
            ..Default::default()
        };
        let pipeline = SharedPipeline::new(config);
        assert_eq!(pipeline.state(), PipelineState::Idle);
        assert!(!pipeline.is_error());
    }

    #[test]
    fn test_state_guards() {
        assert!(PipelineState::Idle.can_start_recording());
        assert!(PipelineState::Error.can_start_recording());
        assert!(!PipelineState::Recording.can_start_recording());
        assert!(!PipelineState::Transcribing.can_start_recording());

        assert!(PipelineState::Recording.can_stop_recording());
        assert!(!PipelineState::Idle.can_stop_recording());

        assert!(PipelineState::Recording.can_cancel());
        assert!(PipelineState::Transcribing.can_cancel());
        assert!(!PipelineState::Idle.can_cancel());
    }

    #[test]
    fn test_force_reset() {
        let config = PipelineConfig {
            stt_api_key: "test-key".to_string(),
            ..Default::default()
        };
        let pipeline = SharedPipeline::new(config);

        // Force reset should always work
        pipeline.force_reset();
        assert_eq!(pipeline.state(), PipelineState::Idle);
    }
}
