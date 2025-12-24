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

use crate::audio_capture::{AudioCapture, AudioCaptureError, AudioCaptureEvent, AudioLevelSnapshot, AudioLevelStats, VadAutoStopConfig};
use crate::llm::{
    format_text, AnthropicLlmProvider, GroqLlmProvider, LlmConfig, LlmError, LlmProvider,
    OllamaLlmProvider, OpenAiLlmProvider,
};
use crate::stt::{AudioFormat, RetryConfig, SttError, SttProvider, SttRegistry, with_retry};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn normalize_program_path(path: &str) -> String {
    // Windows comparisons are case-insensitive, and we want to treat / and \ equivalently.
    path.replace('/', "\\").to_lowercase()
}

fn select_profile_for_foreground_app(llm_config: &LlmConfig) -> Option<crate::llm::ProgramPromptProfile> {
    let foreground = crate::windows_apps::get_foreground_process_path();
    let Some(foreground) = foreground else {
        return None;
    };

    let foreground_norm = normalize_program_path(&foreground);

    for profile in &llm_config.program_prompt_profiles {
        if profile
            .program_paths
            .iter()
            .any(|p| normalize_program_path(p) == foreground_norm)
        {
            log::debug!(
                "Pipeline: Using profile '{}' for foreground app {}",
                profile.name,
                foreground
            );
            return Some(profile.clone());
        }
    }

    None
}

fn canonicalize_stt_provider_id(id: &str) -> String {
    match id {
        // Historical UI value
        "whisper" => "local-whisper".to_string(),
        other => other.to_string(),
    }
}

/// Normalize STT output text.
///
/// Some providers (notably Whisper-based APIs) may include a leading space as a
/// tokenization artifact (many vocabularies encode " space+word" as a single token).
/// We trim only *leading* whitespace to avoid changing internal formatting.
fn normalize_stt_text(text: String) -> String {
    match text.chars().next() {
        Some(c) if c.is_whitespace() => text.trim_start().to_string(),
        _ => text,
    }
}

fn seconds_to_duration_or(seconds: f64, fallback: Duration) -> Duration {
    // Guard against invalid values.
    if !seconds.is_finite() || seconds <= 0.0 {
        return fallback;
    }
    Duration::from_secs_f64(seconds)
}

/// Default timeout for STT transcription requests
const DEFAULT_TRANSCRIPTION_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum WAV file size in bytes (50MB) to prevent memory issues
const MAX_WAV_SIZE_BYTES: usize = 50 * 1024 * 1024;

/// Default values for the quiet-audio gate.
///
/// Thresholds are in dBFS (decibels relative to full scale, where 0 dBFS is max amplitude).
const DEFAULT_QUIET_AUDIO_MIN_DURATION_SECS: f32 = 0.15;
const DEFAULT_QUIET_AUDIO_RMS_DBFS_THRESHOLD: f32 = -60.0;
const DEFAULT_QUIET_AUDIO_PEAK_DBFS_THRESHOLD: f32 = -50.0;

fn amp_to_dbfs(amp: f32) -> f32 {
    if !amp.is_finite() || amp <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * amp.log10()
    }
}

fn is_effectively_quiet(
    stats: AudioLevelStats,
    min_duration_secs: f32,
    rms_dbfs_threshold: f32,
    peak_dbfs_threshold: f32,
) -> bool {
    // Very short recordings are usually accidental taps; treat as quiet.
    if stats.duration_secs < min_duration_secs {
        return true;
    }

    let rms_dbfs = amp_to_dbfs(stats.rms);
    let peak_dbfs = amp_to_dbfs(stats.peak);

    rms_dbfs < rms_dbfs_threshold && peak_dbfs < peak_dbfs_threshold
}

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
    /// Pipeline is rewriting/formatting text via an LLM (optional step)
    Rewriting,
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
        matches!(
            self,
            PipelineState::Recording | PipelineState::Transcribing | PipelineState::Rewriting
        )
    }
}

