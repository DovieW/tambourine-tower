//! Pipeline integration tests.
//!
//! These tests exercise the recording pipeline without requiring real hardware (CPAL audio
//! devices) or network (STT/LLM API calls) by injecting fake/mock implementations.

use super::ocr_session_state::OcrTaskHandle;
use super::*;
use crate::audio_capture::{
    AudioCaptureBackend, AudioCaptureDiagnostics, AudioCaptureError, AudioCaptureEvent,
    AudioEncodeConfig, AudioLevelSnapshot, AudioLevelStats, SharedAudioLevelMeter,
    SharedAudioWaveformMeter,
};
use crate::stt::AudioFormat;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

const MOCK_PROVIDER: &str = "mock";
const MOCK_API_KEY: &str = "test-key";

/// A fake audio capture backend that returns canned WAV data without using CPAL.
pub(super) struct FakeAudioCapture {
    level_meter: SharedAudioLevelMeter,
    waveform_meter: SharedAudioWaveformMeter,
    vad_enabled: bool,
    vad_auto_stop: bool,
    wav: Vec<u8>,
    before_wav: Vec<u8>,
    after_wav: Vec<u8>,
    _queued_events: std::collections::VecDeque<AudioCaptureEvent>,
}

impl FakeAudioCapture {
    pub fn new() -> Self {
        Self {
            level_meter: SharedAudioLevelMeter::new_for_tests(),
            waveform_meter: SharedAudioWaveformMeter::new_for_tests(),
            vad_enabled: false,
            vad_auto_stop: false,
            wav: vec![1, 2, 3],
            before_wav: vec![9],
            after_wav: vec![8],
            _queued_events: std::collections::VecDeque::new(),
        }
    }

    fn diagnostics() -> AudioCaptureDiagnostics {
        AudioCaptureDiagnostics {
            stats: AudioLevelStats {
                duration_secs: 0.5,
                rms: 0.1,
                peak: 0.2,
            },
            speech_detected: None,
        }
    }
}

/// A fake audio capture backend that fails immediately when recording starts.
///
/// This gives us a deterministic regression seam for "no input device" style failures
/// without requiring real hardware setup on the test machine.
struct FailingStartAudioCapture {
    level_meter: SharedAudioLevelMeter,
    waveform_meter: SharedAudioWaveformMeter,
}

impl FailingStartAudioCapture {
    fn new() -> Self {
        Self {
            level_meter: SharedAudioLevelMeter::new_for_tests(),
            waveform_meter: SharedAudioWaveformMeter::new_for_tests(),
        }
    }
}

impl AudioCaptureBackend for FailingStartAudioCapture {
    fn shared_level_meter(&self) -> SharedAudioLevelMeter {
        self.level_meter.clone()
    }

    fn shared_waveform_meter(&self) -> SharedAudioWaveformMeter {
        self.waveform_meter.clone()
    }

    fn level_snapshot(&self) -> AudioLevelSnapshot {
        self.level_meter.snapshot()
    }

    fn set_vad_config(&mut self, _config: crate::audio_capture::VadAutoStopConfig) {}

    fn set_capture_behavior(
        &mut self,
        _hot_mic_enabled: bool,
        _hot_mic_pre_roll_ms: u32,
        _mic_auto_recover_enabled: bool,
        _input_device_name: Option<&str>,
    ) -> Result<(), AudioCaptureError> {
        Ok(())
    }

    fn start_recording_session(
        &mut self,
        _max_duration_secs: f32,
        _input_device_name: Option<&str>,
    ) -> Result<(), AudioCaptureError> {
        Err(AudioCaptureError::NoInputDevice)
    }

    fn stop_and_get_wav_with_diagnostics(
        &mut self,
        _cfg: AudioEncodeConfig,
    ) -> Result<(Vec<u8>, AudioCaptureDiagnostics), AudioCaptureError> {
        Err(AudioCaptureError::NoInputDevice)
    }

    fn stop_and_get_wav_before_after(
        &mut self,
        _after_cfg: AudioEncodeConfig,
    ) -> Result<(Vec<u8>, Vec<u8>, AudioCaptureDiagnostics), AudioCaptureError> {
        Err(AudioCaptureError::NoInputDevice)
    }

    fn stop_recording(&mut self) {}
    fn stop(&mut self) {}

    fn poll_vad_event(&self) -> Option<AudioCaptureEvent> {
        None
    }

    fn is_vad_auto_stop_enabled(&self) -> bool {
        false
    }

    fn set_live_audio_tx(&mut self, _tx: Option<tokio::sync::mpsc::Sender<Vec<f32>>>) {
        // No-op for tests.
    }

    fn capture_sample_rate(&self) -> u32 {
        16000
    }
}

impl AudioCaptureBackend for FakeAudioCapture {
    fn set_paused(&mut self, _paused: bool) -> Result<(), AudioCaptureError> {
        Ok(())
    }
    fn shared_level_meter(&self) -> SharedAudioLevelMeter {
        self.level_meter.clone()
    }

    fn shared_waveform_meter(&self) -> SharedAudioWaveformMeter {
        self.waveform_meter.clone()
    }

    fn level_snapshot(&self) -> AudioLevelSnapshot {
        self.level_meter.snapshot()
    }

    fn set_vad_config(&mut self, config: crate::audio_capture::VadAutoStopConfig) {
        self.vad_enabled = config.enabled;
        self.vad_auto_stop = config.auto_stop;
    }

    fn set_capture_behavior(
        &mut self,
        _hot_mic_enabled: bool,
        _hot_mic_pre_roll_ms: u32,
        _mic_auto_recover_enabled: bool,
        _input_device_name: Option<&str>,
    ) -> Result<(), AudioCaptureError> {
        Ok(())
    }

    fn start_recording_session(
        &mut self,
        _max_duration_secs: f32,
        _input_device_name: Option<&str>,
    ) -> Result<(), AudioCaptureError> {
        Ok(())
    }

    fn stop_and_get_wav_with_diagnostics(
        &mut self,
        _cfg: AudioEncodeConfig,
    ) -> Result<(Vec<u8>, AudioCaptureDiagnostics), AudioCaptureError> {
        Ok((self.wav.clone(), Self::diagnostics()))
    }

