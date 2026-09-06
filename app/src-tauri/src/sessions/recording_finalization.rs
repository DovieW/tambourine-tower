//! Shared recording finalization helpers.
//!
//! This Module owns the narrow, repeatable tail work that happens after STT/LLM processing has
//! reached a terminal point: stamping request-log success metadata, mirroring preset/model chips
//! into History, closing the current request log with cost events, and ending the matching OCR
//! session. It deliberately does **not** own Quick Ask/Quick Replace execution or platform text
//! output; those remain in their dedicated Modules so this stays a small finalization seam.

use tauri::{AppHandle, Manager};

use crate::history::RequestHistoryUpdate;
use crate::history_request_lifecycle;
use crate::pipeline::{LlmOutcome, SharedPipeline, TranscriptionResult};
use crate::request_log::{RequestLog, RequestLogStore};
use crate::stats::{self, EventStatus};

/// Inputs for stamping a successful transcription result onto the current request log.
pub(crate) struct TranscriptionSuccessLogUpdate<'a> {
    pub(crate) result: &'a TranscriptionResult,
    /// `None` means the caller intentionally has no user-visible output after sanitization.
    pub(crate) formatted_transcript: Option<&'a str>,
    pub(crate) audio_duration_secs: Option<f64>,
    pub(crate) audio_size_bytes: Option<usize>,
    /// Human-readable prefix for the STT completion line, e.g. `"STT"` or `"Retry STT"`.
    pub(crate) stt_summary_label: &'a str,
    /// Optional flow-specific final log line after the shared STT/LLM metadata is stamped.
    pub(crate) completion_log_message: Option<&'a str>,
    pub(crate) warn_if_no_formatted_transcript: bool,
}

/// Stamp the shared STT/LLM success metadata onto a request log.
///
/// Keep this helper intentionally data-only: callers still decide when the request can be marked
/// success/error/cancelled, because Quick Replace and Quick Ask may keep the log open for their
/// extra LLM work after transcription itself succeeds.
pub(crate) fn record_transcription_success(
    log: &mut RequestLog,
    update: TranscriptionSuccessLogUpdate<'_>,
) {
    let result = update.result;

    log.raw_transcript = Some(result.stt_text.clone());
    if let Some(text) = update.formatted_transcript {
        log.formatted_transcript = Some(text.to_string());
    }

    log.stt_duration_ms = Some(result.stt_duration_ms);
    log.llm_duration_ms = result.llm_duration_ms;
    log.llm_outcome = Some(result.llm_outcome.code().to_string());
    log.llm_not_attempted_reason = None;
    log.llm_error_message = None;

    // Prefer the WAV-derived duration when available; fall back to OpenAI's verbose response
    // duration because OpenAI can expose provider timing even when local WAV parsing failed.
    let audio_secs = update.audio_duration_secs.or_else(|| {
        if log.stt_provider == "openai" {
            log.stt_response_json
                .as_ref()
                .and_then(stats::parse_openai_stt_duration_secs_from_response_json)
        } else {
            None
        }
    });
    log.audio_duration_secs = audio_secs.map(|secs| secs as f32);
    log.audio_size_bytes = update.audio_size_bytes;

    if result.llm_attempted() {
        // Use the concrete provider/model actually used, including provider defaults.
        log.llm_provider = result.llm_provider_used.clone();
        log.llm_model = result.llm_model_used.clone();
    } else {
        // Avoid misleading chips when no LLM rewrite was attempted.
        log.llm_provider = None;
        log.llm_model = None;
    }

    log.info(format!(
        "{} completed in {}ms ({} chars)",
        update.stt_summary_label,
        result.stt_duration_ms,
        result.stt_text.len()
    ));

    record_llm_outcome(log, result);

    if update.warn_if_no_formatted_transcript && update.formatted_transcript.is_none() {
        log.warn("No transcript output (empty/whitespace)");
    }

    if let Some(message) = update.completion_log_message {
        log.info(message);
    }
}