/// Events emitted by the pipeline
#[cfg_attr(not(test), allow(dead_code))]
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
    /// LLM provider id actually used for this transcription (if the LLM step was attempted).
    ///
    /// This is sourced from the concrete provider instance (including any default/fallback
    /// model selection performed by the provider implementation).
    pub llm_provider_used: Option<String>,
    /// LLM model actually used for this transcription (if the LLM step was attempted).
    ///
    /// This is sourced from the concrete provider instance. If the configured model is None,
    /// this will still be populated with the provider's internal default model.
    pub llm_model_used: Option<String>,
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
    /// Optional backend input device name (CPAL device name).
    ///
    /// When set, recording will attempt to use the first input device whose name
    /// matches exactly, falling back to the system default if not found.
    pub input_device_name: Option<String>,
    /// Maximum recording duration in seconds
    pub max_duration_secs: f32,
    /// STT provider to use
    pub stt_provider: String,
    /// API key for the STT provider
    pub stt_api_key: String,
    /// API keys for all configured STT providers (provider id -> key)
    pub stt_api_keys: HashMap<String, String>,
    /// Optional model override for STT
    pub stt_model: Option<String>,

    /// Optional global transcription prompt.
    ///
    /// Applied by STT providers that support prompting (currently OpenAI transcription endpoint models).
    pub stt_transcription_prompt: Option<String>,
    /// Retry configuration for STT requests
    pub retry_config: RetryConfig,
    /// VAD auto-stop configuration
    pub vad_config: VadAutoStopConfig,
    /// Timeout for transcription requests
    pub transcription_timeout: Duration,
    /// Maximum recording size in bytes (0 = no limit beyond default)
    pub max_recording_bytes: usize,

    /// Enable a quiet-audio gate to avoid silent-audio hallucinations.
    pub quiet_audio_gate_enabled: bool,
    /// Treat recordings shorter than this as effectively quiet.
    pub quiet_audio_min_duration_secs: f32,
    /// RMS threshold (in dBFS) below which the audio is considered quiet.
    pub quiet_audio_rms_dbfs_threshold: f32,
    /// Peak threshold (in dBFS) below which the audio is considered quiet.
    pub quiet_audio_peak_dbfs_threshold: f32,

    /// Experimental noise gate strength (0-100).
    ///
    /// This is applied to the captured audio at stop-time (offline) before WAV encoding.
    /// 0 disables the noise gate.
    pub noise_gate_strength: u8,
    /// LLM formatting configuration
    pub llm_config: LlmConfig,
    /// API keys for all configured LLM providers (provider id -> key)
    pub llm_api_keys: HashMap<String, String>,
    /// Path to local Whisper model (for local-whisper feature)
    #[cfg(feature = "local-whisper")]
    pub whisper_model_path: Option<std::path::PathBuf>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            input_device_name: None,
            max_duration_secs: 300.0, // 5 minutes max
            stt_provider: "groq".to_string(),
            stt_api_key: String::new(),
            stt_api_keys: HashMap::new(),
            stt_model: None,
            stt_transcription_prompt: None,
            retry_config: RetryConfig::default(),
            vad_config: VadAutoStopConfig::default(),
            transcription_timeout: DEFAULT_TRANSCRIPTION_TIMEOUT,
            max_recording_bytes: MAX_WAV_SIZE_BYTES,

            quiet_audio_gate_enabled: true,
            quiet_audio_min_duration_secs: DEFAULT_QUIET_AUDIO_MIN_DURATION_SECS,
            quiet_audio_rms_dbfs_threshold: DEFAULT_QUIET_AUDIO_RMS_DBFS_THRESHOLD,
            quiet_audio_peak_dbfs_threshold: DEFAULT_QUIET_AUDIO_PEAK_DBFS_THRESHOLD,

            noise_gate_strength: 0,

            llm_config: LlmConfig::default(),
            llm_api_keys: HashMap::new(),
            #[cfg(feature = "local-whisper")]
            whisper_model_path: None,
        }
    }
}

/// Internal state for the recording pipeline
struct PipelineInner {
    audio_capture: AudioCapture,
    stt_registry: SttRegistry,
    stt_provider_cache: HashMap<String, Arc<dyn SttProvider>>,
    llm_provider_cache: HashMap<String, Arc<dyn LlmProvider>>,
    state: PipelineState,
    config: PipelineConfig,
    /// Cancellation token for the current operation
    cancel_token: Option<CancellationToken>,

    /// Last captured audio (WAV bytes). Used for debugging/testing.
    last_wav_bytes: Option<Vec<u8>>,
}

impl PipelineInner {
    fn new(config: PipelineConfig) -> Self {
        let audio_capture = AudioCapture::with_vad_config(config.vad_config.clone());
        let mut inner = Self {
            audio_capture,
            stt_registry: SttRegistry::new(),
            stt_provider_cache: HashMap::new(),
            llm_provider_cache: HashMap::new(),
            state: PipelineState::Idle,
            config: config.clone(),
            cancel_token: None,
            last_wav_bytes: None,
        };
        inner.initialize_providers(&config);
        inner
    }