    fn stop_and_get_wav_before_after(
        &mut self,
        _after_cfg: AudioEncodeConfig,
    ) -> Result<(Vec<u8>, Vec<u8>, AudioCaptureDiagnostics), AudioCaptureError> {
        Ok((
            self.before_wav.clone(),
            self.after_wav.clone(),
            Self::diagnostics(),
        ))
    }

    fn stop_recording(&mut self) {}
    fn stop(&mut self) {}

    fn poll_vad_event(&self) -> Option<AudioCaptureEvent> {
        None
    }

    fn is_vad_auto_stop_enabled(&self) -> bool {
        self.vad_enabled && self.vad_auto_stop
    }

    fn set_live_audio_tx(&mut self, _tx: Option<tokio::sync::mpsc::Sender<Vec<f32>>>) {
        // No-op for tests.
    }

    fn capture_sample_rate(&self) -> u32 {
        16000
    }
}

/// Configurable behavior for mock providers.
#[derive(Clone, Default)]
pub(super) struct MockBehavior {
    /// Artificial delay before returning a response.
    pub delay: Option<std::time::Duration>,
    /// If set, the mock will return this error instead of success.
    pub error: Option<String>,
}

/// A mock STT provider that returns a canned transcript without making network calls.
/// Supports configurable latency and error simulation for testing edge cases.
struct MockSttProvider {
    text: String,
    behavior: MockBehavior,
}

impl MockSttProvider {
    fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            behavior: MockBehavior::default(),
        }
    }

    #[allow(dead_code)]
    fn with_behavior(mut self, behavior: MockBehavior) -> Self {
        self.behavior = behavior;
        self
    }
}

#[async_trait]
impl SttProvider for MockSttProvider {
    async fn transcribe(
        &self,
        _audio: &[u8],
        _format: &AudioFormat,
    ) -> Result<String, crate::stt::SttError> {
        if let Some(delay) = self.behavior.delay {
            tokio::time::sleep(delay).await;
        }
        if let Some(ref err) = self.behavior.error {
            return Err(crate::stt::SttError::Api(err.clone()));
        }
        Ok(self.text.clone())
    }

    fn name(&self) -> &'static str {
        "mock"
    }
}

/// A mock LLM provider that returns a canned completion without making network calls.
/// Supports configurable latency and error simulation for testing edge cases.
struct MockLlmProvider {
    response: String,
    behavior: MockBehavior,
}

impl MockLlmProvider {
    fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            behavior: MockBehavior::default(),
        }
    }

    #[allow(dead_code)]
    fn with_behavior(mut self, behavior: MockBehavior) -> Self {
        self.behavior = behavior;
        self
    }
}

/// A mock LLM provider that returns different outputs depending on whether OCR context
/// is present in the user message.
struct ConditionalOcrLlmProvider {
    with_ocr: String,
    without_ocr: String,
}

impl ConditionalOcrLlmProvider {
    fn new(with_ocr: impl Into<String>, without_ocr: impl Into<String>) -> Self {
        Self {
            with_ocr: with_ocr.into(),
            without_ocr: without_ocr.into(),
        }
    }
}

#[async_trait]
impl crate::llm::LlmProvider for MockLlmProvider {
    async fn complete(
        &self,
        _system_prompt: &str,
        _user_message: &str,
    ) -> Result<String, crate::llm::LlmError> {
        if let Some(delay) = self.behavior.delay {
            tokio::time::sleep(delay).await;
        }
        if let Some(ref err) = self.behavior.error {
            return Err(crate::llm::LlmError::Api(err.clone()));
        }
        Ok(self.response.clone())
    }

    fn name(&self) -> &'static str {
        "mock"
    }

    fn model(&self) -> &str {
        "mock-model"
    }
}

#[async_trait]
impl crate::llm::LlmProvider for ConditionalOcrLlmProvider {
    async fn complete(
        &self,
        _system_prompt: &str,
        user_message: &str,
    ) -> Result<String, crate::llm::LlmError> {
        let has_ocr = user_message.contains("OCR context from the currently active window:");
        Ok(if has_ocr {
            self.with_ocr.clone()
        } else {
            self.without_ocr.clone()
        })
    }

    fn name(&self) -> &'static str {
        "mock"
    }

    fn model(&self) -> &str {
        "mock-model"
    }
}

fn set_state_for_test(
    pipeline: &SharedPipeline,
    state: PipelineState,
    token: Option<CancellationToken>,
) {
    let mut inner = pipeline.inner.lock().expect("pipeline lock");
    inner.state = state;
    inner.cancel_token = token;
}

fn test_config_with_max_recording_bytes() -> PipelineConfig {
    PipelineConfig {
        max_recording_bytes: 1024,
        ..Default::default()
    }
}

fn test_config_for_transcription() -> PipelineConfig {
    let mut config = test_config_with_max_recording_bytes();
    config.stt_provider = MOCK_PROVIDER.to_string();
    config.stt_language = None;
    config.quiet_audio_gate_enabled = false;
    config
}

fn mock_llm_config(
    enabled: bool,
    program_prompt_profiles: Vec<crate::llm::ProgramPromptProfile>,
) -> crate::llm::LlmConfig {
    crate::llm::LlmConfig {
        enabled,
        provider: MOCK_PROVIDER.to_string(),
        api_key: String::new(),
        model: None,
        ollama_url: None,
        managed_gateway_url: None,
        openai_reasoning_effort: None,
        gemini_thinking_budget: None,
        gemini_thinking_level: None,
        anthropic_thinking_budget: None,
        prompts: crate::llm::PromptSections::default(),
        program_prompt_profiles,
        timeout: std::time::Duration::from_secs(30),
    }
}

