use crate::audio_capture::AudioCaptureError;
use crate::llm::LlmError;
use crate::stt::SttError;
use std::time::Duration;

use crate::stt::RetryTelemetry;

/// Errors that can occur in the recording pipeline
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
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

// Backwards-compatibility: `PipelineError::NoProvider` is still part of the public API.

/// Events emitted by the pipeline
#[allow(dead_code)]
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

/// Reason the optional LLM formatting step was not attempted.
///
/// This is used to make request logs unambiguous when the rewrite step does not run.
#[derive(Debug, Clone, PartialEq)]
pub enum LlmNotAttemptedReason {
    /// Recording was gated as quiet (STT skipped), so LLM rewrite was never reached.
    QuietAudioGate,
    /// Offline VAD detected no speech (STT skipped), so LLM rewrite was never reached.
    NoSpeechDetectedByVad,
    /// Default/global rewrite toggle is disabled.
    ///
    /// Historically rewrite enablement lived in a global setting (`rewrite_llm_enabled`) and
    /// the Default profile inherited it. We keep this reason so request logs stay explicit.
    DisabledByDefaultProfile,
    /// Per-profile toggle explicitly disabled rewrite.
    DisabledByProfile,
    /// Selected preset explicitly disabled rewrite.
    DisabledByPreset,
    /// Routed to the implicit "Default" target (no preset), which explicitly disabled rewrite.
    DisabledByDefaultTarget,
    /// Rewrite was enabled, but the provider couldn't be constructed/used.
    ProviderUnavailable {
        provider: String,
        error: String,
    },
    /// Fallback for unexpected paths.
    Unknown,
    MeetingMode,
}

impl LlmNotAttemptedReason {
    pub fn code(&self) -> &'static str {
        match self {
            LlmNotAttemptedReason::QuietAudioGate => "quiet_audio_gate",
            LlmNotAttemptedReason::NoSpeechDetectedByVad => "no_speech_detected_by_vad",
            LlmNotAttemptedReason::DisabledByDefaultProfile => "disabled_default_profile",
            LlmNotAttemptedReason::DisabledByProfile => "disabled_profile",
            LlmNotAttemptedReason::DisabledByPreset => "disabled_preset",
            LlmNotAttemptedReason::DisabledByDefaultTarget => "disabled_default_target",
            LlmNotAttemptedReason::ProviderUnavailable { .. } => "provider_unavailable",
            LlmNotAttemptedReason::Unknown => "unknown",
            LlmNotAttemptedReason::MeetingMode => "meeting_mode",
        }
    }

    pub fn to_log_details(&self) -> String {
        match self {
            LlmNotAttemptedReason::QuietAudioGate => {
                "reason=stt_skipped_quiet_audio_gate".to_string()
            }
            LlmNotAttemptedReason::NoSpeechDetectedByVad => {
                "reason=stt_skipped_no_speech_detected".to_string()
            }
            LlmNotAttemptedReason::DisabledByDefaultProfile => {
                "reason=disabled_default_profile".to_string()
            }
            LlmNotAttemptedReason::DisabledByProfile => "reason=disabled_profile".to_string(),
            LlmNotAttemptedReason::DisabledByPreset => "reason=disabled_preset".to_string(),
            LlmNotAttemptedReason::DisabledByDefaultTarget => {
                "reason=disabled_default_target".to_string()
            }
            LlmNotAttemptedReason::ProviderUnavailable { provider, error } => format!(
                "reason=provider_unavailable\nprovider={}\nerror={}",
                provider, error
            ),
            LlmNotAttemptedReason::Unknown => "reason=unknown".to_string(),
            LlmNotAttemptedReason::MeetingMode => "reason=meeting_mode".to_string(),
        }
    }
}

/// Outcome of the optional LLM formatting step.
#[derive(Debug, Clone, PartialEq)]
pub enum LlmOutcome {
    /// LLM step was not attempted.
    NotAttempted(LlmNotAttemptedReason),
    /// LLM step completed successfully and returned formatted text.
    Succeeded,
    /// LLM step timed out and the pipeline fell back to the raw STT transcript.
    TimedOut,
    /// LLM step failed and the pipeline fell back to the raw STT transcript.
    Failed(String),
}

impl LlmOutcome {
    pub fn code(&self) -> &'static str {
        match self {
            LlmOutcome::NotAttempted(_) => "not_attempted",
            LlmOutcome::Succeeded => "succeeded",
            LlmOutcome::TimedOut => "timed_out",
            LlmOutcome::Failed(_) => "failed",
        }
    }
}

/// Detailed result for a transcription request.
///
/// This separates the raw STT transcript from the final output (which may
/// include LLM formatting and/or fallbacks).
#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    pub speaker_segments: Vec<crate::stt::SpeakerSegment>,
    /// Raw transcript as returned from the STT provider (before any LLM formatting).
    pub stt_text: String,
    /// Final output text returned by the pipeline.
    /// If LLM formatting was disabled, this will match `stt_text`.
    /// If LLM formatting failed/timed out, this falls back to `stt_text`.
    pub final_text: String,
    /// Duration of the STT phase (including retries), in milliseconds.
    pub stt_duration_ms: u64,
    /// Structured retry/backoff telemetry for the STT phase (if available).
    ///
    /// This is primarily used for CLI diagnostics/benchmarks so we can tell whether
    /// a slow STT duration was due to retries/backoff (e.g. rate limiting).
    pub stt_retry: Option<RetryTelemetry>,
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
    /// True when live output (streaming paste) already pasted the transcript
    /// during recording, so the caller should skip the final paste step.
    pub live_output_completed: bool,
}

impl TranscriptionResult {
    pub fn llm_attempted(&self) -> bool {
        !matches!(self.llm_outcome, LlmOutcome::NotAttempted(_))
    }
}
