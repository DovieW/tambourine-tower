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

use crate::audio_capture::{
    AudioCapture, AudioCaptureBackend, AudioCaptureDiagnostics, AudioCaptureEvent,
    AudioLevelSnapshot,
};
use crate::llm::{LlmConfig, LlmProvider, ProgramPreset, ProgramPromptProfile};
use crate::settings::store::SettingsReadMode;
use crate::settings_view;
use crate::stt::{StreamingSttSession, SttError, SttProvider, SttRegistry};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

mod batch_stt_orchestration;
#[cfg(test)]
use batch_stt_orchestration::is_managed_auth_token_error;
mod config;
#[cfg(test)]
#[path = "pipeline/tests/enterprise_mode_tests.rs"]
mod enterprise_mode_tests;
pub(crate) mod llm_provider;
pub(crate) mod llm_provider_request;
mod local_provider_lifecycle;
#[cfg(test)]
#[path = "pipeline/tests/managed_outage_tests.rs"]
mod managed_outage_tests;
#[cfg(test)]
#[path = "pipeline/tests/managed_personal_tests.rs"]
mod managed_personal_tests;
mod meeting_transcription;
mod ocr_session;
mod ocr_session_state;
mod profile_matcher;
mod profile_query;
mod profile_resolution;
mod recording;
mod routing;
mod state_machine;
mod stt_cloud_adapters;
mod stt_flow;
pub(crate) mod stt_provider;
mod stt_provider_resolver;
#[cfg(test)]
mod tests;
mod transcription_flow;
mod types;
mod utils;

enum SavedAudioMode<'a> {
    Dictation,
    Journal(
        Option<&'a std::path::Path>,
        &'a crate::recordings::options::RecordingPreferences,
    ),
}

use config::canonicalize_stt_provider_id;
pub(crate) use config::{resolve_provider_mode, ProviderMode};
pub use config::{OcrConfig, PipelineConfig};
pub(crate) use profile_query::{
    program_basename_for_log, resolve_profile_by_id, resolve_profile_for_foreground_app,
};

pub(crate) fn normalize_stt_language_setting(raw: Option<String>) -> Option<String> {
    config::normalize_stt_language_setting(raw)
}

pub use state_machine::PipelineState;
pub use types::{LlmNotAttemptedReason, LlmOutcome, PipelineError, TranscriptionResult};

use profile_matcher::select_profile_for_program_path;
pub(crate) use profile_resolution::{
    resolve_request_profile_context, ActiveWindowOcrModeFallbacks, DefaultProfileSelectionPolicy,
};

pub(crate) fn select_profile_for_foreground_app(
    llm_config: &LlmConfig,
) -> Option<ProgramPromptProfile> {
    let foreground = crate::windows_apps::get_foreground_process_path();
    let Some(foreground) = foreground else {
        log::debug!(
            "Pipeline: Foreground process path unavailable; cannot select per-program profile (profiles={})",
            llm_config.program_prompt_profiles.len()
        );
        return None;
    };

    select_profile_for_program_path(llm_config, &foreground)
}

#[cfg(test)]
use llm_provider::llm_provider_cache_key;
use llm_provider::{create_llm_provider, resolve_cached_llm_provider_config, LlmProviderParams};
use local_provider_lifecycle as local_provider;
use ocr_session_state::OcrSessionState;
use stt_flow::run_stt_transcription;
use stt_provider_resolver::SttProviderResolutionRequest;
use utils::seconds_to_duration_or;

fn managed_gateway_ready(config: &PipelineConfig) -> bool {
    let gateway_ready = config
        .managed_inference_gateway_url
        .as_deref()
        .map(str::trim)
        .map(|url| !url.is_empty())
        .unwrap_or(false);
    let token_ready = config
        .managed_inference_access_token
        .as_deref()
        .map(str::trim)
        .map(|token| !token.is_empty())
        .unwrap_or(false);

    gateway_ready && token_ready
}

fn resolve_stt_provider_for_runtime(
    config: &PipelineConfig,
    requested_provider_id: &str,
    requested_model: Option<&str>,
) -> String {
    let requested = canonicalize_stt_provider_id(requested_provider_id);
    if !config.managed_inference_enabled
        || !config.managed_stt_preferred
        || !stt_provider_resolver::managed_stt_model_supported(requested.as_str(), requested_model)
    {
        return requested;
    }

    if managed_gateway_ready(config) {
        return requested;
    }

    let fallback = config
        .managed_inference_fallback_stt_provider
        .as_deref()
        .map(canonicalize_stt_provider_id)
        .filter(|provider| !provider.trim().is_empty())
        .filter(|provider| provider != &requested)
        .unwrap_or_else(|| requested.clone());

    if fallback != requested {
        log::warn!(
            "Pipeline: managed gateway unavailable, falling back STT provider '{}' -> '{}'",
            requested,
            fallback
        );
    }

    fallback
}

fn resolve_llm_provider_for_runtime(
    config: &PipelineConfig,
    requested_provider_id: &str,
) -> String {
    let requested = requested_provider_id.trim();

    // The provider ID is the inference-source contract. Entitlement makes the
    // `managed` provider available; it must never silently turn a named BYOK
    // provider into managed traffic.
    if requested != "managed" {
        return requested.to_string();
    }

    if config.managed_inference_enabled && managed_gateway_ready(config) {
        return requested.to_string();
    }

    let fallback = config
        .managed_inference_fallback_llm_provider
        .as_deref()
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .filter(|provider| *provider != "managed")
        .filter(|provider| *provider != requested)
        .unwrap_or(requested)
        .to_string();

    if fallback != requested {
        log::warn!(
            "Pipeline: managed gateway unavailable, falling back LLM provider '{}' -> '{}'",
            requested,
            fallback
        );
    }

    fallback
}

// Intent routing helpers live in `pipeline/routing.rs`.

/// Internal state for the recording pipeline
struct PipelineInner {
    recovery_job: Option<CancellationToken>,
    history_only: bool,
    recording_paused: bool,
    audio_capture: Box<dyn AudioCaptureBackend>,
    stt_registry: SttRegistry,
    stt_provider_cache: HashMap<String, Arc<dyn SttProvider>>,
    llm_provider_cache: HashMap<String, Arc<dyn LlmProvider>>,
    /// Injected embeddings provider for testing (bypasses real API calls).
    injected_embeddings_provider: Option<Arc<dyn crate::embeddings::EmbeddingsProvider>>,
    state: PipelineState,
    config: PipelineConfig,
    /// Cancellation token for the current operation
    cancel_token: Option<CancellationToken>,

    /// Request-owned active-window OCR session state.
    ///
    /// This is intentionally decoupled from the pipeline's internal state machine so that
    /// best-effort OCR can remain consumable across internal transitions like `reset_to_idle()`.
    ocr: OcrSessionState,

    /// True after STT portion completes but before LLM / output.
    ///
    /// Used by the overlay to indicate "waiting for OCR" when the pipeline is
    /// still in Transcribing state but STT work has finished.
    stt_complete: bool,

    /// Last captured audio (WAV bytes). Used for debugging/testing.
    last_wav_bytes: Option<Vec<u8>>,

    /// Last recording diagnostics (raw stats + optional speech detection).
    last_recording_diagnostics: Option<AudioCaptureDiagnostics>,

    /// Active concurrent STT streaming session (if the provider supports it).
    ///
    /// Created when recording starts with a streaming-capable provider.
    /// Consumed when recording stops for near-instant transcription.
    active_streaming_session: Option<StreamingSttSession>,

    /// Whether live output (streaming paste) is actively pasting committed
    /// chunks during the current recording session.
    ///
    /// Set to `true` when the partial consumer starts pasting committed chunks.
    /// Read at stop-time to decide whether to skip the final paste step.
    live_output_active: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

struct EffectiveSttSettings {
    provider_id: String,
    model: Option<String>,
    language: Option<String>,
    timeout: Duration,
}

impl PipelineInner {
    fn resolve_effective_stt_settings(
        &self,
        active_profile: Option<&ProgramPromptProfile>,
        active_preset: Option<&ProgramPreset>,
    ) -> EffectiveSttSettings {
        let provider_id = canonicalize_stt_provider_id(
            active_preset
                .and_then(|p| p.stt_provider.as_deref())
                .or_else(|| active_profile.and_then(|p| p.stt_provider.as_deref()))
                .unwrap_or(self.config.stt_provider.as_str()),
        );
        let model = active_preset
            .and_then(|p| p.stt_model.clone())
            .or_else(|| active_profile.and_then(|p| p.stt_model.clone()))
            .or_else(|| self.config.stt_model.clone());
        let language = normalize_stt_language_setting(
            active_preset
                .and_then(|p| p.stt_language.clone())
                .or_else(|| active_profile.and_then(|p| p.stt_language.clone()))
                .or_else(|| self.config.stt_language.clone()),
        );
        let timeout = active_preset
            .and_then(|p| p.stt_timeout_seconds)
            .or_else(|| active_profile.and_then(|p| p.stt_timeout_seconds))
            .map(|s| seconds_to_duration_or(s, self.config.transcription_timeout))
            .unwrap_or(self.config.transcription_timeout);

        EffectiveSttSettings {
            provider_id,
            model,
            language,
            timeout,
        }
    }

    fn cancel_ocr_task(&mut self, mark_cancelled: bool) {
        self.ocr.cancel_task(mark_cancelled);
    }

    fn transition_to(&mut self, next: PipelineState, context: &str) {
        if self.state.can_transition_to(next) {
            self.state = next;
            return;
        }

        self.set_error(&format!(
            "Invalid pipeline state transition {:?} -> {:?} ({})",
            self.state, next, context
        ));
    }
    fn local_whisper_model_key_for_cache(&self) -> String {
        #[cfg(feature = "local-whisper")]
        let model_path = self.config.whisper_model_path.as_deref();

        #[cfg(not(feature = "local-whisper"))]
        let model_path = None;

        local_provider::local_whisper_model_key_for_cache(
            model_path,
            cfg!(feature = "local-whisper"),
        )
    }

    fn local_whisper_cache_key_for_language(&self, language: Option<&str>) -> String {
        local_provider::local_whisper_cache_key_for_language(
            &self.local_whisper_model_key_for_cache(),
            language,
        )
    }