fn insert_mock_llm_api_key(config: &mut PipelineConfig) {
    config
        .llm_api_keys
        .insert(MOCK_PROVIDER.to_string(), MOCK_API_KEY.to_string());
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

#[test]
fn test_cancel_from_recording_transitions_to_idle() {
    let pipeline = SharedPipeline::new(PipelineConfig::default());
    let token = CancellationToken::new();

    // Given a pipeline in Recording with an active cancel token
    set_state_for_test(&pipeline, PipelineState::Recording, Some(token.clone()));

    // When cancellation is requested
    pipeline.cancel();

    // Then the pipeline resets to Idle and the token is cancelled
    assert_eq!(pipeline.state(), PipelineState::Idle);
    assert!(token.is_cancelled());
    assert!(pipeline.get_cancel_token().is_none());
}

#[test]
fn test_cancel_from_transcribing_transitions_to_idle() {
    let pipeline = SharedPipeline::new(PipelineConfig::default());
    let token = CancellationToken::new();

    // Given a pipeline in Transcribing with an active cancel token
    set_state_for_test(&pipeline, PipelineState::Transcribing, Some(token.clone()));

    // When cancellation is requested
    pipeline.cancel();

    // Then the pipeline resets to Idle and the token is cancelled
    assert_eq!(pipeline.state(), PipelineState::Idle);
    assert!(token.is_cancelled());
}

#[test]
fn test_stop_recording_transitions_to_idle() {
    let pipeline = SharedPipeline::new(PipelineConfig::default());
    let token = CancellationToken::new();

    // Given a pipeline marked as Recording
    set_state_for_test(&pipeline, PipelineState::Recording, Some(token));

    // When stopping the recording
    let result = pipeline.stop_recording();

    // Then it resets to Idle and captures a WAV buffer
    assert!(result.is_ok());
    assert_eq!(pipeline.state(), PipelineState::Idle);
    assert!(pipeline.clone_last_wav_bytes().is_some());
    assert!(pipeline.get_cancel_token().is_none());
}

#[test]
fn ocr_session_survives_reset_to_idle() {
    let config = test_config_with_max_recording_bytes();
    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));

    p.begin_ocr_session("req-1".to_string());

    // Seed a completed OCR result so status reports "done".
    {
        let mut inner = p.inner.lock().unwrap();
        inner.state = PipelineState::Recording;
        inner.ocr.result = Some(crate::ocr::OcrResult {
            text: "hello".to_string(),
            provider: "test".to_string(),
            model: "test".to_string(),
        });
        inner.reset_to_idle();
        assert_eq!(inner.state, PipelineState::Idle);
        assert_eq!(inner.ocr.session_id.as_deref(), Some("req-1"));
        assert!(inner.ocr.result.is_some());
    }

    assert_eq!(p.get_ocr_status(), "done");
    assert_eq!(p.ocr_session_id().as_deref(), Some("req-1"));
}

#[test]
fn recording_output_mode_is_owned_by_the_session() {
    let config = test_config_with_max_recording_bytes();
    let pipeline = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));
    pipeline
        .start_recording_with_output(true, None, false)
        .unwrap();
    pipeline.set_recording_paused(true).unwrap();
    assert!(pipeline.is_recording_paused());
    pipeline.set_recording_paused(false).unwrap();
    assert!(!pipeline.is_recording_paused());
    assert!(pipeline.is_history_only_recording());
    assert!(pipeline.start_recording().is_err());
    assert!(pipeline.is_history_only_recording());
    pipeline.cancel();
    pipeline.start_recording().unwrap();
    assert!(!pipeline.is_history_only_recording());
    assert!(pipeline.set_recording_paused(true).is_err());
}

#[test]
fn recovery_owns_pipeline_between_chunks_and_cancels_without_new_recording() {
    let pipeline = SharedPipeline::new_for_tests(
        test_config_with_max_recording_bytes(),
        Box::new(FakeAudioCapture::new()),
    );
    let token = pipeline.begin_recovery().unwrap();
    assert!(pipeline.is_recovering());
    assert!(pipeline.begin_recovery().is_err());
    assert!(pipeline.start_recording().is_err());
    pipeline.cancel();
    assert!(token.is_cancelled());
    assert!(pipeline.start_recording().is_err());
    pipeline.end_recovery();
    pipeline.start_recording().unwrap();
    assert!(pipeline.begin_recovery().is_err());
    pipeline.cancel();
}

#[test]
fn end_ocr_session_clears_ocr_state() {
    let config = test_config_with_max_recording_bytes();
    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));

    p.begin_ocr_session("req-2".to_string());
    {
        let mut inner = p.inner.lock().unwrap();
        inner.ocr.result = Some(crate::ocr::OcrResult {
            text: "hello".to_string(),
            provider: "test".to_string(),
            model: "test".to_string(),
        });
    }

    assert_eq!(p.get_ocr_status(), "done");

    p.end_ocr_session_if_matches("req-2");
    assert_eq!(p.ocr_session_id(), None);
    assert_eq!(p.get_ocr_status(), "not_started");
}

#[tokio::test]
async fn get_ocr_result_without_task_keeps_status_not_started() {
    let config = test_config_with_max_recording_bytes();
    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));

    p.begin_ocr_session("req-no-task".to_string());

    let result = p
        .get_ocr_result_with_timeout(std::time::Duration::from_millis(1))
        .await;

    assert!(result.is_none());
    assert_eq!(p.ocr_session_id().as_deref(), Some("req-no-task"));
    assert_eq!(p.get_ocr_status(), "not_started");
    assert!(!p.inner.lock().unwrap().ocr.awaiting);
}