fn record_llm_outcome(log: &mut RequestLog, result: &TranscriptionResult) {
    match &result.llm_outcome {
        LlmOutcome::NotAttempted(reason) => {
            log.llm_not_attempted_reason = Some(reason.code().to_string());
            if let crate::pipeline::LlmNotAttemptedReason::ProviderUnavailable { .. } = reason {
                log.llm_error_message = Some(reason.to_log_details());
            }
            log.info_with_details("LLM formatting not attempted", reason.to_log_details());
        }
        LlmOutcome::Succeeded => {
            if let Some(ms) = result.llm_duration_ms {
                log.info(format!(
                    "LLM formatting succeeded in {}ms ({} -> {} chars)",
                    ms,
                    result.stt_text.len(),
                    result.final_text.len()
                ));
            } else {
                log.info("LLM formatting succeeded");
            }
        }
        LlmOutcome::TimedOut => {
            if let Some(ms) = result.llm_duration_ms {
                log.warn(format!(
                    "LLM formatting timed out after {}ms; fell back to STT transcript",
                    ms
                ));
            } else {
                log.warn("LLM formatting timed out; fell back to STT transcript");
            }
        }
        LlmOutcome::Failed(err) => {
            log.llm_error_message = Some(err.clone());
            log.warn(format!(
                "LLM formatting failed; fell back to STT transcript ({})",
                err
            ));
        }
    }
}

/// Complete the current request log, emit cost using the supplied WAV bytes, and end OCR.
pub(crate) fn complete_current_request_with_cost(
    app: &AppHandle,
    pipeline: &SharedPipeline,
    request_id: Option<&str>,
    status: EventStatus,
    wav_bytes: Option<&[u8]>,
) {
    complete_current_request_with_cost_inner(app, pipeline, request_id, status, wav_bytes, false);
}

/// Same as `complete_current_request_with_cost`, but preserves legacy command paths that emitted
/// best-effort stats even when the request-log store was unavailable.
pub(crate) fn complete_current_request_with_cost_best_effort(
    app: &AppHandle,
    pipeline: &SharedPipeline,
    request_id: Option<&str>,
    status: EventStatus,
    wav_bytes: Option<&[u8]>,
) {
    complete_current_request_with_cost_inner(app, pipeline, request_id, status, wav_bytes, true);
}

/// Complete using the pipeline's last WAV snapshot.
pub(crate) fn complete_current_request_with_pipeline_wav(
    app: &AppHandle,
    pipeline: &SharedPipeline,
    request_id: Option<&str>,
    status: EventStatus,
) {
    let wav = pipeline.clone_last_wav_bytes();
    if let Some(wav) = wav.as_deref() {
        complete_current_request_with_cost(app, pipeline, request_id, status, Some(wav));
    } else {
        // Several legacy paths only emitted stats when a real WAV snapshot was available. Keep
        // that behavior so a missing capture cannot create duration-less cost rows by accident.
        complete_current_request_without_cost(app, pipeline, request_id);
    }
}

fn complete_current_request_with_cost_inner(
    app: &AppHandle,
    pipeline: &SharedPipeline,
    request_id: Option<&str>,
    status: EventStatus,
    wav_bytes: Option<&[u8]>,
    emit_without_log_store: bool,
) {
    let mut emitted_with_log_store = false;
    if let (Some(id), Some(duration), Some(history)) = (
        request_id,
        wav_bytes.and_then(stats::wav_duration_secs),
        app.try_state::<crate::history::HistoryStorage>(),
    ) {
        if history.set_recording_duration(id, duration).is_err() {
            log::warn!("Could not persist History audio duration");
        }
    }

    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        stats::emit_cost_events_for_current_request(app, status, wav_bytes);
        emitted_with_log_store = true;
        log_store.complete_current();
    }

    if emit_without_log_store && !emitted_with_log_store {
        stats::emit_cost_events_for_current_request(app, status, wav_bytes);
    }

    end_ocr_session_for_request(pipeline, request_id);
}

pub(crate) fn complete_current_request_without_cost(
    app: &AppHandle,
    pipeline: &SharedPipeline,
    request_id: Option<&str>,
) {
    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        log_store.complete_current();
    }

    end_ocr_session_for_request(pipeline, request_id);
}