    fn get_or_create_stt_provider(
        &mut self,
        provider_id: &str,
        model: Option<String>,
    ) -> Result<Arc<dyn SttProvider>, PipelineError> {
        let provider_id = canonicalize_stt_provider_id(provider_id);
        let model_key = model.clone().unwrap_or_else(|| "<default>".to_string());
        let cache_key = format!("{}::{}", provider_id, model_key);

        if let Some(p) = self.stt_provider_cache.get(&cache_key) {
            return Ok(p.clone());
        }

        #[cfg(feature = "local-whisper")]
        if provider_id == "local-whisper" {
            if let Some(model_path) = &self.config.whisper_model_path {
                let provider = crate::stt::LocalWhisperProvider::new(model_path.clone())
                    .map_err(|e| PipelineError::Config(format!("Local Whisper init failed: {}", e)))?;
                let provider = Arc::new(provider);
                self.stt_provider_cache.insert(cache_key, provider.clone());
                return Ok(provider);
            }

            return Err(PipelineError::Config(
                "Local Whisper selected but no model path configured".to_string(),
            ));
        }

        let api_key = self
            .config
            .stt_api_keys
            .get(&provider_id)
            .cloned()
            .unwrap_or_default();

        if api_key.is_empty() {
            return Err(PipelineError::Config(format!(
                "STT provider '{}' requires an API key",
                provider_id
            )));
        }

        let provider: Arc<dyn SttProvider> = match provider_id.as_str() {
            "openai" => Arc::new(crate::stt::OpenAiSttProvider::new(
                api_key,
                model,
                self.config.stt_transcription_prompt.clone(),
            )),
            "groq" => Arc::new(crate::stt::GroqSttProvider::new(
                api_key,
                model,
                self.config.stt_transcription_prompt.clone(),
            )),
            "deepgram" => Arc::new(crate::stt::DeepgramSttProvider::new(api_key, model)),
            other => {
                return Err(PipelineError::Config(format!(
                    "Unknown STT provider: {}",
                    other
                )))
            }
        };

        self.stt_provider_cache.insert(cache_key, provider.clone());
        Ok(provider)
    }

    fn get_or_create_llm_provider(
        &mut self,
        provider_id: &str,
        model: Option<String>,
        timeout: Duration,
        ollama_url: Option<String>,
    ) -> Result<Arc<dyn LlmProvider>, PipelineError> {
        let model_key = model.clone().unwrap_or_else(|| "<default>".to_string());
        let url_key = ollama_url
            .clone()
            .unwrap_or_else(|| "<default-url>".to_string());
        let cache_key = format!(
            "{}::{}::{}::{}",
            provider_id,
            model_key,
            timeout.as_secs_f64(),
            url_key
        );

        if let Some(p) = self.llm_provider_cache.get(&cache_key) {
            return Ok(p.clone());
        }

        let api_key = if provider_id == "ollama" {
            String::new()
        } else {
            self.config
                .llm_api_keys
                .get(provider_id)
                .cloned()
                .unwrap_or_default()
        };

        if provider_id != "ollama" && api_key.is_empty() {
            return Err(PipelineError::Config(format!(
                "LLM provider '{}' requires an API key",
                provider_id
            )));
        }

        let cfg = LlmConfig {
            enabled: true,
            provider: provider_id.to_string(),
            api_key,
            model,
            ollama_url,
            timeout,
            ..Default::default()
        };

        let provider = create_llm_provider(&cfg);
        self.llm_provider_cache.insert(cache_key, provider.clone());
        Ok(provider)
    }

    fn initialize_providers(&mut self, config: &PipelineConfig) {
        // Clear caches on any config update.
        self.stt_provider_cache.clear();
        self.llm_provider_cache.clear();

        // Initialize STT providers
        self.stt_registry = SttRegistry::new();
        let canonical = canonicalize_stt_provider_id(&config.stt_provider);
        match self.get_or_create_stt_provider(&canonical, config.stt_model.clone()) {
            Ok(provider) => {
                self.stt_registry.register(&canonical, provider);
                let _ = self.stt_registry.set_current(&canonical);
            }
            Err(e) => {
                log::warn!(
                    "Pipeline: Default STT provider '{}' not initialized: {}",
                    canonical,
                    e
                );
            }
        }

        // Note: LLM providers are created on-demand per transcription based on the active profile.
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
    level_meter: crate::audio_capture::SharedAudioLevelMeter,
    waveform_meter: crate::audio_capture::SharedAudioWaveformMeter,
}

impl SharedPipeline {
    /// Create a new shared pipeline
    pub fn new(config: PipelineConfig) -> Self {
        let inner = PipelineInner::new(config);
        let level_meter = inner.audio_capture.shared_level_meter();
        let waveform_meter = inner.audio_capture.shared_waveform_meter();
        Self {
            inner: Arc::new(Mutex::new(inner)),
            level_meter,
            waveform_meter,
        }
    }