#[tokio::test]
async fn awaited_ocr_task_cannot_publish_after_session_superseded() {
    let config = test_config_with_max_recording_bytes();
    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));

    p.begin_ocr_session("req-old".to_string());

    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        let _ = finish_rx.await;
        Ok(crate::ocr::OcrResult {
            text: "stale OCR text".to_string(),
            provider: "test".to_string(),
            model: "test".to_string(),
        })
    });
    let abort_handle = handle.abort_handle();
    {
        let mut inner = p.inner.lock().unwrap();
        inner.ocr.abort_handle = Some(abort_handle);
        inner.ocr.task = Some(OcrTaskHandle::new(
            Some("req-old".to_string()),
            Some("req-old".to_string()),
            handle,
        ));
    }

    let waiter_pipeline = p.clone();
    let waiter = tokio::spawn(async move {
        waiter_pipeline
            .get_ocr_result_with_timeout(std::time::Duration::from_secs(30))
            .await
    });

    for _ in 0..16 {
        if p.inner.lock().unwrap().ocr.awaiting {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(p.inner.lock().unwrap().ocr.awaiting);

    p.begin_ocr_session("req-new".to_string());
    let _ = finish_tx.send(());

    let result = waiter.await.expect("OCR waiter task should not panic");
    assert!(result.is_none());
    assert_eq!(p.ocr_session_id().as_deref(), Some("req-new"));
    assert_eq!(p.get_ocr_status(), "not_started");
}

#[tokio::test]
async fn force_reset_aborts_in_flight_ocr_task() {
    struct NotifyOnDrop(Option<tokio::sync::oneshot::Sender<()>>);

    impl Drop for NotifyOnDrop {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }

    let config = test_config_with_max_recording_bytes();
    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));
    p.begin_ocr_session("req-reset".to_string());

    let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel::<()>();
    let drop_guard = NotifyOnDrop(Some(dropped_tx));
    let handle = tokio::spawn(async move {
        let _drop_guard = drop_guard;
        std::future::pending::<Result<crate::ocr::OcrResult, String>>().await
    });
    let abort_handle = handle.abort_handle();
    {
        let mut inner = p.inner.lock().unwrap();
        inner.ocr.abort_handle = Some(abort_handle);
        inner.ocr.task = Some(OcrTaskHandle::new(
            Some("req-reset".to_string()),
            Some("req-reset".to_string()),
            handle,
        ));
    }

    p.force_reset();

    tokio::time::timeout(std::time::Duration::from_secs(1), dropped_rx)
        .await
        .expect("OCR task should be dropped after abort")
        .expect("drop notification should be sent");
    let inner = p.inner.lock().unwrap();
    assert_eq!(inner.ocr.session_id, None);
    assert!(inner.ocr.task.is_none());
    assert!(inner.ocr.abort_handle.is_none());
    assert!(inner.ocr.result.is_none());
}

#[tokio::test]
async fn force_reset_aborts_awaited_ocr_task() {
    struct NotifyOnDrop(Option<tokio::sync::oneshot::Sender<()>>);

    impl Drop for NotifyOnDrop {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }

    let config = test_config_with_max_recording_bytes();
    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));
    p.begin_ocr_session("req-awaiting-reset".to_string());

    let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel::<()>();
    let drop_guard = NotifyOnDrop(Some(dropped_tx));
    let handle = tokio::spawn(async move {
        let _drop_guard = drop_guard;
        std::future::pending::<Result<crate::ocr::OcrResult, String>>().await
    });
    let abort_handle = handle.abort_handle();
    {
        let mut inner = p.inner.lock().unwrap();
        inner.ocr.abort_handle = Some(abort_handle);
        inner.ocr.task = Some(OcrTaskHandle::new(
            Some("req-awaiting-reset".to_string()),
            Some("req-awaiting-reset".to_string()),
            handle,
        ));
    }

    let waiter_pipeline = p.clone();
    let waiter = tokio::spawn(async move {
        waiter_pipeline
            .get_ocr_result_with_timeout(std::time::Duration::from_secs(30))
            .await
    });

    for _ in 0..16 {
        if p.inner.lock().unwrap().ocr.awaiting {
            break;
        }
        tokio::task::yield_now().await;
    }
    {
        let inner = p.inner.lock().unwrap();
        assert!(inner.ocr.awaiting);
        assert!(inner.ocr.task.is_none());
        assert!(inner.ocr.abort_handle.is_some());
    }

    p.force_reset();

    tokio::time::timeout(std::time::Duration::from_secs(1), dropped_rx)
        .await
        .expect("awaited OCR task should be dropped after abort")
        .expect("drop notification should be sent");
    let result = waiter.await.expect("OCR waiter task should not panic");
    assert!(result.is_none());

    let inner = p.inner.lock().unwrap();
    assert_eq!(inner.ocr.session_id, None);
    assert!(inner.ocr.task.is_none());
    assert!(inner.ocr.abort_handle.is_none());
    assert!(inner.ocr.result.is_none());
    assert!(!inner.ocr.awaiting);
}

#[test]
fn pipeline_can_start_and_stop_without_cpal() {
    let config = test_config_with_max_recording_bytes();

    let fake = FakeAudioCapture::new();
    let meter_handle = fake.shared_level_meter();
    let waveform_handle = fake.shared_waveform_meter();

    let p = SharedPipeline::new_for_tests(config, Box::new(fake));

    assert_eq!(p.try_state(), Some(PipelineState::Idle));
    p.start_recording().expect("start recording should succeed");
    assert_eq!(p.try_state(), Some(PipelineState::Recording));

    // Simulate realtime meter updates without needing an actual CPAL callback.
    let s0 = p.audio_level_snapshot_fast();
    meter_handle.set_for_tests(0.25, 0.5);
    let s1 = p.audio_level_snapshot_fast();
    assert!(s1.seq > s0.seq);
    assert!((s1.rms - 0.25).abs() < 1e-6);
    assert!((s1.peak - 0.5).abs() < 1e-6);

    // Same idea for the waveform meter: simulate samples without needing CPAL.
    let w0 = p.audio_waveform_snapshot_fast();
    waveform_handle.set_from_samples_for_tests(&[0.0, -0.25, 0.5, 1.0], 1);
    let w1 = p.audio_waveform_snapshot_fast();
    assert!(w1.seq > w0.seq);

    let wav = p.stop_recording().expect("stop recording should succeed");
    assert_eq!(wav, vec![1, 2, 3]);
    assert_eq!(p.try_state(), Some(PipelineState::Idle));
}

#[test]
fn start_recording_failure_transitions_pipeline_to_error() {
    let config = test_config_with_max_recording_bytes();
    let pipeline = SharedPipeline::new_for_tests(config, Box::new(FailingStartAudioCapture::new()));

    let result = pipeline.start_recording();

    assert!(matches!(
        result,
        Err(crate::pipeline::PipelineError::AudioCapture(
            AudioCaptureError::NoInputDevice
        ))
    ));
    assert_eq!(pipeline.state(), PipelineState::Error);
}