pub(crate) fn end_ocr_session_for_request(pipeline: &SharedPipeline, request_id: Option<&str>) {
    if let Some(req_id) = request_id {
        pipeline.end_ocr_session_if_matches(req_id);
    }
}

/// Mirror the preset selected during routing from the request log into History.
pub(crate) fn persist_current_request_preset_to_history(app: &AppHandle, request_id: Option<&str>) {
    let Some(req_id) = request_id else {
        return;
    };

    let Some(log_store) = app.try_state::<RequestLogStore>() else {
        return;
    };

    let preset_meta =
        log_store.with_current(|log| (log.preset_id.clone(), log.preset_name.clone()));
    let Some((preset_id, preset_name)) = preset_meta else {
        return;
    };

    let _ = history_request_lifecycle::apply_request_history_update(
        app,
        RequestHistoryUpdate::SetPreset {
            request_id: req_id.to_string(),
            preset_id,
            preset_name,
        },
    );
}

pub(crate) fn persist_history_llm_metadata(
    app: &AppHandle,
    request_id: Option<&str>,
    result: &TranscriptionResult,
) {
    let Some(req_id) = request_id else {
        return;
    };

    let (provider, model) = llm_metadata_for_history(result);
    let _ = history_request_lifecycle::apply_request_history_update(
        app,
        RequestHistoryUpdate::SetLlmModel {
            request_id: req_id.to_string(),
            llm_provider: provider,
            llm_model: model,
        },
    );
}

pub(crate) fn llm_metadata_for_history(
    result: &TranscriptionResult,
) -> (Option<String>, Option<String>) {
    if result.llm_attempted() {
        (
            result.llm_provider_used.clone(),
            result.llm_model_used.clone(),
        )
    } else {
        (None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{LlmNotAttemptedReason, LlmOutcome};

    #[test]
    fn history_llm_metadata_is_only_kept_when_llm_was_attempted() {
        let attempted = transcription_result(LlmOutcome::Succeeded);
        assert_eq!(
            llm_metadata_for_history(&attempted),
            (Some("openai".into()), Some("gpt-test".into()))
        );

        let not_attempted = transcription_result(LlmOutcome::NotAttempted(
            LlmNotAttemptedReason::DisabledByDefaultProfile,
        ));
        assert_eq!(llm_metadata_for_history(&not_attempted), (None, None));
    }

    #[test]
    fn success_log_update_uses_actual_llm_metadata() {
        let result = transcription_result(LlmOutcome::Succeeded);
        let mut log = RequestLog::new("openai".into(), Some("whisper-test".into()));

        record_transcription_success(
            &mut log,
            TranscriptionSuccessLogUpdate {
                result: &result,
                formatted_transcript: Some("final"),
                audio_duration_secs: Some(1.25),
                audio_size_bytes: Some(42),
                stt_summary_label: "STT",
                completion_log_message: Some("Transcription completed; output pending"),
                warn_if_no_formatted_transcript: true,
            },
        );

        assert_eq!(log.raw_transcript.as_deref(), Some("raw"));
        assert_eq!(log.formatted_transcript.as_deref(), Some("final"));
        assert_eq!(log.audio_duration_secs, Some(1.25_f32));
        assert_eq!(log.audio_size_bytes, Some(42));
        assert_eq!(log.llm_provider.as_deref(), Some("openai"));
        assert_eq!(log.llm_model.as_deref(), Some("gpt-test"));
        assert_eq!(log.llm_outcome.as_deref(), Some("succeeded"));
    }

    fn transcription_result(llm_outcome: LlmOutcome) -> TranscriptionResult {
        TranscriptionResult {
            speaker_segments: Vec::new(),
            stt_text: "raw".into(),
            final_text: "final".into(),
            stt_duration_ms: 1,
            stt_retry: None,
            llm_duration_ms: Some(2),
            llm_provider_used: Some("openai".into()),
            llm_model_used: Some("gpt-test".into()),
            llm_outcome,
            live_output_completed: false,
        }
    }
}