    /// Try to read the current state without blocking.
    ///
    /// This is useful for UI publishers that should not stall the runtime when
    /// the pipeline mutex is briefly held (e.g., during start-up).
    pub fn try_state(&self) -> Option<PipelineState> {
        self.inner.try_lock().ok().map(|inner| inner.state)
    }

    /// Get the most recent realtime audio input level snapshot without locking
    /// the pipeline mutex.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn audio_level_snapshot_fast(&self) -> AudioLevelSnapshot {
        self.level_meter.snapshot()
    }

    /// Get the most recent realtime waveform min/max buckets without locking the
    /// pipeline mutex.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn audio_waveform_snapshot_fast(&self) -> crate::audio_capture::AudioWaveformSnapshot {
        self.waveform_meter.snapshot()
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
        // Clone out of the config to avoid borrowing `inner` immutably while calling into
        // `audio_capture` mutably.
        let input_device_name = inner.config.input_device_name.clone();
        match inner
            .audio_capture
            .start_with_device_name(max_duration, input_device_name.as_deref())
        {
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
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn stop_recording(&self) -> Result<Vec<u8>, PipelineError> {
        let mut inner = self.inner.lock().map_err(|e| PipelineError::Lock(e.to_string()))?;

        if !inner.state.can_stop_recording() {
            return Err(PipelineError::NotRecording);
        }

        let noise_gate_strength = inner.config.noise_gate_strength;
        match inner
            .audio_capture
            .stop_and_get_wav_with_noise_gate(noise_gate_strength)
        {
            Ok(wav_bytes) => {
                // Keep a copy for STT testing/debugging UI.
                inner.last_wav_bytes = Some(wav_bytes.clone());

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

    /// Transcribe the last captured audio (WAV bytes) using the current effective STT settings.
    ///
    /// This is intended for settings UI testing and debugging.
    pub async fn transcribe_last_audio_for_profile(
        &self,
        profile_id: Option<&str>,
    ) -> Result<String, PipelineError> {
        let (wav_bytes, stt_provider, retry_config, cancel_token) = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|e| PipelineError::Lock(e.to_string()))?;

            let wav_bytes = inner
                .last_wav_bytes
                .clone()
                .ok_or_else(|| {
                    PipelineError::Config(
                        "No audio captured yet. Record once to create test audio.".to_string(),
                    )
                })?;

            let config = inner.config.clone();

            // Resolve per-profile overrides. Note: program prompt profiles live under llm_config.
            let profile = profile_id
                .and_then(|id| {
                    if id == "default" {
                        None
                    } else {
                        Some(id)
                    }
                })
                .and_then(|id| {
                    config
                        .llm_config
                        .program_prompt_profiles
                        .iter()
                        .find(|p| p.id == id)
                        .cloned()
                });

            let desired_stt_provider = canonicalize_stt_provider_id(
                profile
                    .as_ref()
                    .and_then(|p| p.stt_provider.as_deref())
                    .unwrap_or(config.stt_provider.as_str()),
            );
            let desired_stt_model = profile
                .as_ref()
                .and_then(|p| p.stt_model.clone())
                .or_else(|| config.stt_model.clone());

            let stt_provider = match inner.get_or_create_stt_provider(
                &desired_stt_provider,
                desired_stt_model.clone(),
            ) {
                Ok(p) => p,
                Err(e) => {
                    // If the profile specified an override provider, fall back to global provider.
                    let global_provider = canonicalize_stt_provider_id(&config.stt_provider);
                    if global_provider != desired_stt_provider {
                        log::warn!(
                            "Pipeline: Profile STT provider '{}' unavailable ({}), falling back to '{}'",
                            desired_stt_provider,
                            e,
                            global_provider
                        );

                        let global_model = config.stt_model.clone();
                        inner
                            .get_or_create_stt_provider(&global_provider, global_model)
                            .map_err(|err| {
                                inner.set_error(&format!(
                                    "No STT provider configured: {}",
                                    err
                                ));
                                PipelineError::NoProvider
                            })?
                    } else {
                        inner.set_error(&format!("No STT provider configured: {}", e));
                        return Err(PipelineError::NoProvider);
                    }
                }
            };

            let cancel_token = inner
                .cancel_token
                .clone()
                .unwrap_or_else(CancellationToken::new);

            (
                wav_bytes,
                stt_provider,
                config.retry_config.clone(),
                cancel_token,
            )
        };

        let wav = Arc::new(wav_bytes);
        let format = AudioFormat::default();

        let transcription_future = async {
            with_retry(&retry_config, || {
                let provider = stt_provider.clone();
                let wav = wav.clone();
                let format = format.clone();

                async move {
                    provider.transcribe(wav.as_slice(), &format).await
                }
            })
            .await
        };

        // Cancellation protection (test endpoint intentionally does NOT enforce timeout)
        tokio::select! {
            biased;

            _ = cancel_token.cancelled() => {
                Err(PipelineError::Cancelled)
            }

            result = transcription_future => {
                result
                    .map(normalize_stt_text)
                    .map_err(PipelineError::from)
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
        let (wav_bytes, stt_provider, llm_provider, llm_prompts, llm_timeout, retry_config, timeout, cancel_token) = {
            let mut inner = self.inner.lock().map_err(|e| PipelineError::Lock(e.to_string()))?;

            if !inner.state.can_stop_recording() {
                return Err(PipelineError::NotRecording);
            }

            let noise_gate_strength = inner.config.noise_gate_strength;
            let (wav_bytes, stats) = match inner
                .audio_capture
                .stop_and_get_wav_with_stats_with_noise_gate(noise_gate_strength)
            {
                Ok(out) => out,
                Err(e) => {
                    inner.set_error(&format!("Failed to stop recording: {}", e));
                    return Err(PipelineError::AudioCapture(e));
                }
            };

            // Keep a copy for STT testing/debugging UI.
            inner.last_wav_bytes = Some(wav_bytes.clone());

            if inner.config.quiet_audio_gate_enabled
                && is_effectively_quiet(
                    stats,
                    inner.config.quiet_audio_min_duration_secs,
                    inner.config.quiet_audio_rms_dbfs_threshold,
                    inner.config.quiet_audio_peak_dbfs_threshold,
                )
            {
                log::info!(
                    "Pipeline: Skipping STT because recording is quiet (duration {:.2}s, rms {:.1} dBFS, peak {:.1} dBFS)",
                    stats.duration_secs,
                    amp_to_dbfs(stats.rms),
                    amp_to_dbfs(stats.peak)
                );

                inner.reset_to_idle();
                return Ok(TranscriptionResult {
                    stt_text: String::new(),
                    final_text: String::new(),
                    stt_duration_ms: 0,
                    llm_duration_ms: None,
                    llm_provider_used: None,
                    llm_model_used: None,
                    llm_outcome: LlmOutcome::NotAttempted,
                });
            }

            // Check size limit
            let max_bytes = inner.config.max_recording_bytes;
            if max_bytes > 0 && wav_bytes.len() > max_bytes {
                inner.set_error(&format!("Recording too large: {} bytes", wav_bytes.len()));
                return Err(PipelineError::RecordingTooLarge(wav_bytes.len(), max_bytes));
            }

            inner.state = PipelineState::Transcribing;

            let llm_config = inner.config.llm_config.clone();
            let active_profile = select_profile_for_foreground_app(&llm_config);
            let llm_prompts = active_profile
                .as_ref()
                .map(|p| p.prompts.clone())
                .unwrap_or_else(|| llm_config.prompts.clone());

            // Resolve effective STT settings (profile overrides -> global defaults, with safe fallback)
            let desired_stt_provider = canonicalize_stt_provider_id(
                active_profile
                    .as_ref()
                    .and_then(|p| p.stt_provider.as_deref())
                    .unwrap_or(inner.config.stt_provider.as_str()),
            );
            let desired_stt_model = active_profile
                .as_ref()
                .and_then(|p| p.stt_model.clone())
                .or_else(|| inner.config.stt_model.clone());
            let desired_timeout = active_profile
                .as_ref()
                .and_then(|p| p.stt_timeout_seconds)
                .map(|s| seconds_to_duration_or(s, inner.config.transcription_timeout))
                .unwrap_or(inner.config.transcription_timeout);

            let stt_provider = match inner.get_or_create_stt_provider(&desired_stt_provider, desired_stt_model.clone()) {
                Ok(p) => p,
                Err(e) => {
                    // If the profile specified an override provider, fall back to global provider.
                    let global_provider = canonicalize_stt_provider_id(&inner.config.stt_provider);
                    if global_provider != desired_stt_provider {
                        log::warn!(
                            "Pipeline: Profile STT provider '{}' unavailable ({}), falling back to '{}'",
                            desired_stt_provider,
                            e,
                            global_provider
                        );
                        let global_model = inner.config.stt_model.clone();
                        inner.get_or_create_stt_provider(&global_provider, global_model)
                            .map_err(|err| {
                                inner.set_error(&format!("No STT provider configured: {}", err));
                                PipelineError::NoProvider
                            })?
                    } else {
                        inner.set_error(&format!("No STT provider configured: {}", e));
                        return Err(PipelineError::NoProvider);
                    }
                }
            };

            // Resolve effective LLM provider/model (profile overrides -> global defaults), gated by
            // the active profile's enable flag (falls back to the global enable).
            let llm_timeout = llm_config.timeout;
            let effective_llm_enabled = active_profile
                .as_ref()
                .and_then(|p| p.rewrite_llm_enabled)
                .unwrap_or(inner.config.llm_config.enabled);

            let llm_provider = if effective_llm_enabled {
                let desired_llm_provider = active_profile
                    .as_ref()
                    .and_then(|p| p.llm_provider.clone())
                    .unwrap_or_else(|| llm_config.provider.clone());
                let desired_llm_model = active_profile
                    .as_ref()
                    .and_then(|p| p.llm_model.clone())
                    .or_else(|| llm_config.model.clone());

                match inner.get_or_create_llm_provider(
                    desired_llm_provider.as_str(),
                    desired_llm_model.clone(),
                    llm_timeout,
                    llm_config.ollama_url.clone(),
                ) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        // Fallback to global provider if profile requested a different one.
                        if active_profile
                            .as_ref()
                            .and_then(|p| p.llm_provider.as_ref())
                            .is_some()
                            && desired_llm_provider != llm_config.provider
                        {
                            log::warn!(
                                "Pipeline: Profile LLM provider '{}' unavailable ({}), falling back to '{}'",
                                desired_llm_provider,
                                e,
                                llm_config.provider
                            );
                            inner
                                .get_or_create_llm_provider(
                                    llm_config.provider.as_str(),
                                    llm_config.model.clone(),
                                    llm_timeout,
                                    llm_config.ollama_url.clone(),
                                )
                                .ok()
                        } else {
                            log::warn!(
                                "Pipeline: LLM disabled for this transcription ({}).",
                                e
                            );
                            None
                        }
                    }
                }
            } else {
                None
            };

            let retry_config = inner.config.retry_config.clone();
            let cancel_token = inner.cancel_token.clone().unwrap_or_else(CancellationToken::new);

            (
                wav_bytes,
                stt_provider,
                llm_provider,
                llm_prompts,
                llm_timeout,
                retry_config,
                desired_timeout,
                cancel_token,
            )
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
            Ok(t) => normalize_stt_text(t),
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

        // Capture the *actual* provider/model that will be used (including provider defaults)
        // before we move `llm_provider` into the formatting block.
        let llm_provider_used: Option<String> = llm_provider.as_ref().map(|p| p.name().to_string());
        let llm_model_used: Option<String> = llm_provider.as_ref().map(|p| p.model().to_string());

        let final_text = if let Some(llm) = llm_provider {
            // Expose the optional LLM step as a distinct phase for UI.
            {
                let mut inner = self
                    .inner
                    .lock()
                    .map_err(|e| PipelineError::Lock(e.to_string()))?;
                if inner.state == PipelineState::Transcribing {
                    inner.state = PipelineState::Rewriting;
                }
            }

            log::info!("Pipeline: Applying LLM formatting");

            llm_outcome = LlmOutcome::Succeeded; // may be overwritten by fallback paths
            let llm_start = std::time::Instant::now();

            // Apply LLM formatting with timeout
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
            llm_provider_used,
            llm_model_used,
            llm_outcome,
        })
    }

    /// Transcribe provided WAV bytes using the same STT + optional LLM logic as the main pipeline.
    ///
    /// This is used for retrying failed requests from persisted audio.
    pub async fn transcribe_wav_bytes_detailed(
        &self,
        wav_bytes: Vec<u8>,
    ) -> Result<TranscriptionResult, PipelineError> {
        // Phase 1: Resolve providers/config under lock.
        let (stt_provider, llm_provider, llm_prompts, llm_timeout, retry_config, timeout, cancel_token) = {
            let mut inner = self.inner.lock().map_err(|e| PipelineError::Lock(e.to_string()))?;

            // Guard: don't run a retry while actively recording.
            if inner.state == PipelineState::Recording {
                return Err(PipelineError::AlreadyRecording);
            }
            if matches!(inner.state, PipelineState::Transcribing | PipelineState::Rewriting) {
                return Err(PipelineError::Lock("Pipeline already transcribing".to_string()));
            }

            // Keep a copy for STT testing/debugging UI.
            inner.last_wav_bytes = Some(wav_bytes.clone());

            // Check size limit
            let max_bytes = inner.config.max_recording_bytes;
            if max_bytes > 0 && wav_bytes.len() > max_bytes {
                inner.set_error(&format!("Recording too large: {} bytes", wav_bytes.len()));
                return Err(PipelineError::RecordingTooLarge(wav_bytes.len(), max_bytes));
            }

            inner.state = PipelineState::Transcribing;

            // Ensure we have a cancellation token for this attempt.
            let cancel_token = CancellationToken::new();
            inner.cancel_token = Some(cancel_token.clone());

            let llm_config = inner.config.llm_config.clone();
            let active_profile = select_profile_for_foreground_app(&llm_config);
            let llm_prompts = active_profile
                .as_ref()
                .map(|p| p.prompts.clone())
                .unwrap_or_else(|| llm_config.prompts.clone());

            // Resolve effective STT settings (profile overrides -> global defaults, with safe fallback)
            let desired_stt_provider = canonicalize_stt_provider_id(
                active_profile
                    .as_ref()
                    .and_then(|p| p.stt_provider.as_deref())
                    .unwrap_or(inner.config.stt_provider.as_str()),
            );
            let desired_stt_model = active_profile
                .as_ref()
                .and_then(|p| p.stt_model.clone())
                .or_else(|| inner.config.stt_model.clone());
            let desired_timeout = active_profile
                .as_ref()
                .and_then(|p| p.stt_timeout_seconds)
                .map(|s| seconds_to_duration_or(s, inner.config.transcription_timeout))
                .unwrap_or(inner.config.transcription_timeout);

            let stt_provider = match inner.get_or_create_stt_provider(&desired_stt_provider, desired_stt_model.clone()) {
                Ok(p) => p,
                Err(e) => {
                    // If the profile specified an override provider, fall back to global provider.
                    let global_provider = canonicalize_stt_provider_id(&inner.config.stt_provider);
                    if global_provider != desired_stt_provider {
                        log::warn!(
                            "Pipeline: Profile STT provider '{}' unavailable ({}), falling back to '{}'",
                            desired_stt_provider,
                            e,
                            global_provider
                        );
                        let global_model = inner.config.stt_model.clone();
                        inner.get_or_create_stt_provider(&global_provider, global_model)
                            .map_err(|err| {
                                inner.set_error(&format!("No STT provider configured: {}", err));
                                PipelineError::NoProvider
                            })?
                    } else {
                        inner.set_error(&format!("No STT provider configured: {}", e));
                        return Err(PipelineError::NoProvider);
                    }
                }
            };

            // Resolve effective LLM provider/model (profile overrides -> global defaults)
            let llm_timeout = llm_config.timeout;
            let effective_llm_enabled = active_profile
                .as_ref()
                .and_then(|p| p.rewrite_llm_enabled)
                .unwrap_or(inner.config.llm_config.enabled);

            let llm_provider = if effective_llm_enabled {
                let desired_llm_provider = active_profile
                    .as_ref()
                    .and_then(|p| p.llm_provider.clone())
                    .unwrap_or_else(|| llm_config.provider.clone());
                let desired_llm_model = active_profile
                    .as_ref()
                    .and_then(|p| p.llm_model.clone())
                    .or_else(|| llm_config.model.clone());

                match inner.get_or_create_llm_provider(
                    desired_llm_provider.as_str(),
                    desired_llm_model.clone(),
                    llm_timeout,
                    llm_config.ollama_url.clone(),
                ) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        // Fallback to global provider if profile requested a different one.
                        if active_profile
                            .as_ref()
                            .and_then(|p| p.llm_provider.as_ref())
                            .is_some()
                            && desired_llm_provider != llm_config.provider
                        {
                            log::warn!(
                                "Pipeline: Profile LLM provider '{}' unavailable ({}), falling back to '{}'",
                                desired_llm_provider,
                                e,
                                llm_config.provider
                            );
                            inner
                                .get_or_create_llm_provider(
                                    llm_config.provider.as_str(),
                                    llm_config.model.clone(),
                                    llm_timeout,
                                    llm_config.ollama_url.clone(),
                                )
                                .ok()
                        } else {
                            log::warn!("Pipeline: LLM disabled for this transcription ({})", e);
                            None
                        }
                    }
                }
            } else {
                None
            };

            let retry_config = inner.config.retry_config.clone();

            (
                stt_provider,
                llm_provider,
                llm_prompts,
                llm_timeout,
                retry_config,
                desired_timeout,
                cancel_token,
            )
        };

        log::info!(
            "Pipeline: Starting retry transcription ({} bytes, timeout {:?})",
            wav_bytes.len(),
            timeout
        );

        // Phase 2: STT transcription
        let format = AudioFormat::default();
        let wav = Arc::new(wav_bytes);

        let transcription_future = async {
            with_retry(&retry_config, || {
                let provider = stt_provider.clone();
                let wav = wav.clone();
                let format = format.clone();
                async move { provider.transcribe(wav.as_slice(), &format).await }
            })
            .await
        };

        let stt_start = std::time::Instant::now();
        let stt_result = tokio::select! {
            biased;

            _ = cancel_token.cancelled() => {
                log::info!("Pipeline: Retry transcription cancelled");
                Err(PipelineError::Cancelled)
            }

            _ = tokio::time::sleep(timeout) => {
                log::warn!("Pipeline: Retry transcription timed out after {:?}", timeout);
                Err(PipelineError::Timeout(timeout))
            }

            result = transcription_future => {
                result.map_err(PipelineError::from)
            }
        };

        let stt_text = match stt_result {
            Ok(t) => normalize_stt_text(t),
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
        log::info!("Pipeline: Retry STT complete, {} chars", stt_text.len());

        // Phase 3: Optional LLM formatting
        let mut llm_duration_ms: Option<u64> = None;
        let mut llm_outcome: LlmOutcome = LlmOutcome::NotAttempted;

        let llm_provider_used: Option<String> = llm_provider.as_ref().map(|p| p.name().to_string());
        let llm_model_used: Option<String> = llm_provider.as_ref().map(|p| p.model().to_string());

        let final_text = if let Some(llm) = llm_provider {
            // Expose the optional LLM step as a distinct phase for UI.
            {
                let mut inner = self
                    .inner
                    .lock()
                    .map_err(|e| PipelineError::Lock(e.to_string()))?;
                if inner.state == PipelineState::Transcribing {
                    inner.state = PipelineState::Rewriting;
                }
            }

            log::info!("Pipeline: Applying LLM formatting (retry)");
            llm_outcome = LlmOutcome::Succeeded;
            let llm_start = std::time::Instant::now();

            let llm_result = tokio::select! {
                biased;

                _ = cancel_token.cancelled() => {
                    log::info!("Pipeline: Retry LLM formatting cancelled");
                    Err(PipelineError::Cancelled)
                }

                _ = tokio::time::sleep(llm_timeout) => {
                    log::warn!("Pipeline: Retry LLM formatting timed out, using raw transcript");
                    llm_outcome = LlmOutcome::TimedOut;
                    Ok(stt_text.clone())
                }

                result = format_text(llm.as_ref(), &stt_text, &llm_prompts) => {
                    match result {
                        Ok(formatted) => {
                            log::info!("Pipeline: Retry LLM formatted {} -> {} chars", stt_text.len(), formatted.len());
                            Ok(formatted)
                        }
                        Err(e) => {
                            log::warn!("Pipeline: Retry LLM formatting failed ({}), using raw transcript", e);
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
                Err(_) => stt_text.clone(),
            }
        } else {
            stt_text.clone()
        };

        // Phase 4: Reset to idle
        {
            let mut inner = self.inner.lock().map_err(|e| PipelineError::Lock(e.to_string()))?;
            inner.reset_to_idle();
            log::info!("Pipeline: Retry complete, {} chars output", final_text.len());
        }

        Ok(TranscriptionResult {
            stt_text,
            final_text,
            stt_duration_ms,
            llm_duration_ms,
            llm_provider_used,
            llm_model_used,
            llm_outcome,
        })
    }

    /// Stop recording and transcribe the audio.
    ///
    /// Kept for backwards compatibility. Prefer `stop_and_transcribe_detailed`.
    #[cfg_attr(not(test), allow(dead_code))]
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

    /// Get a clone of the last captured WAV bytes, if present.
    pub fn clone_last_wav_bytes(&self) -> Option<Vec<u8>> {
        self.inner.lock().ok().and_then(|inner| inner.last_wav_bytes.clone())
    }

    /// Poll for VAD events (non-blocking)
    ///
    /// Returns the next VAD event if one is available, or None if no events are pending.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn poll_vad_event(&self) -> Option<AudioCaptureEvent> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.audio_capture.poll_vad_event())
    }

    /// Check if VAD auto-stop is enabled
    #[cfg_attr(not(test), allow(dead_code))]
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

    /// Get the most recent realtime audio input level snapshot.
    ///
    /// This is cheap and intended for UI metering (e.g., overlay waveform). The snapshot is
    /// updated from the CPAL input callback while recording.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn audio_level_snapshot(&self) -> AudioLevelSnapshot {
        self.inner
            .lock()
            .map(|inner| inner.audio_capture.level_snapshot())
            .unwrap_or(AudioLevelSnapshot {
                seq: 0,
                rms: 0.0,
                peak: 0.0,
            })
    }

    /// Get the name of the current STT provider
    #[cfg_attr(not(test), allow(dead_code))]
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

    /// Whether there is a previously captured audio buffer available for testing.
    pub fn has_last_audio(&self) -> bool {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.last_wav_bytes.as_ref().map(|b| !b.is_empty()))
            .unwrap_or(false)
    }

    /// Get the cancellation token for external use (e.g., for coordinating with other async tasks)
    #[cfg_attr(not(test), allow(dead_code))]
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
            level_meter: self.level_meter.clone(),
            waveform_meter: self.waveform_meter.clone(),
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