#[tokio::test]
async fn pipeline_can_transcribe_without_network_or_hardware() {
    // Given: a pipeline with fake audio capture (no CPAL device) and a fake STT provider
    // injected into the provider cache (no network).
    let config = test_config_for_transcription();

    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));
    p.inject_stt_provider_for_tests(
        MOCK_PROVIDER,
        None,
        None,
        Arc::new(MockSttProvider::new("hello from tests")),
    );

    p.start_recording().expect("start recording should succeed");

    // When: we stop and transcribe
    let result = p
        .stop_and_transcribe_detailed()
        .await
        .expect("stop/transcribe should succeed");

    // Then: we get a transcript without hitting any real IO.
    assert_eq!(result.stt_text, "hello from tests");
    assert_eq!(result.final_text, "hello from tests");
    assert_eq!(p.state(), PipelineState::Idle);
}

#[tokio::test]
async fn meeting_transcription_bypasses_only_dictation_size_limit_and_requires_ownership() {
    let mut config = test_config_for_transcription();
    config.max_recording_bytes = 1024;
    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));
    p.inject_stt_provider_for_tests(
        MOCK_PROVIDER,
        None,
        None,
        Arc::new(MockSttProvider::new("one final transcript")),
    );
    let mut audio = crate::audio_capture::AudioBuffer::new(16000, 1, 1.0);
    audio.append(&vec![0.25; 16000]);
    let wav = audio.to_wav_bytes().unwrap();
    assert!(matches!(
        p.transcribe_wav_bytes_detailed(wav.clone()).await,
        Err(PipelineError::RecordingTooLarge(..))
    ));
    assert!(p
        .transcribe_journal_wav(wav.clone(), None, None, &Default::default())
        .await
        .is_err());
    p.begin_recovery().unwrap();
    let result = p
        .transcribe_journal_wav(wav, None, None, &Default::default())
        .await
        .unwrap();
    assert_eq!(result.stt_text, "one final transcript");
    assert_eq!(result.final_text, "one final transcript");
    assert_eq!(p.state(), PipelineState::Idle);
    assert!(p.is_recovering());
    p.end_recovery();
}

#[tokio::test]
async fn meeting_managed_choice_never_falls_back_to_a_user_key_when_access_is_unavailable() {
    use crate::recordings::options::{MeetingModel, RecordingMode, RecordingPreferences};
    let mut config = test_config_for_transcription();
    config.managed_stt_preferred = false;
    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));
    // A direct provider is ready in the cache. Choosing Managed may not use it.
    p.inject_stt_provider_for_tests(
        "openai",
        Some("gpt-4o-transcribe-diarize"),
        None,
        Arc::new(MockSttProvider::new("Must not use a different route")),
    );
    let mut audio = crate::audio_capture::AudioBuffer::new(16000, 1, 1.0);
    audio.append(&vec![0.25; 16000]);
    let options = RecordingPreferences {
        mode: RecordingMode::Meeting,
        meeting_model: Some(MeetingModel {
            provider: "openai".into(),
            model: "gpt-4o-transcribe-diarize".into(),
            use_managed: true,
        }),
    };
    p.begin_recovery().unwrap();
    let error = p
        .transcribe_journal_wav(audio.to_wav_bytes().unwrap(), None, None, &options)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("Managed meeting transcription is unavailable"));
    assert!(!p.config().managed_stt_preferred);
    assert!(p.is_recovering());
    p.end_recovery();
}

#[tokio::test]
async fn meeting_mode_never_rewrites_even_when_dictation_rewriting_is_enabled() {
    use crate::recordings::options::{MeetingModel, RecordingMode, RecordingPreferences};
    let mut config = test_config_for_transcription();
    config.llm_config = mock_llm_config(true, Vec::new());
    insert_mock_llm_api_key(&mut config);
    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));
    p.inject_stt_provider_for_tests(
        MOCK_PROVIDER,
        Some("meeting-model"),
        None,
        Arc::new(MockSttProvider::new("Unchanged meeting")),
    );
    p.inject_llm_provider_for_tests(
        MOCK_PROVIDER,
        None,
        Arc::new(MockLlmProvider::new("Must not run")),
    );
    let mut audio = crate::audio_capture::AudioBuffer::new(16000, 1, 1.0);
    audio.append(&vec![0.25; 16000]);
    let options = RecordingPreferences {
        mode: RecordingMode::Meeting,
        meeting_model: Some(MeetingModel {
            provider: MOCK_PROVIDER.into(),
            model: "meeting-model".into(),
            use_managed: false,
        }),
    };
    p.inner.lock().unwrap().config.managed_stt_preferred = true;
    p.begin_recovery().unwrap();
    let result = p
        .transcribe_journal_wav(audio.to_wav_bytes().unwrap(), None, None, &options)
        .await
        .unwrap();
    assert_eq!(result.final_text, "Unchanged meeting");
    assert!(
        p.config().managed_stt_preferred,
        "Meeting must not change Dictation's route"
    );
    assert!(!result.llm_attempted());
    assert_eq!(
        result.llm_outcome,
        crate::pipeline::LlmOutcome::NotAttempted(
            crate::pipeline::LlmNotAttemptedReason::MeetingMode
        )
    );
    assert!(p.is_recovering());
    p.end_recovery();
}