    fn local_whisper_cache_key(&self) -> String {
        self.local_whisper_cache_key_for_language(self.config.stt_language.as_deref())
    }

    fn is_local_whisper_loaded(&self) -> bool {
        let key = self.local_whisper_cache_key();
        local_provider::local_whisper_cache_contains(&self.stt_provider_cache, &key)
    }

    fn unload_local_whisper(&mut self) {
        local_provider::retain_after_local_whisper_unload(&mut self.stt_provider_cache);
    }

    fn force_load_local_whisper(&mut self) -> Result<(), PipelineError> {
        let cache_key = self.local_whisper_cache_key();

        if local_provider::local_whisper_cache_contains(&self.stt_provider_cache, &cache_key) {
            return Ok(());
        }

        #[cfg(feature = "local-whisper")]
        {
            let Some(model_path) = &self.config.whisper_model_path else {
                return Err(PipelineError::Config(
                    "Local Whisper: no model path configured".to_string(),
                ));
            };

            let provider =
                crate::stt::LocalWhisperProvider::with_config(crate::stt::LocalWhisperConfig {
                    model_path: model_path.clone(),
                    language: self.config.stt_language.clone(),
                    transcription_prompt: self.config.stt_transcription_prompt.clone(),
                    ..Default::default()
                })
                .map_err(|e| PipelineError::Config(format!("Local Whisper init failed: {}", e)))?;
            let provider = Arc::new(provider);
            local_provider::insert_loaded_local_whisper(
                &mut self.stt_provider_cache,
                cache_key,
                provider,
            );
            return Ok(());
        }

        #[cfg(not(feature = "local-whisper"))]
        {
            Err(PipelineError::Config(
                "Local Whisper feature is not enabled".to_string(),
            ))
        }
    }

    #[allow(dead_code)]
    fn build_http_client_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<reqwest::Client, PipelineError> {
        crate::network::build_http_client_with_timeout(&self.config.proxy_settings, timeout)
            .map_err(|e| PipelineError::Config(format!("Failed to create HTTP client: {}", e)))
    }

    fn new_with_audio_capture(
        config: PipelineConfig,
        audio_capture: Box<dyn AudioCaptureBackend>,
    ) -> Self {
        let mut inner = Self {
            history_only: false,
            recovery_job: None,
            recording_paused: false,
            audio_capture,
            stt_registry: SttRegistry::new(),
            stt_provider_cache: HashMap::new(),
            llm_provider_cache: HashMap::new(),
            injected_embeddings_provider: None,
            state: PipelineState::Idle,
            config: config.clone(),
            cancel_token: None,
            ocr: OcrSessionState::default(),
            stt_complete: false,
            last_wav_bytes: None,
            last_recording_diagnostics: None,
            active_streaming_session: None,
            live_output_active: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        inner.initialize_providers(&config);
        inner
    }

    fn new(config: PipelineConfig) -> Self {
        Self::new_with_audio_capture(
            config.clone(),
            Box::new(AudioCapture::with_vad_config(config.vad_config.clone())),
        )
    }

    fn get_or_create_stt_provider(
        &mut self,
        provider_id: &str,
        model: Option<String>,
        language: Option<String>,
    ) -> Result<Arc<dyn SttProvider>, PipelineError> {
        stt_provider_resolver::get_or_create_stt_provider(self, provider_id, model, language)
    }

    fn get_or_create_llm_provider(
        &mut self,
        provider_id: &str,
        params: LlmProviderParams,
    ) -> Result<Arc<dyn LlmProvider>, PipelineError> {
        let resolved = resolve_cached_llm_provider_config(&self.config, provider_id, params)?;

        if resolved.managed_transport_active {
            if let Some(store) = &self.config.request_log_store {
                let _ = store.with_current(|log| {
                    log.managed_inference = true;
                });
            }
        }

        if let Some(p) = self.llm_provider_cache.get(&resolved.cache_key) {
            return Ok(p.clone());
        }

        let provider = create_llm_provider(
            &resolved.config,
            self.config.request_log_store.clone(),
            &self.config.proxy_settings,
        )?;
        self.llm_provider_cache
            .insert(resolved.cache_key, provider.clone());
        Ok(provider)
    }

    fn initialize_providers(&mut self, config: &PipelineConfig) {
        // Clear caches on config updates.
        // IMPORTANT: keep any cached local-whisper models unless we explicitly evicted them
        // (e.g. model path / transcription prompt changed). This prevents expensive model
        // reloads during routine config sync and makes "on_launch" preload actually stick.
        self.stt_provider_cache
            .retain(|k, _| k.starts_with("local-whisper::"));
        self.llm_provider_cache.clear();

        // Initialize STT providers
        self.stt_registry = SttRegistry::new();
        let canonical = canonicalize_stt_provider_id(&config.stt_provider);

        // Avoid blocking the pipeline lock during config sync.
        // Local Whisper model initialization can take noticeable time and should be done
        // lazily (when we actually need to transcribe).
        #[cfg(feature = "local-whisper")]
        if canonical == "local-whisper" {
            self.stt_registry.set_current_name_for_ui(&canonical);
            return;
        }

        match self.get_or_create_stt_provider(
            &canonical,
            config.stt_model.clone(),
            config.stt_language.clone(),
        ) {
            Ok(provider) => {
                self.stt_registry.register(&canonical, provider);
                let _ = self.stt_registry.set_current(&canonical);
            }
            Err(e) => {
                // Keep the name for UI/telemetry even if provider init fails.
                self.stt_registry.set_current_name_for_ui(&canonical);
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
        self.transition_to(PipelineState::Idle, "reset_to_idle");
        self.cancel_token = None;
        // Clean up any active streaming session (e.g. quiet audio gate, cancellation).
        self.active_streaming_session.take();
        self.audio_capture.set_live_audio_tx(None);
        // NOTE: Do NOT reset stt_complete here. OCR may still be running for Quick Ask / Quick Replace,
        // which need the UI to show "OCR..." while waiting. stt_complete is reset when starting a new recording.
        //
        // IMPORTANT (Option A): do not implicitly cancel/clear OCR on reset-to-idle.
        //
        // We may still need OCR for post-transcription flows like Quick Ask / Quick Replace,
        // which run after the pipeline has returned to Idle.
        //
        // OCR should be cancelled/cleared only on explicit session end/cancel (Escape,
        // superseded by new session, force reset).
    }

    /// Transition to error state
    fn set_error(&mut self, msg: &str) {
        log::error!("Pipeline error: {}", msg);
        self.state = PipelineState::Error;
        self.cancel_token = None;
        self.stt_complete = false;
        // Clean up any active streaming session.
        self.active_streaming_session.take();
        self.audio_capture.set_live_audio_tx(None);
        self.cancel_ocr_task(false);
    }
}

// Re-export types from transcription_flow module (shared definitions)
use transcription_flow::{complete_transcription_flow, SessionPresetLock, TranscriptionContext};

/// Callbacks adapter for integrating transcription_flow with SharedPipeline.
///
/// This implements `TranscriptionCallbacks` by acquiring locks on the pipeline's
/// inner state as needed for state transitions and provider creation.
struct PipelineCallbacks {
    inner: Arc<Mutex<PipelineInner>>,
}

impl transcription_flow::TranscriptionCallbacks for PipelineCallbacks {
    fn transition_to_routing(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if inner.state == PipelineState::Transcribing {
                inner.transition_to(PipelineState::Routing, "transcription_flow (routing)");
            }
        }
    }

    fn transition_from_routing(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if inner.state == PipelineState::Routing {
                inner.transition_to(
                    PipelineState::Transcribing,
                    "transcription_flow (routing done)",
                );
            }
        }
    }

    fn transition_to_rewriting(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if inner.state == PipelineState::Transcribing {
                inner.transition_to(PipelineState::Rewriting, "transcription_flow (rewrite)");
            }
        }
    }

    fn get_or_create_llm_provider(
        &self,
        provider_id: &str,
        params: LlmProviderParams,
    ) -> Result<Arc<dyn LlmProvider>, PipelineError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| PipelineError::Lock(e.to_string()))?;
        inner.get_or_create_llm_provider(provider_id, params)
    }
}

/// Thread-safe wrapper for the recording pipeline
///
/// Uses standard Mutex to be Send + Sync for Tauri state management.
/// Provides robust error handling and cancellation support.

#[derive(Clone)]
pub struct SharedPipeline {
    inner: Arc<Mutex<PipelineInner>>,
    level_meter: crate::audio_capture::SharedAudioLevelMeter,
    waveform_meter: crate::audio_capture::SharedAudioWaveformMeter,
    embedding_cache: Arc<Mutex<HashMap<String, Vec<f32>>>>,
    app_handle: Arc<Mutex<Option<AppHandle>>>,
    session_preset_lock: Arc<Mutex<Option<SessionPresetLock>>>,
    session_profile_override: Arc<Mutex<Option<String>>>,
}