#[tokio::test]
async fn pipeline_can_transcribe_and_rewrite_without_network_or_hardware() {
    // Given: a pipeline with fake audio capture, fake STT, AND fake LLM providers.
    let mut config = test_config_for_transcription();
    config.llm_config = mock_llm_config(true, Vec::new());
    // Insert fake API key so the pipeline doesn't bail early.
    insert_mock_llm_api_key(&mut config);

    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));
    p.inject_stt_provider_for_tests(
        MOCK_PROVIDER,
        None,
        None,
        Arc::new(MockSttProvider::new("hello from stt")),
    );
    p.inject_llm_provider_for_tests(
        MOCK_PROVIDER,
        None,
        Arc::new(MockLlmProvider::new("Hello from LLM")),
    );

    p.start_recording().expect("start recording should succeed");

    // When: we stop and transcribe (with rewrite enabled)
    let result = p
        .stop_and_transcribe_detailed()
        .await
        .expect("stop/transcribe should succeed");

    // Then: the final output should be the LLM-rewritten text, not the raw STT.
    assert_eq!(result.stt_text, "hello from stt");
    assert_eq!(result.final_text, "Hello from LLM");
    assert!(result.llm_attempted());
    assert_eq!(result.llm_outcome, crate::pipeline::LlmOutcome::Succeeded);
    assert_eq!(p.state(), PipelineState::Idle);
}

#[tokio::test]
async fn rewrite_disabled_falls_back_to_stt_text() {
    // Given: a pipeline with LLM rewrite explicitly disabled.
    let mut config = test_config_for_transcription();
    config.llm_config = mock_llm_config(false, Vec::new());

    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));
    p.inject_stt_provider_for_tests(
        MOCK_PROVIDER,
        None,
        None,
        Arc::new(MockSttProvider::new("raw stt transcript")),
    );
    // NOTE: we do NOT inject an LLM provider — rewrite should not be attempted.

    p.start_recording().expect("start recording should succeed");

    // When: we stop and transcribe
    let result = p
        .stop_and_transcribe_detailed()
        .await
        .expect("stop/transcribe should succeed");

    // Then: final_text should match stt_text (no rewrite).
    assert_eq!(result.stt_text, "raw stt transcript");
    assert_eq!(result.final_text, "raw stt transcript");
    assert!(!result.llm_attempted());
    assert!(matches!(
        result.llm_outcome,
        crate::pipeline::LlmOutcome::NotAttempted(_)
    ));
    assert_eq!(p.state(), PipelineState::Idle);
}

#[tokio::test]
async fn stt_error_propagates_correctly() {
    // Given: a pipeline with an STT provider that will fail.
    let config = test_config_for_transcription();

    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));
    p.inject_stt_provider_for_tests(
        MOCK_PROVIDER,
        None,
        None,
        Arc::new(MockSttProvider::new("unused").with_behavior(MockBehavior {
            error: Some("Simulated API failure".to_string()),
            ..Default::default()
        })),
    );

    p.start_recording().expect("start recording should succeed");

    // When: we stop and transcribe
    let result = p.stop_and_transcribe_detailed().await;

    // Then: we get an error, not a success.
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, crate::pipeline::PipelineError::Stt(_)),
        "Expected STT error, got: {:?}",
        err
    );
    // Pipeline transitions to Error state after a failed transcription.
    assert_eq!(p.state(), PipelineState::Error);
}

// =============================================================================
// Mock Embeddings Provider for deterministic routing tests
// =============================================================================

use crate::embeddings::{EmbeddingsError, EmbeddingsProvider};
use serde_json::Value as JsonValue;
use std::collections::HashMap as StdHashMap;
use std::sync::Mutex;

/// A mock embeddings provider that returns deterministic embeddings based on text content.
///
/// This provider maps specific input texts to predetermined embedding vectors, enabling
/// deterministic offline tests for intent routing without making real API calls.
struct MockEmbeddingsProvider {
    /// Map from input text to embedding vector
    embeddings_map: Mutex<StdHashMap<String, Vec<f32>>>,
    /// Default embedding to return if text is not in map
    default_embedding: Vec<f32>,
    /// Optional error to simulate failures
    error: Option<String>,
}

impl MockEmbeddingsProvider {
    /// Create a new mock provider with a default embedding vector
    fn new(default_embedding: Vec<f32>) -> Self {
        Self {
            embeddings_map: Mutex::new(StdHashMap::new()),
            default_embedding,
            error: None,
        }
    }

    /// Add a text -> embedding mapping
    fn with_embedding(self, text: impl Into<String>, embedding: Vec<f32>) -> Self {
        self.embeddings_map
            .lock()
            .unwrap()
            .insert(text.into(), embedding);
        self
    }

    /// Configure the provider to return an error
    #[allow(dead_code)]
    fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }
}

#[async_trait]
impl EmbeddingsProvider for MockEmbeddingsProvider {
    async fn embed_text(
        &self,
        text: &str,
        _input_type: Option<&str>,
    ) -> Result<(Vec<f32>, JsonValue, JsonValue), EmbeddingsError> {
        if let Some(ref err) = self.error {
            return Err(EmbeddingsError::Api(err.clone()));
        }

        let embedding = self
            .embeddings_map
            .lock()
            .unwrap()
            .get(text)
            .cloned()
            .unwrap_or_else(|| self.default_embedding.clone());

        let request_json = serde_json::json!({
            "mock": true,
            "input": text,
        });
        let response_json = serde_json::json!({
            "mock": true,
            "embedding_len": embedding.len(),
        });

        Ok((embedding, request_json, response_json))
    }

    fn name(&self) -> &'static str {
        "mock"
    }

    fn model(&self) -> &str {
        "mock-embeddings-model"
    }
}

// =============================================================================
// Routing and Preset Selection Invariant Tests
// =============================================================================

use crate::llm::{ProgramPreset, ProgramPromptProfile, PromptSections};
use crate::settings::{IntentRouterSettings, IntentRouterStrategy};

/// Helper to create a profile with presets configured for embeddings routing
fn create_routing_test_profile(presets: Vec<(&str, &str, Vec<&str>)>) -> ProgramPromptProfile {
    let prompt_presets: Vec<ProgramPreset> = presets
        .into_iter()
        .map(|(id, name, hints)| ProgramPreset {
            id: id.to_string(),
            name: name.to_string(),
            routing_hints: hints.into_iter().map(|s| s.to_string()).collect(),
            prompts: PromptSections::default(),
            rewrite_llm_enabled: true,
            stt_provider: None,
            stt_model: None,
            stt_language: None,
            stt_timeout_seconds: None,
            llm_provider: None,
            llm_model: None,
            openai_reasoning_effort: None,
            gemini_thinking_budget: None,
            gemini_thinking_level: None,
            anthropic_thinking_budget: None,
        })
        .collect();

    ProgramPromptProfile {
        id: "test-profile".to_string(),
        name: "Test Profile".to_string(),
        program_paths: vec![],
        rewrite_llm_enabled: Some(true),
        rewrite_include_clipboard_context: None,
        stt_provider: None,
        stt_model: None,
        stt_language: None,
        stt_timeout_seconds: None,
        llm_provider: None,
        llm_model: None,
        openai_reasoning_effort: None,
        gemini_thinking_budget: None,
        gemini_thinking_level: None,
        anthropic_thinking_budget: None,
        prompts: PromptSections::default(),
        presets: prompt_presets,
        default_preset_id: None,
        default_preset_description: None,
        default_target_rewrite_llm_enabled: true,
        active_preset_id: None,
        router: Some(IntentRouterSettings {
            enabled: true,
            strategy: IntentRouterStrategy::Embeddings,
            embedding_provider: Some("mock".to_string()),
            embedding_model: Some("mock-model".to_string()),
            pick_highest_score: true, // Always pick the best match
            similarity_threshold: None,
            similarity_margin: None,
            llm_provider: None,
            llm_model: None,
            llm_system_prompt: None,
            openai_reasoning_effort: None,
            gemini_thinking_budget: None,
            gemini_thinking_level: None,
            anthropic_thinking_budget: None,
        }),
        quick_ask_provider: None,
        quick_ask_model: None,
        quick_ask_system_prompt: None,
        context_grab_method: None,
        quick_replace_include_clipboard_context: None,
        quick_ask_include_clipboard_context: None,
        quick_replace_enabled: None,
        quick_replace_provider: None,
        quick_replace_model: None,
        quick_replace_system_prompt: None,
        quick_ask_openai_reasoning_effort: None,
        quick_ask_gemini_thinking_budget: None,
        quick_ask_gemini_thinking_level: None,
        quick_ask_anthropic_thinking_budget: None,

        rewrite_active_window_ocr_mode: None,
        quick_replace_active_window_ocr_mode: None,
        quick_ask_active_window_ocr_mode: None,
    }
}

#[tokio::test]
async fn routing_selects_preset_with_highest_similarity() {
    // Given: A profile with two presets and embeddings configured so that
    // "send email" is very similar to the "email" preset hint.
    // Create embeddings where:
    // - Transcript "send email" -> [1.0, 0.0, 0.0]
    // - "email and messages" hint -> [0.95, 0.05, 0.0] (high similarity to transcript)
    // - "calendar events" hint -> [0.0, 1.0, 0.0] (low similarity)
    let mock_embeddings = Arc::new(
        MockEmbeddingsProvider::new(vec![0.5, 0.5, 0.0])
            .with_embedding("send email", vec![1.0, 0.0, 0.0])
            .with_embedding("email and messages", vec![0.95, 0.05, 0.0])
            .with_embedding("calendar events", vec![0.0, 1.0, 0.0]),
    );

    let profile = create_routing_test_profile(vec![
        ("email-preset", "Email", vec!["email and messages"]),
        ("calendar-preset", "Calendar", vec!["calendar events"]),
    ]);

    let mut config = test_config_for_transcription();
    config.llm_config = mock_llm_config(true, vec![profile.clone()]);
    insert_mock_llm_api_key(&mut config);

    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));
    p.inject_stt_provider_for_tests(
        MOCK_PROVIDER,
        None,
        None,
        Arc::new(MockSttProvider::new("send email")),
    );
    p.inject_llm_provider_for_tests(
        MOCK_PROVIDER,
        None,
        Arc::new(MockLlmProvider::new("Sending email")),
    );
    p.inject_embeddings_provider_for_tests(mock_embeddings);

    // Set the active profile
    {
        let mut inner = p.inner.lock().expect("pipeline lock");
        inner.config.llm_config.program_prompt_profiles = vec![profile];
    }

    p.start_recording().expect("start recording should succeed");

    // When: we stop and transcribe
    let result = p
        .stop_and_transcribe_detailed()
        .await
        .expect("stop/transcribe should succeed");

    // Then: the pipeline successfully completed (LLM rewrite happened)
    assert_eq!(result.final_text, "Sending email");
    assert_eq!(p.state(), PipelineState::Idle);
}

#[tokio::test]
async fn rewrite_consumes_manual_ocr_result_when_available() {
    // Given: manual OCR mode, and an OCR result already produced (e.g. user clicked
    // the overlay OCR pill while recording).
    let mut profile = create_routing_test_profile(vec![]);
    profile.id = "default".to_string();
    profile.name = "Default".to_string();
    // Keep this test focused: we only care about rewrite user-message assembly.
    profile.router = None;

    let mut config = test_config_for_transcription();
    config.llm_config = mock_llm_config(true, vec![profile.clone()]);
    insert_mock_llm_api_key(&mut config);
    config.ocr_config.rewrite_mode = "manual".to_string();

    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));
    // STT produces any transcript (not important).
    p.inject_stt_provider_for_tests(
        MOCK_PROVIDER,
        None,
        None,
        Arc::new(MockSttProvider::new("hello")),
    );
    // LLM returns a different string depending on whether OCR context made it into the prompt.
    p.inject_llm_provider_for_tests(
        MOCK_PROVIDER,
        None,
        Arc::new(ConditionalOcrLlmProvider::new("WITH_OCR", "NO_OCR")),
    );

    {
        let mut inner = p.inner.lock().expect("pipeline lock");
        inner.config.llm_config.program_prompt_profiles = vec![profile];
    }

    p.start_recording().expect("start recording should succeed");

    // Simulate: OCR already finished before we hit rewrite.
    {
        let mut inner = p.inner.lock().expect("pipeline lock");
        inner.ocr.result = Some(crate::ocr::OcrResult {
            text: "some ocr text".to_string(),
            provider: "mock".to_string(),
            model: "mock-model".to_string(),
        });
    }

    let result = p
        .stop_and_transcribe_detailed()
        .await
        .expect("stop/transcribe should succeed");

    assert_eq!(result.final_text, "WITH_OCR");
    assert_eq!(p.state(), PipelineState::Idle);
}