impl SharedPipeline {
    pub fn begin_recovery(&self) -> Result<CancellationToken, PipelineError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| PipelineError::Lock(e.to_string()))?;
        if inner.recovery_job.is_some() || !inner.state.can_start_recording() {
            return Err(PipelineError::AlreadyRecording);
        }
        let token = CancellationToken::new();
        inner.recovery_job = Some(token.clone());
        Ok(token)
    }

    pub fn end_recovery(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.recovery_job = None;
        }
    }

    pub fn is_recovering(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.recovery_job.is_some())
            .unwrap_or(true)
    }
    fn mark_stt_complete(&self, reason: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.stt_complete = true;
            log::debug!("stt_complete set to true ({})", reason);
        }
    }

    fn finish_failed_stt_attempt(&self, error: &PipelineError) -> Result<(), PipelineError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|err| PipelineError::Lock(err.to_string()))?;
        if matches!(error, PipelineError::Cancelled) {
            inner.reset_to_idle();
        } else {
            inner.set_error(&error.to_string());
        }
        Ok(())
    }

    /// Create a new shared pipeline
    pub fn new(config: PipelineConfig) -> Self {
        let inner = PipelineInner::new(config);
        let level_meter = inner.audio_capture.shared_level_meter();
        let waveform_meter = inner.audio_capture.shared_waveform_meter();
        Self {
            inner: Arc::new(Mutex::new(inner)),
            level_meter,
            waveform_meter,
            embedding_cache: Arc::new(Mutex::new(HashMap::new())),
            app_handle: Arc::new(Mutex::new(None)),
            session_preset_lock: Arc::new(Mutex::new(None)),
            session_profile_override: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    fn new_for_tests(config: PipelineConfig, audio_capture: Box<dyn AudioCaptureBackend>) -> Self {
        let inner = PipelineInner::new_with_audio_capture(config, audio_capture);
        let level_meter = inner.audio_capture.shared_level_meter();
        let waveform_meter = inner.audio_capture.shared_waveform_meter();
        Self {
            inner: Arc::new(Mutex::new(inner)),
            level_meter,
            waveform_meter,
            embedding_cache: Arc::new(Mutex::new(HashMap::new())),
            app_handle: Arc::new(Mutex::new(None)),
            session_preset_lock: Arc::new(Mutex::new(None)),
            session_profile_override: Arc::new(Mutex::new(None)),
        }
    }

    /// Test-only seam: inject an STT provider into the pipeline cache so we can run
    /// end-to-end pipeline tests without real network calls.
    ///
    /// This intentionally bypasses API-key validation in `get_or_create_stt_provider` by
    /// pre-populating the cache key the pipeline will look up.
    #[cfg(test)]
    fn inject_stt_provider_for_tests(
        &self,
        provider_id: &str,
        model: Option<&str>,
        language: Option<&str>,
        provider: Arc<dyn SttProvider>,
    ) {
        let mut inner = self.inner.lock().expect("pipeline lock");
        let provider_id = canonicalize_stt_provider_id(provider_id);

        let cache_key = stt_provider_resolver::stt_provider_cache_key(
            &inner,
            &provider_id,
            model.map(|s| s.to_string()),
            language.map(|s| s.to_string()),
        );

        inner.stt_provider_cache.insert(cache_key, provider.clone());
        inner.stt_registry.register(&provider_id, provider);
        let _ = inner.stt_registry.set_current(&provider_id);
    }

    /// Test-only seam: inject an LLM provider into the pipeline cache so we can run
    /// end-to-end pipeline tests with rewrite enabled without real network calls.
    #[cfg(test)]
    fn inject_llm_provider_for_tests(
        &self,
        provider_id: &str,
        model: Option<&str>,
        provider: Arc<dyn LlmProvider>,
    ) {
        let mut inner = self.inner.lock().expect("pipeline lock");

        let cache_key = llm_provider_cache_key(
            provider_id,
            &LlmProviderParams {
                model: model.map(|s| s.to_string()),
                timeout: Duration::from_secs(30),
                ollama_url: None,
                openai_reasoning_effort: None,
                gemini_thinking_budget: None,
                gemini_thinking_level: None,
                anthropic_thinking_budget: None,
            },
        );

        inner.llm_provider_cache.insert(cache_key, provider);
    }

    /// Test-only seam: inject an embeddings provider into the pipeline so we can run
    /// end-to-end pipeline tests with routing enabled without real network calls.
    #[cfg(test)]
    fn inject_embeddings_provider_for_tests(
        &self,
        provider: Arc<dyn crate::embeddings::EmbeddingsProvider>,
    ) {
        let mut inner = self.inner.lock().expect("pipeline lock");
        inner.injected_embeddings_provider = Some(provider);
    }

    /// Provide an app handle for best-effort persistence of recreatable caches.
    pub fn set_app_handle(&self, app: AppHandle) {
        if let Ok(mut guard) = self.app_handle.lock() {
            *guard = Some(app);
        }
    }

    /// Returns whether the STT portion has completed for the current session.
    ///
    /// Used by the overlay to distinguish "transcribing (STT in progress)" from
    /// "transcribing (waiting for OCR)" when `ocr_status == "running"`.
    pub(crate) fn is_stt_complete(&self) -> bool {
        self.inner.lock().map(|g| g.stt_complete).unwrap_or(false)
    }

    pub(crate) fn peek_session_profile_override(&self) -> Option<String> {
        self.session_profile_override
            .lock()
            .ok()
            .and_then(|g| g.clone())
    }

    /// Merge precomputed embeddings into the in-memory cache.
    ///
    /// This cache is used by embeddings routing to avoid recomputing per-preset hint embeddings.
    pub fn preload_embedding_cache(&self, entries: HashMap<String, Vec<f32>>) {
        if entries.is_empty() {
            return;
        }

        if let Ok(mut cache) = self.embedding_cache.lock() {
            // Keep cache bounded. (Note: persisted cache may be larger than the runtime cache.)
            if cache.len() + entries.len() > 2048 {
                cache.clear();
            }
            cache.extend(entries);
        }
    }

    pub fn embedding_cache_contains_key(&self, key: &str) -> bool {
        self.embedding_cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(key).map(|v| !v.is_empty()))
            .unwrap_or(false)
    }

    pub fn embedding_cache_get(&self, key: &str) -> Option<Vec<f32>> {
        self.embedding_cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(key).cloned())
            .and_then(|v| if v.is_empty() { None } else { Some(v) })
    }

    /// Set (or clear) the in-memory session profile override.
    ///
    /// This does not persist to disk. When set, the next transcription will prefer
    /// this profile id over selecting based on the current foreground application.
    ///
    /// This helps avoid Windows focus edge cases where our always-on-top overlay
    /// briefly becomes the foreground window during stop/transcribe.
    pub fn set_session_profile_override(
        &self,
        profile_id: Option<String>,
    ) -> Result<(), PipelineError> {
        let mut guard = self
            .session_profile_override
            .lock()
            .map_err(|e| PipelineError::Lock(e.to_string()))?;

        let normalized = profile_id.and_then(|s| {
            let t = s.trim().to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        });

        *guard = normalized;
        Ok(())
    }

    fn take_session_profile_override(&self) -> Option<String> {
        self.session_profile_override
            .lock()
            .ok()
            .and_then(|mut g| g.take())
    }

    /// Set (or clear) the in-memory session preset lock.
    ///
    /// This does not persist to disk. When set, it takes precedence over the
    /// persisted profile `active_preset_id` and intent router.
    pub fn set_session_preset_lock(
        &self,
        profile_id: Option<String>,
        preset_id: Option<String>,
    ) -> Result<(), PipelineError> {
        let mut lock = self
            .session_preset_lock
            .lock()
            .map_err(|e| PipelineError::Lock(e.to_string()))?;

        if let Some(preset_id) = preset_id.and_then(|s| {
            let t = s.trim().to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        }) {
            let pid = profile_id.and_then(|s| {
                let t = s.trim().to_string();
                if t.is_empty() {
                    None
                } else {
                    Some(t)
                }
            });
            *lock = Some(SessionPresetLock {
                profile_id: pid,
                preset_id,
            });
        } else {
            *lock = None;
        }

        Ok(())
    }

    /// Take (read and clear) the current session preset lock.
    fn take_session_preset_lock(&self) -> Option<SessionPresetLock> {
        self.session_preset_lock
            .lock()
            .ok()
            .and_then(|mut g| g.take())
    }

    /// Read (without clearing) the current session preset lock.
    pub fn peek_session_preset_lock(&self) -> Option<(Option<String>, String)> {
        let guard = self.session_preset_lock.lock().ok()?;
        guard
            .as_ref()
            .map(|lock| (lock.profile_id.clone(), lock.preset_id.clone()))
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
        self.start_recording_with_output(false, None, false)
    }

    pub fn is_history_only_recording(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.history_only)
            .unwrap_or(true)
    }

    pub fn recording_progress(&self) -> Result<f64, PipelineError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| PipelineError::Lock(e.to_string()))?;
        let (seconds, failed) = inner.audio_capture.recording_progress();
        if failed && matches!(inner.state, PipelineState::Recording | PipelineState::Error) {
            if inner.state == PipelineState::Recording {
                inner.audio_capture.stop();
                inner.set_error("Audio capture or recovery storage failed. Saved audio may be incomplete; recover it before recording again.");
            }
            return Err(PipelineError::AudioCapture(
                crate::audio_capture::AudioCaptureError::Encoding(
                    "Audio capture stopped. Recover the saved audio before recording again.".into(),
                ),
            ));
        }
        Ok(seconds)
    }

    pub fn recovery_path(&self) -> Option<std::path::PathBuf> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.audio_capture.recovery_path())
    }

    pub fn set_recording_paused(&self, paused: bool) -> Result<(), PipelineError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| PipelineError::Lock(e.to_string()))?;
        if inner.state != PipelineState::Recording || !inner.history_only {
            return Err(PipelineError::NotRecording);
        }
        inner
            .audio_capture
            .set_paused(paused)
            .map_err(PipelineError::AudioCapture)?;
        inner.recording_paused = paused;
        Ok(())
    }

    pub fn is_recording_paused(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.state == PipelineState::Recording && inner.recording_paused)
            .unwrap_or(false)
    }

    pub fn start_recording_with_output(
        &self,
        history_only: bool,
        recovery_path: Option<std::path::PathBuf>,
        computer_audio: bool,
    ) -> Result<(), PipelineError> {
        // Defensive: clear any previous session preset lock so we don't accidentally
        // apply an override from a prior (cancelled) session.
        //
        // The overlay/hotkey path can still set the lock again while recording.
        let _ = self.set_session_preset_lock(None, None);

        let mut inner = self
            .inner
            .lock()
            .map_err(|e| PipelineError::Lock(e.to_string()))?;

        // State guard: only allow starting from Idle or Error states
        if inner.recovery_job.is_some() || !recording::can_start_recording(inner.state) {
            return Err(PipelineError::AlreadyRecording);
        }

        // Create a new cancellation token for this session
        inner
            .audio_capture
            .set_computer_audio(history_only && computer_audio)
            .map_err(PipelineError::AudioCapture)?;
        inner
            .audio_capture
            .set_recovery_path(recovery_path)
            .map_err(PipelineError::AudioCapture)?;
        let cancel_token = CancellationToken::new();
        inner.cancel_token = Some(cancel_token);

        let max_duration = inner.config.max_duration_secs;
        // Clone out of the config to avoid borrowing `inner` immutably while calling into
        // `audio_capture` mutably.
        let input_device_name = inner.config.input_device_name.clone();
        match recording::start_recording_session(
            inner.audio_capture.as_mut(),
            max_duration,
            input_device_name.as_deref(),
        ) {
            Ok(()) => {
                inner.stt_complete = false;
                inner.history_only = history_only;
                inner.recording_paused = false;
                log::debug!("stt_complete reset to false (start_recording)");
                inner.transition_to(PipelineState::Recording, "start_recording");
                log::info!("Pipeline: Recording started");
                Ok(())
            }
            Err(e) => {
                inner.set_error(&format!("Failed to start recording: {}", e));
                Err(e)
            }
        }
    }

    /// Try to start a concurrent STT streaming session.
    ///
    /// If the current STT provider supports streaming (e.g. ElevenLabs Scribe v2),
    /// this connects to the STT service and wires up the audio capture to send
    /// chunks in real-time. When recording stops, the transcript will be
    /// near-instant instead of waiting for a full batch upload.
    ///
    /// This is fire-and-forget: if streaming setup fails, the pipeline will fall
    /// back to the normal batch path in `stop_and_transcribe_detailed`.
    pub async fn try_start_concurrent_streaming(&self, app_handle: &AppHandle) {
        // Home recordings must not feed the live dictation/insertion path.
        if self.is_history_only_recording() {
            return;
        }
        // Helper: log to the request log store (if available) so the user can
        // see streaming diagnostics in the UI.
        let log_to_request = |app: &AppHandle, msg: String| {
            if let Some(store) = app.try_state::<crate::request_log::RequestLogStore>() {
                store.with_current(|log| {
                    log.info(msg);
                });
            }
        };

        // 1. Resolve STT provider under the lock (brief).
        let (stt_provider, sample_rate, use_simulated, proxy_settings) = {
            let mut inner = match self.inner.lock() {
                Ok(inner) => inner,
                Err(e) => {
                    log::warn!("Concurrent streaming: lock failed: {}", e);
                    return;
                }
            };

            if inner.state != PipelineState::Recording {
                log::debug!("Concurrent streaming: not recording, skipping");
                return;
            }

            let effective = inner.resolve_effective_stt_settings(None, None);

            // Create the provider on-demand if it's not cached yet (e.g. first
            // recording after app start, or after a config sync that cleared the cache).
            let provider = match inner.get_or_create_stt_provider(
                &effective.provider_id,
                effective.model.clone(),
                effective.language.clone(),
            ) {
                Ok(p) => p,
                Err(e) => {
                    log::warn!(
                        "Concurrent streaming: failed to create STT provider '{}': {}",
                        effective.provider_id,
                        e
                    );
                    return;
                }
            };

            let use_simulated =
                !provider.supports_streaming() && inner.config.stt_simulated_streaming;

            if !provider.supports_streaming() && !use_simulated {
                log::debug!(
                    "Concurrent streaming: provider '{}' does not support streaming",
                    effective.provider_id
                );
                return;
            }

            let sr = inner.audio_capture.capture_sample_rate();
            (
                provider,
                sr,
                use_simulated,
                inner.config.proxy_settings.clone(),
            )
        };

        // 2. Start the streaming session (async, outside the lock).
        let mut session = if use_simulated {
            log::info!(
                "Simulated streaming: starting session (sample_rate={})",
                sample_rate
            );
            log_to_request(
                app_handle,
                format!(
                    "Simulated streaming: starting (sample_rate={})",
                    sample_rate
                ),
            );
            crate::stt::simulated_streaming::start_simulated_streaming(stt_provider, sample_rate)
        } else {
            log::info!(
                "Concurrent streaming: starting session (sample_rate={})",
                sample_rate
            );
            if let Some(message) =
                crate::stt::streaming::describe_websocket_transport_policy_gap(&proxy_settings)
            {
                log::warn!("{}", message);
                log_to_request(app_handle, message);
            }
            log_to_request(
                app_handle,
                format!(
                    "Realtime streaming: connecting (sample_rate={})",
                    sample_rate
                ),
            );
            match stt_provider.start_streaming(sample_rate).await {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("Concurrent streaming: failed to start: {}", e);
                    log_to_request(
                        app_handle,
                        format!("Realtime streaming: connection failed ({})", e),
                    );
                    return;
                }
            }
        };

        // 3. Wire up the audio channel and store the session (brief lock).
        let audio_tx = session.audio_tx.clone();
        let partial_rx = session.take_partial_rx();
        {
            let mut inner = match self.inner.lock() {
                Ok(inner) => inner,
                Err(e) => {
                    log::warn!("Concurrent streaming: lock failed (post-connect): {}", e);
                    return;
                }
            };

            if inner.state != PipelineState::Recording {
                log::info!(
                    "Concurrent streaming: recording stopped before session ready, aborting"
                );
                return;
            }

            inner.audio_capture.set_live_audio_tx(Some(audio_tx));
            inner.active_streaming_session = Some(session);
            log::info!("Concurrent streaming: session active, audio channel wired");
        }

        // 4. Spawn a task to forward partial transcripts as Tauri events
        //    and optionally paste committed chunks live.
        if let Some(mut partial_rx) = partial_rx {
            let app = app_handle.clone();

            // Read live output config under a brief lock.
            let (configured_live_output, live_output_flag) = {
                let inner = match self.inner.lock() {
                    Ok(inner) => inner,
                    Err(_) => {
                        log::warn!("Concurrent streaming: lock failed reading live output config");
                        return;
                    }
                };
                (
                    inner.config.stt_live_output,
                    inner.live_output_active.clone(),
                )
            };

            // Reset the flag at the start of a new session.
            live_output_flag.store(false, std::sync::atomic::Ordering::SeqCst);

            // Read output settings once (won't change mid-recording).
            let stt_live_output = configured_live_output
                && crate::platform_capabilities::automatic_text_injection_supported();
            if configured_live_output && !stt_live_output {
                log::warn!(
                    "Live output disabled: {}",
                    crate::text::inject::WAYLAND_CLIPBOARD_FALLBACK_MESSAGE
                );
                crate::emit_system_event(
                    &app,
                    "warning",
                    "Live output is unavailable in this Wayland session",
                    Some("The completed transcript will be copied to the clipboard."),
                );
            }

            let (output_mode, output_hit_enter) = if stt_live_output {
                let view = settings_view::read_output_settings_view(&app, SettingsReadMode::Cached);
                (view.mode, view.hit_enter)
            } else {
                (crate::text::inject::OutputMode::Paste, false)
            };

            tokio::spawn(async move {
                let mut is_first_chunk = true;
                while let Some(partial) = partial_rx.recv().await {
                    // Always emit the partial transcript event for overlay display.
                    let payload = serde_json::json!({ "text": partial.text });
                    if let Err(e) = app.emit(crate::events::EVENT_STT_PARTIAL_TRANSCRIPT, payload) {
                        log::warn!("Failed to emit partial transcript event: {}", e);
                    }

                    // Live output: paste committed chunks immediately.
                    if stt_live_output {
                        if let Some(committed) = partial.committed_text {
                            live_output_flag.store(true, std::sync::atomic::Ordering::SeqCst);

                            // Add a leading space between chunks (not for the first one).
                            let output_text = if is_first_chunk {
                                is_first_chunk = false;
                                committed
                            } else {
                                format!(" {}", committed)
                            };

                            // Paste on a blocking thread to avoid blocking the async runtime.
                            // Never preserve clipboard during live output — the
                            // save/restore cycle adds ~100-200ms of dead time
                            // between every chunk, causing noticeable pauses.
                            let text = output_text.clone();
                            let mode = output_mode;
                            let enter = output_hit_enter;
                            tokio::task::spawn_blocking(move || {
                                if let Err(e) = crate::text::inject::output_text_with_mode_options(
                                    &text, mode, enter, false,
                                ) {
                                    log::error!("Live output: failed to paste chunk: {}", e);
                                }
                            })
                            .await
                            .ok();
                        }
                    }
                }
                log::debug!("Partial transcript consumer: channel closed");
            });
        }
    }

    /// Stop recording and return the raw WAV audio
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn stop_recording(&self) -> Result<Vec<u8>, PipelineError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| PipelineError::Lock(e.to_string()))?;

        if !recording::can_stop_recording(inner.state) {
            return Err(PipelineError::NotRecording);
        }

        let encode_cfg = inner.config.audio_encode_config();
        match recording::stop_recording_session(inner.audio_capture.as_mut(), encode_cfg) {
            Ok(outcome) => {
                inner.last_recording_diagnostics = Some(outcome.diagnostics);

                // Check size limit
                let max_bytes = inner.config.max_recording_bytes;
                if let Err(e) =
                    recording::validate_recording_size(outcome.wav_bytes.len(), max_bytes)
                {
                    inner.set_error(&format!(
                        "Recording too large: {} bytes",
                        outcome.wav_bytes.len()
                    ));
                    return Err(e);
                }

                // Keep a copy for STT testing/debugging UI.
                inner.last_wav_bytes = Some(outcome.wav_bytes.clone());

                inner.reset_to_idle();
                log::info!(
                    "Pipeline: Recording stopped, {} bytes captured",
                    outcome.wav_bytes.len()
                );
                Ok(outcome.wav_bytes)
            }
            Err(e) => {
                inner.set_error(&format!("Failed to stop recording: {}", e));
                Err(e)
            }
        }
    }

    /// Stop recording and return a before/after pair of WAV bytes.
    ///
    /// - before: raw capture with no preprocessing/gates
    /// - after: capture encoded with the current audio settings
    ///
    /// Intended for settings UI A/B testing.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn stop_recording_before_after(&self) -> Result<(Vec<u8>, Vec<u8>), PipelineError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| PipelineError::Lock(e.to_string()))?;

        if !recording::can_stop_recording(inner.state) {
            return Err(PipelineError::NotRecording);
        }

        let encode_cfg = inner.config.audio_encode_config();
        match recording::stop_recording_before_after(inner.audio_capture.as_mut(), encode_cfg) {
            Ok(outcome) => {
                inner.last_recording_diagnostics = Some(outcome.diagnostics);

                // Check size limit (both, to avoid surprising huge payloads)
                let max_bytes = inner.config.max_recording_bytes;
                if let Err(e) =
                    recording::validate_recording_size(outcome.before_wav.len(), max_bytes)
                {
                    inner.set_error(&format!(
                        "Recording too large: {} bytes",
                        outcome.before_wav.len()
                    ));
                    return Err(e);
                }
                if let Err(e) =
                    recording::validate_recording_size(outcome.after_wav.len(), max_bytes)
                {
                    inner.set_error(&format!(
                        "Recording too large: {} bytes",
                        outcome.after_wav.len()
                    ));
                    return Err(e);
                }

                // Keep a copy of the processed output for STT test + debugging.
                inner.last_wav_bytes = Some(outcome.after_wav.clone());

                inner.reset_to_idle();
                Ok((outcome.before_wav, outcome.after_wav))
            }
            Err(e) => {
                inner.set_error(&format!("Failed to stop recording: {}", e));
                Err(e)
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

            let wav_bytes = inner.last_wav_bytes.clone().ok_or_else(|| {
                PipelineError::Config(
                    "No audio captured yet. Record once to create test audio.".to_string(),
                )
            })?;

            let config = inner.config.clone();

            // Resolve per-profile overrides. Note: program prompt profiles live under llm_config.
            let profile = profile_id
                .and_then(|id| if id == "default" { None } else { Some(id) })
                .and_then(|id| {
                    config
                        .llm_config
                        .program_prompt_profiles
                        .iter()
                        .find(|p| p.id == id)
                        .cloned()
                });

            let resolved_stt = stt_provider_resolver::resolve_stt_provider_for_transcription(
                &mut inner,
                SttProviderResolutionRequest {
                    active_profile: profile.as_ref(),
                    active_preset: None,
                    forced_provider: None,
                    forced_model: None,
                },
            )?;

            let cancel_token = inner
                .cancel_token
                .clone()
                .unwrap_or_else(CancellationToken::new);

            (
                wav_bytes,
                resolved_stt.provider,
                config.retry_config.clone(),
                cancel_token,
            )
        };

        // Test endpoint intentionally does NOT enforce timeout
        let result = run_stt_transcription(
            stt_provider,
            &wav_bytes,
            &retry_config,
            None, // no timeout for test endpoint
            &cancel_token,
            "Pipeline (test)",
        )
        .await?;

        Ok(result.text)
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
    pub async fn stop_and_transcribe_detailed(&self) -> Result<TranscriptionResult, PipelineError> {
        // Profile override is per recording session; take + clear it now so it doesn't
        // leak into the next request.
        let session_profile_override = self.take_session_profile_override();

        // Phase 1: Stop recording and prepare for transcription (synchronous, holds lock briefly)
        let (
            wav_bytes,
            stt_provider,
            stt_provider_id,
            stt_model,
            stt_language,
            retry_config,
            timeout,
            cancel_token,
            active_profile,
            default_rewrite_include_clipboard_context,
            ocr_config,
            ocr_modes,
            mut streaming_session,
        ) = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|e| PipelineError::Lock(e.to_string()))?;

            if !recording::can_stop_recording(inner.state) {
                return Err(PipelineError::NotRecording);
            }

            let encode_cfg = inner.config.audio_encode_config();
            let outcome =
                match recording::stop_recording_session(inner.audio_capture.as_mut(), encode_cfg) {
                    Ok(out) => out,
                    Err(e) => {
                        // Clean up streaming on error path.
                        inner.active_streaming_session.take();
                        inner.audio_capture.set_live_audio_tx(None);
                        inner.set_error(&format!("Failed to stop recording: {}", e));
                        return Err(e);
                    }
                };

            // Worker thread has flushed all remaining audio chunks. Now clear the
            // live audio sender so it won't leak into the next session.
            inner.audio_capture.set_live_audio_tx(None);

            // Persist diagnostics for UI readout.
            inner.last_recording_diagnostics = Some(outcome.diagnostics);

            // Check size limit before cloning/storing debug audio.
            let max_bytes = inner.config.max_recording_bytes;
            if let Err(e) = recording::validate_recording_size(outcome.wav_bytes.len(), max_bytes) {
                inner.set_error(&format!(
                    "Recording too large: {} bytes",
                    outcome.wav_bytes.len()
                ));
                return Err(e);
            }

            // Keep a copy for STT testing/debugging UI.
            inner.last_wav_bytes = Some(outcome.wav_bytes.clone());

            // Evaluate quiet audio gate (VAD-based speech detection + amplitude thresholds).
            let gate_config = recording::QuietAudioGateConfig {
                enabled: inner.config.quiet_audio_gate_enabled,
                require_speech: inner.config.quiet_audio_require_speech,
                min_duration_secs: inner.config.quiet_audio_min_duration_secs,
                rms_dbfs_threshold: inner.config.quiet_audio_rms_dbfs_threshold,
                peak_dbfs_threshold: inner.config.quiet_audio_peak_dbfs_threshold,
            };
            match recording::evaluate_quiet_audio_gate(&outcome.diagnostics, gate_config) {
                recording::QuietAudioGateResult::NoSpeechDetected => {
                    inner.reset_to_idle();
                    return Ok(TranscriptionResult {
                        speaker_segments: Vec::new(),
                        stt_text: String::new(),
                        final_text: String::new(),
                        stt_duration_ms: 0,
                        stt_retry: None,
                        llm_duration_ms: None,
                        llm_provider_used: None,
                        llm_model_used: None,
                        llm_outcome: LlmOutcome::NotAttempted(
                            LlmNotAttemptedReason::NoSpeechDetectedByVad,
                        ),
                        live_output_completed: false,
                    });
                }
                recording::QuietAudioGateResult::Quiet => {
                    inner.reset_to_idle();
                    return Ok(TranscriptionResult {
                        speaker_segments: Vec::new(),
                        stt_text: String::new(),
                        final_text: String::new(),
                        stt_duration_ms: 0,
                        stt_retry: None,
                        llm_duration_ms: None,
                        llm_provider_used: None,
                        llm_model_used: None,
                        llm_outcome: LlmOutcome::NotAttempted(
                            LlmNotAttemptedReason::QuietAudioGate,
                        ),
                        live_output_completed: false,
                    });
                }
                recording::QuietAudioGateResult::NotQuiet => {}
            }

            inner.transition_to(PipelineState::Transcribing, "stop_and_transcribe_detailed");

            let llm_config = inner.config.llm_config.clone();
            let request_profile_context = resolve_request_profile_context(
                &llm_config,
                session_profile_override.as_deref(),
                select_profile_for_foreground_app(&llm_config),
                ActiveWindowOcrModeFallbacks {
                    rewrite: inner.config.ocr_config.rewrite_mode.as_str(),
                    quick_ask: inner.config.ocr_config.quick_ask_mode.as_str(),
                    quick_replace: inner.config.ocr_config.quick_replace_mode.as_str(),
                },
                DefaultProfileSelectionPolicy::UseDefaultAsActiveFallback,
            );
            let active_profile = request_profile_context.active_profile().cloned();
            let default_profile = request_profile_context.default_profile().cloned();

            let default_rewrite_include_clipboard_context = default_profile
                .as_ref()
                .and_then(|p| p.rewrite_include_clipboard_context)
                .unwrap_or(false);

            let active_preset = request_profile_context.effective_preset().cloned();

            // Persist the *actual* profile used for this request into the request log.
            // Note: picking the profile at transcription time tends to be more accurate than
            // at recording start (e.g. overlay window can steal focus).
            if let Some(store) = inner.config.request_log_store.as_ref() {
                store.with_current(|log| {
                    log.profile_id = request_profile_context
                        .request_log_profile_id()
                        .map(str::to_string);
                    log.profile_name = request_profile_context
                        .request_log_profile_name()
                        .map(str::to_string);
                });
            }
            // Resolve effective STT settings (profile overrides -> global defaults, with safe fallback)
            let resolved_stt = stt_provider_resolver::resolve_stt_provider_for_transcription(
                &mut inner,
                SttProviderResolutionRequest {
                    active_profile: active_profile.as_ref(),
                    active_preset: active_preset.as_ref(),
                    forced_provider: None,
                    forced_model: None,
                },
            )?;

            let retry_config = inner.config.retry_config.clone();
            let cancel_token = inner
                .cancel_token
                .clone()
                .unwrap_or_else(CancellationToken::new);

            let ocr_modes = request_profile_context.ocr_modes().clone();

            // Take the streaming session (if one was started during recording).
            // live_audio_tx was already cleared right after stop_recording_session.
            let streaming_session = inner.active_streaming_session.take();
            log::debug!(
                "Pipeline: streaming session taken={}, provider={}",
                streaming_session.is_some(),
                resolved_stt.provider_id
            );

            (
                outcome.wav_bytes,
                resolved_stt.provider,
                resolved_stt.provider_id,
                resolved_stt.model,
                resolved_stt.language,
                retry_config,
                resolved_stt.timeout,
                cancel_token,
                active_profile,
                default_rewrite_include_clipboard_context,
                inner.config.ocr_config.clone(),
                ocr_modes,
                streaming_session,
            )
        };

        self.start_ocr_task_if_auto(&ocr_config, ocr_modes.should_auto_start(false));

        // If the current model is realtime-only but the streaming session hasn't
        // been stored yet (WebSocket still connecting in the spawned task), wait
        // for it rather than immediately failing.
        if streaming_session.is_none() && stt_provider.requires_streaming() {
            log::info!(
                "Pipeline: Realtime model with no streaming session yet — waiting for connection"
            );
            const MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
            const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);
            let wait_start = std::time::Instant::now();
            while wait_start.elapsed() < MAX_WAIT {
                tokio::time::sleep(POLL_INTERVAL).await;
                if let Ok(mut inner) = self.inner.lock() {
                    if inner.active_streaming_session.is_some() {
                        streaming_session = inner.active_streaming_session.take();
                        log::info!(
                            "Pipeline: Streaming session became available after {}ms",
                            wait_start.elapsed().as_millis()
                        );
                        break;
                    }
                }
            }
            if streaming_session.is_none() {
                log::warn!(
                    "Pipeline: Timed out after {}ms waiting for streaming connection",
                    wait_start.elapsed().as_millis()
                );
            }
        }

        // Phase 2: Transcribe — use streaming session if available, otherwise batch.
        let stt_start = std::time::Instant::now();
        let (stt_text, stt_duration_ms, stt_retry) = if let Some(session) = streaming_session {
            log::info!(
                "Pipeline: Finalizing concurrent streaming session ({} bytes recorded)",
                wav_bytes.len()
            );
            match session.finalize().await {
                Ok(text) => {
                    let duration_ms = stt_start.elapsed().as_millis() as u64;
                    let normalized = utils::normalize_stt_text(text);
                    log::info!(
                        "Pipeline: Streaming STT finalized, {} chars in {}ms",
                        normalized.len(),
                        duration_ms
                    );
                    if let Ok(mut inner) = self.inner.lock() {
                        inner.stt_complete = true;
                        log::debug!("stt_complete set to true (streaming finalize)");
                    }
                    (normalized, duration_ms, None)
                }
                Err(e) => {
                    // If the provider requires streaming (realtime-only model),
                    // don't fall back to batch — propagate the error directly.
                    if stt_provider.requires_streaming() {
                        log::error!(
                            "Pipeline: Realtime STT failed ({}) — no batch fallback for this model",
                            e
                        );
                        let mut inner = self
                            .inner
                            .lock()
                            .map_err(|err| PipelineError::Lock(err.to_string()))?;
                        inner.set_error(&e.to_string());
                        return Err(PipelineError::Stt(e));
                    }

                    log::warn!(
                        "Pipeline: Streaming STT failed ({}), falling back to batch",
                        e
                    );
                    // Fall back to batch transcription.
                    let result = self
                        .run_batch_stt_request(
                            stt_provider,
                            &stt_provider_id,
                            stt_model.clone(),
                            stt_language.clone(),
                            &wav_bytes,
                            &retry_config,
                            timeout,
                            &cancel_token,
                            "Pipeline (streaming-fallback)",
                            "streaming fallback",
                        )
                        .await?;
                    (result.text, result.duration_ms, Some(result.retry))
                }
            }
        } else {
            // If the provider requires streaming (realtime-only model) but no
            // streaming session was started, we can't fall back to batch.
            if stt_provider.requires_streaming() {
                log::error!(
                    "Pipeline: No streaming session for realtime-only model — cannot batch-transcribe"
                );
                let mut inner = self
                    .inner
                    .lock()
                    .map_err(|err| PipelineError::Lock(err.to_string()))?;
                inner.set_error("Realtime streaming session was not started. Please try again.");
                return Err(PipelineError::Stt(SttError::Config(
                    "Realtime streaming session was not started — no batch fallback available"
                        .into(),
                )));
            }

            log::info!(
                "Pipeline: Starting batch transcription ({} bytes, timeout {:?})",
                wav_bytes.len(),
                timeout
            );

            // Phase 2: Transcribe with retry logic (async, outside the lock)
            let result = self
                .run_batch_stt_request(
                    stt_provider,
                    &stt_provider_id,
                    stt_model.clone(),
                    stt_language.clone(),
                    &wav_bytes,
                    &retry_config,
                    timeout,
                    &cancel_token,
                    "Pipeline",
                    "stop_and_transcribe_detailed",
                )
                .await?;
            (result.text, result.duration_ms, Some(result.retry))
        };

        // Phase 3-4: Routing and LLM rewrite via transcription_flow module
        let (proxy_settings, llm_api_keys, request_log_store, llm_enabled_global, llm_config) = {
            let inner = self
                .inner
                .lock()
                .map_err(|e| PipelineError::Lock(e.to_string()))?;
            (
                inner.config.proxy_settings.clone(),
                inner.config.llm_api_keys.clone(),
                inner.config.request_log_store.clone(),
                inner.config.llm_config.enabled,
                inner.config.llm_config.clone(),
            )
        };

        // OCR modes:
        // - auto: start + wait
        // - manual: user may have triggered OCR via overlay; if so, wait for it here
        // - off: never wait
        let ocr_result = if ocr_modes.should_wait_for_normal_dictation_ocr() {
            self.get_ocr_result_with_timeout(Duration::from_millis(ocr_config.request_timeout_ms))
                .await
        } else {
            None
        };
        let ocr_text = ocr_result.as_ref().map(|r| r.text.clone());

        // Session lock is a one-shot override: take + clear it now so it only
        // applies to this transcription attempt.
        let session_lock = self.take_session_preset_lock();
        let persist_app = self.app_handle.lock().ok().and_then(|g| g.clone());

        // Retrieve injected embeddings provider for testing (None in production)
        let injected_embeddings_provider = {
            let inner = self
                .inner
                .lock()
                .map_err(|e| PipelineError::Lock(e.to_string()))?;
            inner.injected_embeddings_provider.clone()
        };

        let ctx = TranscriptionContext {
            active_profile: active_profile.clone(),
            active_window_ocr_text: ocr_text,
            llm_enabled_global,
            default_rewrite_include_clipboard_context,
            session_lock: session_lock.map(|l| transcription_flow::SessionPresetLock {
                profile_id: l.profile_id,
                preset_id: l.preset_id,
            }),
            proxy_settings,
            llm_api_keys,
            request_log_store,
            embedding_cache: &self.embedding_cache,
            persist_app,
            cancel_token: cancel_token.clone(),
            injected_embeddings_provider,
            force_llm_rewrite: false,
            forced_llm_provider: None,
            forced_llm_model: None,
        };

        let callbacks = PipelineCallbacks {
            inner: self.inner.clone(),
        };

        let result = complete_transcription_flow(
            &ctx,
            &callbacks,
            &stt_text,
            stt_duration_ms,
            stt_retry,
            &llm_config,
        )
        .await;

        // Phase 5: Update state to idle and check live output flag
        let live_output_completed = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|e| PipelineError::Lock(e.to_string()))?;

            let was_live = inner
                .live_output_active
                .load(std::sync::atomic::Ordering::SeqCst);

            // Reset the flag for the next session.
            inner
                .live_output_active
                .store(false, std::sync::atomic::Ordering::SeqCst);

            inner.reset_to_idle();
            log::info!(
                "Pipeline: Complete, {} chars output (live_output={})",
                result.final_text.len(),
                was_live,
            );
            was_live
        };

        Ok(TranscriptionResult {
            live_output_completed,
            ..result
        })
    }

    /// Transcribe provided WAV bytes using the same STT + optional LLM logic as the main pipeline.
    ///
    /// This is used for retrying failed requests from persisted audio.
    #[allow(dead_code)]
    pub async fn transcribe_wav_bytes_detailed(
        &self,
        wav_bytes: Vec<u8>,
    ) -> Result<TranscriptionResult, PipelineError> {
        self.transcribe_wav_bytes_detailed_for_profile(wav_bytes, None)
            .await
    }

    /// Transcribe provided WAV bytes, optionally forcing a specific prompt profile.
    ///
    /// When `profile_id_override` is provided, we attempt to use that per-program profile
    /// (by id) instead of selecting based on the current foreground application.
    pub async fn transcribe_wav_bytes_detailed_for_profile(
        &self,
        wav_bytes: Vec<u8>,
        profile_id_override: Option<&str>,
    ) -> Result<TranscriptionResult, PipelineError> {
        self.transcribe_wav_bytes_detailed_for_profile_with_llm_overrides(
            wav_bytes,
            profile_id_override,
            None,
            None,
            None,
            None,
        )
        .await
    }

    /// Transcribe provided WAV bytes, optionally forcing a specific prompt profile,
    /// and optionally forcing the LLM rewrite provider/model for this one transcription.
    ///
    /// This is intended for CLI benchmarking/diagnostics so we can measure rewrite latency
    /// without changing persisted settings.
    pub async fn transcribe_wav_bytes_detailed_for_profile_with_llm_overrides(
        &self,
        wav_bytes: Vec<u8>,
        profile_id_override: Option<&str>,
        forced_stt_provider: Option<&str>,
        forced_stt_model: Option<&str>,
        forced_llm_provider: Option<&str>,
        forced_llm_model: Option<&str>,
    ) -> Result<TranscriptionResult, PipelineError> {
        self.transcribe_saved_audio(
            wav_bytes,
            profile_id_override,
            forced_stt_provider,
            forced_stt_model,
            forced_llm_provider,
            forced_llm_model,
            SavedAudioMode::Dictation,
        )
        .await
    }

    pub async fn transcribe_journal_wav(
        &self,
        wav: Vec<u8>,
        profile: Option<&str>,
        checkpoint: Option<&std::path::Path>,
        options: &crate::recordings::options::RecordingPreferences,
    ) -> Result<TranscriptionResult, PipelineError> {
        if !self.is_recovering() {
            return Err(PipelineError::Config(
                "Meeting recovery ownership is required".into(),
            ));
        }
        let selection = if options.mode == crate::recordings::options::RecordingMode::Meeting {
            Some(options.meeting_model.as_ref().ok_or_else(|| {
                PipelineError::Config(
                    "Select a meeting transcription model in Recording options".into(),
                )
            })?)
        } else {
            None
        };
        self.transcribe_saved_audio(
            wav,
            profile,
            selection.map(|s| s.provider.as_str()),
            selection.map(|s| s.model.as_str()),
            None,
            None,
            SavedAudioMode::Journal(checkpoint, options),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn transcribe_saved_audio(
        &self,
        wav_bytes: Vec<u8>,
        profile_id_override: Option<&str>,
        forced_stt_provider: Option<&str>,
        forced_stt_model: Option<&str>,
        forced_llm_provider: Option<&str>,
        forced_llm_model: Option<&str>,
        mode: SavedAudioMode<'_>,
    ) -> Result<TranscriptionResult, PipelineError> {
        let is_journal = matches!(mode, SavedAudioMode::Journal(..));
        let is_meeting = matches!(
            mode,
            SavedAudioMode::Journal(_, options) if options.mode == crate::recordings::options::RecordingMode::Meeting
        );
        let meeting_selection = match mode {
            SavedAudioMode::Journal(_, options) if is_meeting => options.meeting_model.as_ref(),
            _ => None,
        };
        // Phase 1: Resolve providers/config under lock.
        let (
            stt_provider,
            stt_provider_id,
            stt_model,
            stt_language,
            retry_config,
            timeout,
            cancel_token,
            active_profile,
            default_rewrite_include_clipboard_context,
            ocr_config,
            ocr_modes,
        ) = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|e| PipelineError::Lock(e.to_string()))?;

            // Guard: don't run a retry while actively recording.
            if inner.state == PipelineState::Recording {
                return Err(PipelineError::AlreadyRecording);
            }
            if matches!(
                inner.state,
                PipelineState::Transcribing | PipelineState::Rewriting
            ) {
                return Err(PipelineError::Lock(
                    "Pipeline already transcribing".to_string(),
                ));
            }

            // Avoid a permanent full-meeting debug copy.
            if !is_journal {
                inner.last_wav_bytes = Some(wav_bytes.clone());
            }

            // Check size limit
            let max_bytes = if is_journal {
                meeting_transcription::MAX_MEETING_WAV_BYTES
            } else {
                inner.config.max_recording_bytes
            };
            if max_bytes > 0 && wav_bytes.len() > max_bytes {
                inner.set_error(&format!("Recording too large: {} bytes", wav_bytes.len()));
                return Err(PipelineError::RecordingTooLarge(wav_bytes.len(), max_bytes));
            }

            inner.transition_to(
                PipelineState::Transcribing,
                "transcribe_wav_bytes_detailed_for_profile",
            );

            // Ensure we have a cancellation token for this attempt.
            let cancel_token = if is_journal {
                inner.recovery_job.clone().ok_or_else(|| {
                    PipelineError::Config("Meeting recovery ownership is required".into())
                })?
            } else {
                CancellationToken::new()
            };
            inner.cancel_token = Some(cancel_token.clone());

            let llm_config = inner.config.llm_config.clone();
            let request_profile_context = resolve_request_profile_context(
                &llm_config,
                profile_id_override,
                select_profile_for_foreground_app(&llm_config),
                ActiveWindowOcrModeFallbacks {
                    rewrite: inner.config.ocr_config.rewrite_mode.as_str(),
                    quick_ask: inner.config.ocr_config.quick_ask_mode.as_str(),
                    quick_replace: inner.config.ocr_config.quick_replace_mode.as_str(),
                },
                DefaultProfileSelectionPolicy::UseDefaultAsActiveFallback,
            );
            let active_profile = request_profile_context.active_profile().cloned();
            let default_profile = request_profile_context.default_profile().cloned();

            let default_rewrite_include_clipboard_context = default_profile
                .as_ref()
                .and_then(|p| p.rewrite_include_clipboard_context)
                .unwrap_or(false);

            let active_preset = request_profile_context.effective_preset().cloned();

            // Persist the profile being used for this retry attempt into the request log, if available.
            if let Some(store) = inner.config.request_log_store.as_ref() {
                store.with_current(|log| {
                    log.profile_id = request_profile_context
                        .request_log_profile_id()
                        .map(str::to_string);
                    log.profile_name = request_profile_context
                        .request_log_profile_name()
                        .map(str::to_string);
                });
            }
            // Resolve effective STT settings (profile overrides -> global defaults, with safe fallback)
            if let Some(selection) = meeting_selection {
                if selection.use_managed
                    && !(inner.config.managed_inference_enabled
                        && managed_gateway_ready(&inner.config)
                        && stt_provider_resolver::managed_stt_model_supported(
                            &selection.provider,
                            Some(&selection.model),
                        ))
                {
                    let message = "Managed meeting transcription is unavailable. Your recording is preserved.";
                    inner.set_error(message);
                    return Err(PipelineError::Config(message.into()));
                }
            }
            // Provider creation and its cache key both see the meeting's route.
            // Restore Dictation's preference before releasing this mutex, even
            // on resolution failure; the created provider owns its own transport.
            let dictation_managed_preference = inner.config.managed_stt_preferred;
            if let Some(selection) = meeting_selection {
                inner.config.managed_stt_preferred = selection.use_managed;
            }
            let resolved_stt = stt_provider_resolver::resolve_stt_provider_for_transcription(
                &mut inner,
                SttProviderResolutionRequest {
                    active_profile: if is_meeting {
                        None
                    } else {
                        active_profile.as_ref()
                    },
                    active_preset: if is_meeting {
                        None
                    } else {
                        active_preset.as_ref()
                    },
                    forced_provider: forced_stt_provider,
                    forced_model: forced_stt_model,
                },
            );
            inner.config.managed_stt_preferred = dictation_managed_preference;
            let resolved_stt = resolved_stt?;

            let retry_config = inner.config.retry_config.clone();

            let ocr_modes = request_profile_context.ocr_modes().clone();

            (
                resolved_stt.provider,
                resolved_stt.provider_id,
                resolved_stt.model,
                resolved_stt.language,
                retry_config,
                resolved_stt.timeout,
                cancel_token,
                active_profile,
                default_rewrite_include_clipboard_context,
                inner.config.ocr_config.clone(),
                ocr_modes,
            )
        };

        if !is_meeting {
            self.start_ocr_task_if_auto(&ocr_config, ocr_modes.should_auto_start(false));
        }

        log::info!(
            "Pipeline: Starting retry transcription ({} bytes, timeout {:?})",
            wav_bytes.len(),
            timeout
        );

        // Phase 2: STT transcription
        let result = if let SavedAudioMode::Journal(checkpoint, _) = mode {
            let settings_key = {
                let inner = self
                    .inner
                    .lock()
                    .map_err(|e| PipelineError::Lock(e.to_string()))?;
                let key = format!(
                    "{:?}",
                    (
                        &stt_provider_id,
                        &stt_model,
                        &stt_language,
                        &inner.config.stt_transcription_prompt
                    )
                );
                if let Some(selection) = meeting_selection {
                    format!("{key}:managed={}", selection.use_managed)
                } else {
                    key
                }
            };
            let result = meeting_transcription::transcribe(
                &wav_bytes,
                checkpoint,
                &settings_key,
                &cancel_token,
                |chunk| {
                    let provider = stt_provider.clone();
                    let model = stt_model.clone();
                    let language = stt_language.clone();
                    let token = &cancel_token;
                    let retry = &retry_config;
                    let provider_id = &stt_provider_id;
                    async move {
                        self.run_batch_stt_request(
                            provider,
                            provider_id,
                            model,
                            language,
                            &chunk,
                            retry,
                            timeout,
                            token,
                            "Meeting",
                            "meeting_upload",
                        )
                        .await
                    }
                },
            )
            .await;
            match result {
                Ok(result) => {
                    self.mark_stt_complete("meeting_complete");
                    result
                }
                Err(error) => {
                    self.finish_failed_stt_attempt(&error)?;
                    return Err(error);
                }
            }
        } else {
            self.run_batch_stt_request(
                stt_provider,
                &stt_provider_id,
                stt_model.clone(),
                stt_language.clone(),
                &wav_bytes,
                &retry_config,
                timeout,
                &cancel_token,
                "Pipeline (retry)",
                "retry_transcription",
            )
            .await?
        };
        let speaker_segments = result.segments;
        let (stt_text, stt_duration_ms, stt_retry) =
            (result.text, result.duration_ms, Some(result.retry));

        // Meeting is a separate product mode, not an inherited rewrite toggle.
        // Bypass routing, OCR, clipboard context, and every rewrite override.
        if is_meeting {
            if cancel_token.is_cancelled() {
                self.finish_failed_stt_attempt(&PipelineError::Cancelled)?;
                return Err(PipelineError::Cancelled);
            }
            let final_text = crate::stt::speaker_document(&stt_text, &speaker_segments);
            self.inner
                .lock()
                .map_err(|e| PipelineError::Lock(e.to_string()))?
                .reset_to_idle();
            return Ok(TranscriptionResult {
                speaker_segments,
                stt_text,
                final_text,
                stt_duration_ms,
                stt_retry,
                llm_duration_ms: None,
                llm_provider_used: None,
                llm_model_used: None,
                llm_outcome: LlmOutcome::NotAttempted(LlmNotAttemptedReason::MeetingMode),
                live_output_completed: false,
            });
        }

        // Phase 3-4: Routing and LLM rewrite via transcription_flow module
        let (proxy_settings, llm_api_keys, request_log_store, llm_enabled_global, llm_config) = {
            let inner = self
                .inner
                .lock()
                .map_err(|e| PipelineError::Lock(e.to_string()))?;
            (
                inner.config.proxy_settings.clone(),
                inner.config.llm_api_keys.clone(),
                inner.config.request_log_store.clone(),
                inner.config.llm_config.enabled,
                inner.config.llm_config.clone(),
            )
        };

        let ocr_result = if ocr_modes.should_wait_for_normal_dictation_ocr() {
            self.get_ocr_result_with_timeout(Duration::from_millis(ocr_config.request_timeout_ms))
                .await
        } else {
            None
        };
        let ocr_text = ocr_result.as_ref().map(|r| r.text.clone());

        let session_lock = self.take_session_preset_lock();
        let persist_app = self.app_handle.lock().ok().and_then(|g| g.clone());

        // Retrieve injected embeddings provider for testing (None in production)
        let injected_embeddings_provider = {
            let inner = self
                .inner
                .lock()
                .map_err(|e| PipelineError::Lock(e.to_string()))?;
            inner.injected_embeddings_provider.clone()
        };

        let ctx = TranscriptionContext {
            active_profile: active_profile.clone(),
            active_window_ocr_text: ocr_text,
            llm_enabled_global: llm_enabled_global
                || forced_llm_provider.is_some()
                || forced_llm_model.is_some(),
            default_rewrite_include_clipboard_context,
            session_lock: session_lock.map(|l| transcription_flow::SessionPresetLock {
                profile_id: l.profile_id,
                preset_id: l.preset_id,
            }),
            proxy_settings,
            llm_api_keys,
            request_log_store,
            embedding_cache: &self.embedding_cache,
            persist_app,
            cancel_token: cancel_token.clone(),
            injected_embeddings_provider,
            force_llm_rewrite: forced_llm_provider.is_some() || forced_llm_model.is_some(),
            forced_llm_provider: forced_llm_provider.map(|s| s.to_string()),
            forced_llm_model: forced_llm_model.map(|s| s.to_string()),
        };

        let callbacks = PipelineCallbacks {
            inner: self.inner.clone(),
        };

        let result = complete_transcription_flow(
            &ctx,
            &callbacks,
            &stt_text,
            stt_duration_ms,
            stt_retry,
            &llm_config,
        )
        .await;

        if is_journal && cancel_token.is_cancelled() {
            self.finish_failed_stt_attempt(&PipelineError::Cancelled)?;
            return Err(PipelineError::Cancelled);
        }

        // Phase 5: Update state to idle
        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|e| PipelineError::Lock(e.to_string()))?;
            inner.reset_to_idle();
            log::info!(
                "Pipeline: Retry complete, {} chars output",
                result.final_text.len()
            );
        }

        Ok(result)
    }

    /// Stop recording and transcribe the audio.
    ///
    /// Kept for backwards compatibility. Prefer `stop_and_transcribe_detailed`.
    #[allow(dead_code)]
    pub async fn stop_and_transcribe(&self) -> Result<String, PipelineError> {
        self.stop_and_transcribe_detailed()
            .await
            .map(|r| r.final_text)
    }

    /// Update configuration
    ///
    /// Note: This will not affect an in-progress recording.
    pub fn update_config(&self, config: PipelineConfig) -> Result<(), PipelineError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| PipelineError::Lock(e.to_string()))?;

        let ocr_enabled = |ocr: &OcrConfig| {
            let base_url_ok = ocr
                .base_url
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !base_url_ok {
                return false;
            }
            ocr.rewrite_mode != "off"
                || ocr.quick_replace_mode != "off"
                || ocr.quick_ask_mode != "off"
        };

        let old_ocr_enabled = ocr_enabled(&inner.config.ocr_config);
        let new_ocr_enabled = ocr_enabled(&config.ocr_config);

        // If the local-whisper model path changed, evict cached models.
        // Otherwise switching models could keep multiple large GGML files resident.
        let old_local_whisper_key = inner.local_whisper_model_key_for_cache();
        let old_stt_prompt = inner.config.stt_transcription_prompt.clone();

        // Don't update config while recording - could cause issues
        if inner.state == PipelineState::Recording {
            log::warn!("Pipeline: Config update requested while recording, will take effect after current session");
        }

        inner.config = config.clone();

        // If OCR was enabled and is now disabled (URL cleared or modes all off), cancel any
        // in-flight OCR work for the current session.
        if old_ocr_enabled && !new_ocr_enabled {
            inner.cancel_ocr_task(true);
        }

        let new_local_whisper_key = inner.local_whisper_model_key_for_cache();
        // If the Local Whisper model identity or transcription prompt changed, evict cached
        // models so stale local providers do not keep large GGML files resident or reuse old
        // prompt state. We unload only (no auto-load) to respect the user's load mode.
        if local_provider::should_evict_local_whisper_cache(
            &old_local_whisper_key,
            &new_local_whisper_key,
            old_stt_prompt.as_deref(),
            inner.config.stt_transcription_prompt.as_deref(),
        ) {
            inner.unload_local_whisper();
        }

        inner.stt_registry = SttRegistry::new();
        inner.initialize_providers(&config);
        // Update VAD config on audio capture
        inner.audio_capture.set_vad_config(config.vad_config);

        let enabled_profile_ids: HashSet<String> = inner
            .config
            .llm_config
            .program_prompt_profiles
            .iter()
            .map(|p| p.id.clone())
            .collect();

        // Apply capture behavior (Hot Mic + auto-recovery).
        // Safe to call while recording: it won't stop the stream mid-session.
        inner
            .audio_capture
            .set_capture_behavior(
                config.hot_mic_enabled,
                config.hot_mic_pre_roll_ms,
                config.mic_auto_recover_enabled,
                config.input_device_name.as_deref(),
            )
            .map_err(PipelineError::AudioCapture)?;

        // If the active override/lock refers to a disabled (filtered) profile, clear it.
        if let Ok(mut override_guard) = self.session_profile_override.lock() {
            if override_guard
                .as_deref()
                .map(|id| !enabled_profile_ids.contains(id))
                .unwrap_or(false)
            {
                *override_guard = None;
            }
        }

        if let Ok(mut lock_guard) = self.session_preset_lock.lock() {
            if lock_guard
                .as_ref()
                .and_then(|lock| lock.profile_id.as_deref())
                .map(|id| !enabled_profile_ids.contains(id))
                .unwrap_or(false)
            {
                *lock_guard = None;
            }
        }
        log::info!("Pipeline configuration updated");
        Ok(())
    }

    /// Temporarily override the audio capture behavior without updating the full PipelineConfig.
    ///
    /// This is intended for short-lived UI utilities (e.g. Settings mic level test) that
    /// need a CPAL stream running to drive realtime meters.
    pub fn set_capture_behavior_override(
        &self,
        hot_mic_enabled: bool,
        hot_mic_pre_roll_ms: u32,
        mic_auto_recover_enabled: bool,
        input_device_name: Option<&str>,
    ) -> Result<(), PipelineError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| PipelineError::Lock(e.to_string()))?;

        // Never stop/retarget the stream mid-recording.
        if inner.state == PipelineState::Recording {
            return Err(PipelineError::AlreadyRecording);
        }

        inner
            .audio_capture
            .set_capture_behavior(
                hot_mic_enabled,
                hot_mic_pre_roll_ms,
                mic_auto_recover_enabled,
                input_device_name,
            )
            .map_err(PipelineError::AudioCapture)?;

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
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.last_wav_bytes.clone())
    }

    /// Get a copy of the last recording diagnostics (raw stats + optional speech detection).
    pub fn last_recording_diagnostics(&self) -> Option<AudioCaptureDiagnostics> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.last_recording_diagnostics)
    }

    /// Poll for VAD events (non-blocking)
    ///
    /// Returns the next VAD event if one is available, or None if no events are pending.
    #[allow(dead_code)]
    pub fn poll_vad_event(&self) -> Option<AudioCaptureEvent> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.audio_capture.poll_vad_event())
    }

    /// Check if VAD auto-stop is enabled
    #[allow(dead_code)]
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
            if let Some(job) = &inner.recovery_job {
                job.cancel();
            }
            if !inner.state.can_cancel() {
                log::debug!(
                    "Pipeline: Cancel requested but nothing to cancel (state: {:?})",
                    inner.state
                );
                return;
            }

            // Signal cancellation to any async tasks
            if let Some(token) = inner.cancel_token.take() {
                token.cancel();
            }

            // Stop audio capture if recording
            if inner.state == PipelineState::Recording {
                inner.audio_capture.stop_recording();
                if let Err(error) = inner.audio_capture.discard_recovery() {
                    inner.set_error(&error.to_string());
                    return;
                }
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
            if let Some(job) = &inner.recovery_job {
                job.cancel();
            }
            // Cancel any async tasks
            if let Some(token) = inner.cancel_token.take() {
                token.cancel();
            }

            inner.cancel_ocr_task(true);
            inner.ocr.session_id = None;

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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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

    pub fn is_local_whisper_loaded(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.is_local_whisper_loaded())
            .unwrap_or(false)
    }

    pub fn unload_local_whisper(&self) -> Result<(), PipelineError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| PipelineError::Lock(e.to_string()))?;

        if inner.state == PipelineState::Recording {
            return Err(PipelineError::AlreadyRecording);
        }

        inner.unload_local_whisper();
        Ok(())
    }

    pub fn force_load_local_whisper(&self) -> Result<(), PipelineError> {
        #[cfg(feature = "local-whisper")]
        {
            // Phase 1: fast path + capture config while holding the lock briefly.
            let (cache_key, model_path, transcription_prompt, stt_language) = {
                let inner = self
                    .inner
                    .lock()
                    .map_err(|e| PipelineError::Lock(e.to_string()))?;

                if inner.state == PipelineState::Recording {
                    return Err(PipelineError::AlreadyRecording);
                }

                let cache_key = inner.local_whisper_cache_key();
                if local_provider::local_whisper_cache_contains(
                    &inner.stt_provider_cache,
                    &cache_key,
                ) {
                    return Ok(());
                }

                let Some(model_path) = inner.config.whisper_model_path.clone() else {
                    return Err(PipelineError::Config(
                        "Local Whisper: no model path configured".to_string(),
                    ));
                };

                (
                    cache_key,
                    model_path,
                    inner.config.stt_transcription_prompt.clone(),
                    inner.config.stt_language.clone(),
                )
            };

            // Phase 2: load the model outside the lock (this can take seconds).
            let provider =
                crate::stt::LocalWhisperProvider::with_config(crate::stt::LocalWhisperConfig {
                    model_path,
                    transcription_prompt,
                    language: stt_language,
                    ..Default::default()
                })
                .map_err(|e| PipelineError::Config(format!("Local Whisper init failed: {}", e)))?;
            let provider = Arc::new(provider);

            // Phase 3: insert into cache under lock.
            let mut inner = self
                .inner
                .lock()
                .map_err(|e| PipelineError::Lock(e.to_string()))?;

            // If recording started while we were loading, don't mutate pipeline state/caches.
            if inner.state == PipelineState::Recording {
                return Err(PipelineError::AlreadyRecording);
            }

            local_provider::insert_loaded_local_whisper_if_absent(
                &mut inner.stt_provider_cache,
                cache_key,
                provider,
            );

            Ok(())
        }

        #[cfg(not(feature = "local-whisper"))]
        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|e| PipelineError::Lock(e.to_string()))?;

            if inner.state == PipelineState::Recording {
                return Err(PipelineError::AlreadyRecording);
            }

            inner.force_load_local_whisper()
        }
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
    #[allow(dead_code)]
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

// Ensure SharedPipeline is Send + Sync for Tauri state
unsafe impl Send for SharedPipeline {}
unsafe impl Sync for SharedPipeline {}

// Tests have been moved to `pipeline/tests.rs` for better organization.