#[tokio::test]
async fn routing_with_no_matching_preset_uses_default() {
    // Given: A profile with presets but transcript doesn't match any well
    // Create embeddings where the transcript doesn't match any hints well
    let mock_embeddings = Arc::new(
        MockEmbeddingsProvider::new(vec![0.5, 0.5, 0.0])
            .with_embedding("random unrelated text", vec![0.0, 0.0, 1.0]) // orthogonal to hints
            .with_embedding("email and messages", vec![1.0, 0.0, 0.0])
            .with_embedding("calendar events", vec![0.0, 1.0, 0.0]),
    );

    let mut profile = create_routing_test_profile(vec![
        ("email-preset", "Email", vec!["email and messages"]),
        ("calendar-preset", "Calendar", vec!["calendar events"]),
    ]);
    // Set a default preset
    profile.default_preset_id = Some("email-preset".to_string());
    // Disable pick_highest_score to use threshold-based selection
    if let Some(ref mut router) = profile.router {
        router.pick_highest_score = false;
        router.similarity_threshold = Some(0.8); // High threshold
    }

    let mut config = test_config_for_transcription();
    config.llm_config = mock_llm_config(true, vec![profile.clone()]);
    insert_mock_llm_api_key(&mut config);

    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));
    p.inject_stt_provider_for_tests(
        MOCK_PROVIDER,
        None,
        None,
        Arc::new(MockSttProvider::new("random unrelated text")),
    );
    p.inject_llm_provider_for_tests(
        MOCK_PROVIDER,
        None,
        Arc::new(MockLlmProvider::new("Processed text")),
    );
    p.inject_embeddings_provider_for_tests(mock_embeddings);

    {
        let mut inner = p.inner.lock().expect("pipeline lock");
        inner.config.llm_config.program_prompt_profiles = vec![profile];
    }

    p.start_recording().expect("start recording should succeed");

    // When: we stop and transcribe
    let result = p
        .stop_and_transcribe_detailed()
        .await
        .expect("stop/transcribe should succeed");

    // Then: pipeline completes successfully using the default preset
    assert_eq!(result.final_text, "Processed text");
    assert_eq!(p.state(), PipelineState::Idle);
}

#[tokio::test]
async fn routing_respects_session_preset_lock() {
    // Given: A profile with routing enabled BUT a session preset lock is set
    let mock_embeddings = Arc::new(
        MockEmbeddingsProvider::new(vec![0.5, 0.5, 0.0])
            .with_embedding("send email", vec![1.0, 0.0, 0.0])
            .with_embedding("email and messages", vec![0.95, 0.05, 0.0])
            .with_embedding("calendar events", vec![0.0, 1.0, 0.0]),
    );

    let profile = create_routing_test_profile(vec![
        ("email-preset", "Email", vec!["email and messages"]),
        ("calendar-preset", "Calendar", vec!["calendar events"]),
    ]);

    let mut config = test_config_for_transcription();
    config.llm_config = mock_llm_config(true, vec![profile.clone()]);
    insert_mock_llm_api_key(&mut config);

    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));
    p.inject_stt_provider_for_tests(
        MOCK_PROVIDER,
        None,
        None,
        Arc::new(MockSttProvider::new("send email")), // Would normally route to email-preset
    );
    p.inject_llm_provider_for_tests(
        MOCK_PROVIDER,
        None,
        Arc::new(MockLlmProvider::new("Calendar event created")),
    );
    p.inject_embeddings_provider_for_tests(mock_embeddings);

    {
        let mut inner = p.inner.lock().expect("pipeline lock");
        inner.config.llm_config.program_prompt_profiles = vec![profile];
    }

    // Set session lock to force calendar-preset (overriding what routing would choose)
    p.set_session_preset_lock(None, Some("calendar-preset".to_string()))
        .expect("set session lock");

    p.start_recording().expect("start recording should succeed");

    // When: we stop and transcribe
    let result = p
        .stop_and_transcribe_detailed()
        .await
        .expect("stop/transcribe should succeed");

    // Then: session lock was respected (we got calendar output despite email-like input)
    assert_eq!(result.final_text, "Calendar event created");
    assert_eq!(p.state(), PipelineState::Idle);
}

#[tokio::test]
async fn embeddings_provider_error_gracefully_falls_back() {
    // Given: An embeddings provider configured to fail
    let mock_embeddings = Arc::new(
        MockEmbeddingsProvider::new(vec![0.5, 0.5, 0.0])
            .with_error("Simulated embeddings API failure"),
    );

    let profile =
        create_routing_test_profile(vec![("email-preset", "Email", vec!["email and messages"])]);

    let mut config = test_config_for_transcription();
    config.llm_config = mock_llm_config(true, vec![profile.clone()]);
    insert_mock_llm_api_key(&mut config);

    let p = SharedPipeline::new_for_tests(config, Box::new(FakeAudioCapture::new()));
    p.inject_stt_provider_for_tests(
        MOCK_PROVIDER,
        None,
        None,
        Arc::new(MockSttProvider::new("send email")),
    );
    p.inject_llm_provider_for_tests(
        MOCK_PROVIDER,
        None,
        Arc::new(MockLlmProvider::new("Fallback output")),
    );
    p.inject_embeddings_provider_for_tests(mock_embeddings);

    {
        let mut inner = p.inner.lock().expect("pipeline lock");
        inner.config.llm_config.program_prompt_profiles = vec![profile];
    }

    p.start_recording().expect("start recording should succeed");

    // When: we stop and transcribe (embeddings will fail)
    let result = p
        .stop_and_transcribe_detailed()
        .await
        .expect("stop/transcribe should succeed even with embeddings failure");

    // Then: pipeline completes successfully (routing failed but LLM rewrite still worked)
    assert_eq!(result.final_text, "Fallback output");
    assert_eq!(p.state(), PipelineState::Idle);
}
