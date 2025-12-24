//! Tauri commands for the recording pipeline.
//!
//! These commands expose the recording pipeline functionality to the frontend,
//! enabling voice dictation directly from the Tauri app.

use crate::audio_capture::VadAutoStopConfig;
use crate::pipeline::{LlmOutcome, PipelineConfig, PipelineError, PipelineState, SharedPipeline};
use crate::recordings::{RecordingStore, RecordingsStats};
use crate::request_log::RequestLogStore;
use crate::history::{HistoryStorage, RequestModelInfo};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

#[cfg(desktop)]
use tauri_plugin_store::StoreExt;

fn get_max_saved_recordings(app: &AppHandle) -> usize {
    #[cfg(desktop)]
    {
        let default: u64 = 1000;
        let raw = app
            .store("settings.json")
            .ok()
            .and_then(|store| store.get("max_saved_recordings"))
            .and_then(|v| v.as_u64())
            .unwrap_or(default);

        return (raw.clamp(1, 100_000)) as usize;
    }

    #[cfg(not(desktop))]
    {
        1000
    }
}

/// Tauri-compatible error type for commands
#[derive(Debug, serde::Serialize)]
pub struct CommandError {
    pub message: String,
    pub error_type: String,
}

impl From<PipelineError> for CommandError {
    fn from(err: PipelineError) -> Self {
        let error_type = match &err {
            PipelineError::AudioCapture(_) => "audio",
            PipelineError::Stt(_) => "stt",
            PipelineError::Llm(_) => "llm",
            PipelineError::NoProvider => "config",
            PipelineError::AlreadyRecording => "state",
            PipelineError::NotRecording => "state",
            PipelineError::Config(_) => "config",
            PipelineError::Lock(_) => "internal",
            PipelineError::Cancelled => "cancelled",
            PipelineError::Timeout(_) => "timeout",
            PipelineError::RecordingTooLarge(_, _) => "size",
        };
        Self {
            message: err.to_string(),
            error_type: error_type.to_string(),
        }
    }
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self {
            message,
            error_type: "unknown".to_string(),
        }
    }
}

/// Get the absolute path to a saved WAV recording for a given request id.
///
/// Returns `null` when the recording doesn't exist.
#[tauri::command]
pub fn recording_get_wav_path(
    app: AppHandle,
    request_id: String,
) -> Result<Option<String>, CommandError> {
    let store = app
        .try_state::<RecordingStore>()
        .ok_or_else(|| CommandError::from("Recording store not available".to_string()))?;

    let path = store.wav_path_if_exists(&request_id).map_err(CommandError::from)?;
    Ok(path.map(|p| p.to_string_lossy().to_string()))
}

/// Get saved WAV bytes for a given request id as base64.
///
/// Some webviews can fail to play `convertFileSrc` URLs for WAVs if the asset protocol
/// serves an unexpected content-type; base64+Blob playback is a reliable fallback.
///
/// Returns `null` when the recording doesn't exist.
#[tauri::command]
pub fn recording_get_wav_base64(
    app: AppHandle,
    request_id: String,
) -> Result<Option<String>, CommandError> {
    use base64::Engine;

    let store = app
        .try_state::<RecordingStore>()
        .ok_or_else(|| CommandError::from("Recording store not available".to_string()))?;

    // Reuse the same validation / existence semantics.
    let path = store.wav_path_if_exists(&request_id).map_err(CommandError::from)?;
    let Some(_) = path else {
        return Ok(None);
    };

    let wav = store.load_wav(&request_id).map_err(CommandError::from)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(wav);
    Ok(Some(encoded))
}

/// Open the recordings folder in the OS file manager.
#[tauri::command]
pub fn recordings_open_folder(app: AppHandle) -> Result<(), CommandError> {
    let store = app
        .try_state::<RecordingStore>()
        .ok_or_else(|| CommandError::from("Recording store not available".to_string()))?;

    open::that(store.directory())
        .map_err(|e| CommandError::from(format!("Failed to open recordings folder: {}", e)))
}

/// Total bytes used by saved recordings on disk.
#[tauri::command]
pub fn recordings_get_storage_bytes(app: AppHandle) -> Result<u64, CommandError> {
    let store = app
        .try_state::<RecordingStore>()
        .ok_or_else(|| CommandError::from("Recording store not available".to_string()))?;

    store.total_size_bytes().map_err(CommandError::from)
}

/// Stats about saved recordings (count + total bytes).
#[tauri::command]
pub fn recordings_get_stats(app: AppHandle) -> Result<RecordingsStats, CommandError> {
    let store = app
        .try_state::<RecordingStore>()
        .ok_or_else(|| CommandError::from("Recording store not available".to_string()))?;

    store.stats().map_err(CommandError::from)
}

/// Start recording audio using the pipeline
#[tauri::command]
pub fn pipeline_start_recording(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
) -> Result<(), CommandError> {
    // Start request logging
    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        let config = pipeline.config();
        log_store.start_request(
            config.stt_provider.clone(),
            config.stt_model.clone(),
        );
        log_store.with_current(|log| {
            log.llm_provider = if config.llm_config.enabled {
                Some(config.llm_config.provider.clone())
            } else {
                None
            };
            log.llm_model = config.llm_config.model.clone();
            log.info("Recording started");
        });
    }

    pipeline.start_recording().map_err(|e| {
        if let Some(log_store) = app.try_state::<RequestLogStore>() {
            log_store.with_current(|log| {
                log.error(format!("Failed to start recording: {}", e));
                log.complete_error(e.to_string());
            });
            log_store.complete_current();
        }
        CommandError::from(e)
    })?;

    // While recording/transcribing, allow Escape to cancel without triggering transcription.
    #[cfg(desktop)]
    crate::set_escape_cancel_shortcut_enabled(&app, true);

    // Emit event to frontend
    let _ = app.emit("pipeline-recording-started", ());

    Ok(())
}

/// Stop recording and transcribe the audio
#[tauri::command]
pub async fn pipeline_stop_and_transcribe(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
) -> Result<String, CommandError> {
    let max_saved_recordings = get_max_saved_recordings(&app);

    // Ensure Escape-to-cancel is available during the transcription phase.
    #[cfg(desktop)]
    crate::set_escape_cancel_shortcut_enabled(&app, true);

    // Try to capture the active request id for history + persistent audio.
    let active_request_id: Option<String> = app
        .try_state::<RequestLogStore>()
        .and_then(|store| store.with_current(|log| log.id.clone()));

    // Capture model info for persistence in history.
    let model_info = {
        let config = pipeline.config();
        RequestModelInfo {
            stt_provider: Some(config.stt_provider.clone()),
            stt_model: config.stt_model.clone(),
            llm_provider: if config.llm_config.enabled {
                Some(config.llm_config.provider.clone())
            } else {
                None
            },
            llm_model: config.llm_config.model.clone(),
        }
    };

    // Create an in-progress history entry so the History view shows a running request.
    if let Some(req_id) = active_request_id.as_deref() {
        if let Some(history) = app.try_state::<HistoryStorage>() {
            let _ = history.add_request_entry(
                req_id.to_string(),
                model_info,
                max_saved_recordings,
            );
            let _ = app.emit("history-changed", ());
        }
    }

    // Emit transcription started *only if* the pipeline actually enters the
    // Transcribing state.
    //
    // This prevents the overlay from briefly showing "TRANSCRIBING..." when the
    // quiet-audio gate (hallucination protection) decides to skip STT.
    {
        let app_clone = app.clone();
        let pipeline_clone = pipeline.inner().clone();
        tauri::async_runtime::spawn(async move {
            let start = Instant::now();
            loop {
                match pipeline_clone.state() {
                    PipelineState::Transcribing | PipelineState::Rewriting => {
                        let _ = app_clone.emit("pipeline-transcription-started", ());
                        break;
                    }
                    PipelineState::Idle | PipelineState::Error => {
                        // Quiet-audio skip resets to Idle; errors also shouldn't show
                        // a "transcribing" phase.
                        break;
                    }
                    PipelineState::Recording => {
                        // Still finalizing stop.
                    }
                }

                if start.elapsed() > Duration::from_secs(2) {
                    break;
                }

                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        });
    }

    // Log transcription start
    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        log_store.with_current(|log| {
            log.info("Recording stopped, starting transcription");
        });
    }

    let result = match pipeline.stop_and_transcribe_detailed().await {
        Ok(r) => r,
        Err(PipelineError::Cancelled) => {
            // User cancelled (Escape / cancel button). Treat as a normal outcome.
            #[cfg(desktop)]
            crate::set_escape_cancel_shortcut_enabled(&app, false);

            // Best-effort: complete current request as cancelled.
            if let Some(log_store) = app.try_state::<RequestLogStore>() {
                log_store.with_current(|log| {
                    log.warn("Recording cancelled by user");
                    log.complete_cancelled();
                });
                log_store.complete_current();
            }

            let _ = app.emit("pipeline-cancelled", ());
            return Ok(String::new());
        }
        Err(e) => {
            #[cfg(desktop)]
            crate::set_escape_cancel_shortcut_enabled(&app, false);

            if let Some(log_store) = app.try_state::<RequestLogStore>() {
                log_store.with_current(|log| {
                    log.error(format!("Transcription failed: {}", e));
                    log.complete_error(e.to_string());
                });
                log_store.complete_current();
            }

            // Update history entry with error (keep it visible for retry)
            if let Some(req_id) = active_request_id.as_deref() {
                if let Some(history) = app.try_state::<HistoryStorage>() {
                    let _ = history.complete_request_error(req_id, e.to_string());
                    let _ = app.emit("history-changed", ());
                }
            }

            // Persist audio for retry (best-effort)
            if let (Some(req_id), Some(store)) = (
                active_request_id.as_deref(),
                app.try_state::<RecordingStore>(),
            ) {
                if let Some(wav) = pipeline.clone_last_wav_bytes() {
                    if store.save_wav(req_id, &wav).is_ok() {
                        let _ = store.prune_to_max_files(max_saved_recordings);
                    }
                }
            }

            // Emit pipeline-error event with request_id so the overlay can show a retry button.
            let payload = serde_json::json!({
                "message": e.to_string(),
                "request_id": active_request_id.clone(),
            });
            let _ = app.emit("pipeline-error", payload);

            return Err(CommandError::from(e));
        }
    };

    let final_text = result.final_text.clone();

    // Log success
    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        log_store.with_current(|log| {
            log.raw_transcript = Some(result.stt_text.clone());
            log.formatted_transcript = Some(result.final_text.clone());
            log.stt_duration_ms = Some(result.stt_duration_ms);
            log.llm_duration_ms = result.llm_duration_ms;

            // Use the provider instance's model (includes provider defaults) so the UI can show
            // the real model used even if no explicit model override was configured.
            if result.llm_attempted() {
                log.llm_provider = result.llm_provider_used.clone();
                log.llm_model = result.llm_model_used.clone();
            }

            log.info(format!(
                "STT completed in {}ms ({} chars)",
                result.stt_duration_ms,
                result.stt_text.len()
            ));

            match &result.llm_outcome {
                LlmOutcome::NotAttempted => {
                    log.info("LLM formatting not attempted (disabled or unavailable)");
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
                    log.warn(format!(
                        "LLM formatting failed; fell back to STT transcript ({})",
                        err
                    ));
                }
            }

            log.complete_success();
        });
        log_store.complete_current();
    }

    // Persist audio for retry (best-effort)
    if let (Some(req_id), Some(store)) = (
        active_request_id.as_deref(),
        app.try_state::<RecordingStore>(),
    ) {
        if let Some(wav) = pipeline.clone_last_wav_bytes() {
            if store.save_wav(req_id, &wav).is_ok() {
                let _ = store.prune_to_max_files(max_saved_recordings);
            }
        }
    }

    // Update history entry with success text
    if let Some(req_id) = active_request_id.as_deref() {
        if let Some(history) = app.try_state::<HistoryStorage>() {
            let _ = history.complete_request_success(req_id, final_text.clone());
            let _ = app.emit("history-changed", ());
        }
    }

    // Emit transcript ready event
    let _ = app.emit("pipeline-transcript-ready", &final_text);

    // Done transcribing - stop stealing Escape.
    #[cfg(desktop)]
    crate::set_escape_cancel_shortcut_enabled(&app, false);

    Ok(final_text)
}

/// Retry transcription for a prior request id.
///
/// Loads the saved WAV (if available), creates a new request log + history entry,
/// and re-runs STT + optional LLM formatting.
#[tauri::command]
pub async fn pipeline_retry_transcription(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
    request_id: String,
) -> Result<String, CommandError> {
    let max_saved_recordings = get_max_saved_recordings(&app);

    // Allow Escape-to-cancel while the retry transcription is running.
    #[cfg(desktop)]
    crate::set_escape_cancel_shortcut_enabled(&app, true);

    let recording_store = app
        .try_state::<RecordingStore>()
        .ok_or_else(|| CommandError::from("Recording store not available".to_string()))?;

    let wav = recording_store
        .load_wav(&request_id)
        .map_err(CommandError::from)?;

    // Start a *new* request log for the retry attempt.
    let config = pipeline.config();
    let new_request_id: Option<String> = app.try_state::<RequestLogStore>().map(|log_store| {
        log_store.start_request(config.stt_provider.clone(), config.stt_model.clone())
    });

    // Capture model info for persistence in history.
    let model_info = RequestModelInfo {
        stt_provider: Some(config.stt_provider.clone()),
        stt_model: config.stt_model.clone(),
        llm_provider: if config.llm_config.enabled {
            Some(config.llm_config.provider.clone())
        } else {
            None
        },
        llm_model: config.llm_config.model.clone(),
    };

    // Create a history entry for the retry attempt.
    if let Some(req_id) = new_request_id.as_deref() {
        if let Some(history) = app.try_state::<HistoryStorage>() {
            let _ = history.add_request_entry(
                req_id.to_string(),
                model_info,
                max_saved_recordings,
            );
            let _ = app.emit("history-changed", ());
        }
    }

    let _ = app.emit("pipeline-transcription-started", ());

    // Run the retry transcription (STT + optional LLM)
    let result = match pipeline.transcribe_wav_bytes_detailed(wav.clone()).await {
        Ok(r) => r,
        Err(PipelineError::Cancelled) => {
            #[cfg(desktop)]
            crate::set_escape_cancel_shortcut_enabled(&app, false);
            let _ = app.emit("pipeline-cancelled", ());
            return Ok(String::new());
        }
        Err(e) => {
            #[cfg(desktop)]
            crate::set_escape_cancel_shortcut_enabled(&app, false);

            if let Some(log_store) = app.try_state::<RequestLogStore>() {
                log_store.with_current(|log| {
                    log.error(format!("Retry transcription failed: {}", e));
                    log.complete_error(e.to_string());
                });
                log_store.complete_current();
            }

            if let Some(req_id) = new_request_id.as_deref() {
                if let Some(history) = app.try_state::<HistoryStorage>() {
                    let _ = history.complete_request_error(req_id, e.to_string());
                    let _ = app.emit("history-changed", ());
                }
            }

            // Also emit pipeline-error so the overlay can present the always-on-top retry UI.
            let payload = serde_json::json!({
                "message": e.to_string(),
                "request_id": new_request_id,
            });
            let _ = app.emit("pipeline-error", payload);

            return Err(CommandError::from(e));
        }
    };

    // Persist audio under the *new* request id (best-effort)
    if let Some(req_id) = new_request_id.as_deref() {
        if recording_store.save_wav(req_id, &wav).is_ok() {
            let _ = recording_store.prune_to_max_files(max_saved_recordings);
        }
    }

    let final_text = result.final_text.clone();

    // Update log store on success
    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        log_store.with_current(|log| {
            log.raw_transcript = Some(result.stt_text.clone());
            log.formatted_transcript = Some(result.final_text.clone());
            log.stt_duration_ms = Some(result.stt_duration_ms);
            log.llm_duration_ms = result.llm_duration_ms;

            if result.llm_attempted() {
                log.llm_provider = result.llm_provider_used.clone();
                log.llm_model = result.llm_model_used.clone();
            }

            log.info(format!(
                "Retry STT completed in {}ms ({} chars)",
                result.stt_duration_ms,
                result.stt_text.len()
            ));
            log.complete_success();
        });
        log_store.complete_current();
    }

    // Update history on success
    if let Some(req_id) = new_request_id.as_deref() {
        if let Some(history) = app.try_state::<HistoryStorage>() {
            let _ = history.complete_request_success(req_id, final_text.clone());
            let _ = app.emit("history-changed", ());
        }
    }

    // Emit transcript ready event
    let _ = app.emit("pipeline-transcript-ready", &final_text);

    #[cfg(desktop)]
    crate::set_escape_cancel_shortcut_enabled(&app, false);

    Ok(final_text)
}

/// Cancel the current recording/transcription
#[tauri::command]
pub fn pipeline_cancel(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
) -> Result<(), CommandError> {
    // `pipeline` is kept for API stability; on desktop we delegate to a helper that
    // re-acquires the shared pipeline from app state.
    let _ = pipeline;

    #[cfg(desktop)]
    {
        // Reuse the centralized cancel logic so audio mute/pause state is restored too.
        crate::cancel_pipeline_session(&app, "Command");
        return Ok(());
    }

    #[cfg(not(desktop))]
    {
        // Log cancellation
        if let Some(log_store) = app.try_state::<RequestLogStore>() {
            log_store.with_current(|log| {
                log.warn("Recording cancelled by user");
                log.complete_cancelled();
            });
            log_store.complete_current();
        }

        pipeline.cancel();

        // Emit cancelled event
        let _ = app.emit("pipeline-cancelled", ());

        Ok(())
    }
}

/// Get the current pipeline state
#[tauri::command]
pub fn pipeline_get_state(
    pipeline: State<'_, SharedPipeline>,
) -> Result<String, CommandError> {
    let state = pipeline.state();
    let state_str = match state {
        PipelineState::Idle => "idle",
        PipelineState::Recording => "recording",
        PipelineState::Transcribing => "transcribing",
        PipelineState::Rewriting => "rewriting",
        PipelineState::Error => "error",
    };
    Ok(state_str.to_string())
}

/// Check if the pipeline is currently recording
#[tauri::command]
pub fn pipeline_is_recording(
    pipeline: State<'_, SharedPipeline>,
) -> Result<bool, CommandError> {
    Ok(pipeline.is_recording())
}

/// Configuration payload for updating the pipeline
#[derive(Debug, serde::Deserialize)]
pub struct PipelineConfigPayload {
    pub stt_provider: Option<String>,
    pub stt_api_key: Option<String>,
    pub stt_model: Option<String>,
    pub max_duration_secs: Option<f32>,
    pub max_retries: Option<u32>,
    pub vad_enabled: Option<bool>,
    pub vad_auto_stop: Option<bool>,
    /// Timeout in seconds for transcription requests
    pub transcription_timeout_secs: Option<u64>,
    /// Maximum recording size in bytes
    pub max_recording_bytes: Option<usize>,
}

/// Update the pipeline configuration
#[tauri::command]
pub fn pipeline_update_config(
    pipeline: State<'_, SharedPipeline>,
    config: PipelineConfigPayload,
) -> Result<(), CommandError> {
    use std::collections::HashMap;

    let mut retry_config = crate::stt::RetryConfig::default();
    if let Some(max_retries) = config.max_retries {
        retry_config.max_retries = max_retries;
    }

    // Build VAD config from payload if any VAD settings provided
    let vad_config = VadAutoStopConfig {
        enabled: config.vad_enabled.unwrap_or(false),
        auto_stop: config.vad_auto_stop.unwrap_or(false),
        ..VadAutoStopConfig::default()
    };

    let mut new_config = PipelineConfig::default();
    new_config.stt_provider = config.stt_provider.unwrap_or_else(|| "groq".to_string());
    new_config.stt_api_key = config.stt_api_key.unwrap_or_default();
    new_config.stt_api_keys = HashMap::new();
    new_config.stt_model = config.stt_model;
    new_config.max_duration_secs = config.max_duration_secs.unwrap_or(300.0);
    new_config.retry_config = retry_config;
    new_config.vad_config = vad_config;
    new_config.transcription_timeout = config
        .transcription_timeout_secs
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(60));
    new_config.max_recording_bytes = config.max_recording_bytes.unwrap_or(50 * 1024 * 1024);
    new_config.llm_config = crate::llm::LlmConfig::default();
    new_config.llm_api_keys = HashMap::new();

    pipeline.update_config(new_config).map_err(CommandError::from)?;
    log::info!("Pipeline configuration updated");

    Ok(())
}

/// Stop recording, transcribe, and type the result
/// This is the main end-to-end command for voice dictation
#[tauri::command]
pub async fn pipeline_dictate(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
) -> Result<String, CommandError> {
    // Ensure Escape-to-cancel remains available while we transcribe.
    #[cfg(desktop)]
    crate::set_escape_cancel_shortcut_enabled(&app, true);

    // Log transcription start
    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        log_store.with_current(|log| {
            log.info("Recording stopped, starting transcription");
        });
    }

    // Emit transcription started *only if* the pipeline actually enters the
    // Transcribing state (avoid flashing "TRANSCRIBING..." on quiet-audio skips).
    {
        let app_clone = app.clone();
        let pipeline_clone = pipeline.inner().clone();
        tauri::async_runtime::spawn(async move {
            let start = Instant::now();
            loop {
                match pipeline_clone.state() {
                    PipelineState::Transcribing | PipelineState::Rewriting => {
                        let _ = app_clone.emit("pipeline-transcription-started", ());
                        break;
                    }
                    PipelineState::Idle | PipelineState::Error => {
                        break;
                    }
                    PipelineState::Recording => {}
                }

                if start.elapsed() > Duration::from_secs(2) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        });
    }

    let result = match pipeline.stop_and_transcribe_detailed().await {
        Ok(r) => r,
        Err(PipelineError::Cancelled) => {
            #[cfg(desktop)]
            crate::set_escape_cancel_shortcut_enabled(&app, false);
            let _ = app.emit("pipeline-cancelled", ());
            return Ok(String::new());
        }
        Err(e) => {
            #[cfg(desktop)]
            crate::set_escape_cancel_shortcut_enabled(&app, false);

            if let Some(log_store) = app.try_state::<RequestLogStore>() {
                log_store.with_current(|log| {
                    log.error(format!("Transcription failed: {}", e));
                    log.complete_error(e.to_string());
                });
                log_store.complete_current();
            }
            return Err(CommandError::from(e));
        }
    };

    let final_text = result.final_text.clone();

    // Emit transcript ready event
    let _ = app.emit("pipeline-transcript-ready", &final_text);

    // Type the transcript
    if !final_text.is_empty() {
        if let Some(log_store) = app.try_state::<RequestLogStore>() {
            log_store.with_current(|log| {
                log.info("Typing transcript...");
            });
        }

        crate::commands::text::type_text(app.clone(), final_text.clone())
            .await
            .map_err(|e| {
                if let Some(log_store) = app.try_state::<RequestLogStore>() {
                    log_store.with_current(|log| {
                        log.error(format!("Failed to type text: {}", e));
                    });
                }
                CommandError::from(e)
            })?;
    }

    // Log success
    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        log_store.with_current(|log| {
            log.raw_transcript = Some(result.stt_text.clone());
            log.formatted_transcript = Some(result.final_text.clone());
            log.stt_duration_ms = Some(result.stt_duration_ms);
            log.llm_duration_ms = result.llm_duration_ms;

            log.info(format!(
                "STT completed in {}ms ({} chars)",
                result.stt_duration_ms,
                result.stt_text.len()
            ));

            match &result.llm_outcome {
                LlmOutcome::NotAttempted => {
                    log.info("LLM formatting not attempted (disabled or unavailable)");
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
                    log.warn(format!(
                        "LLM formatting failed; fell back to STT transcript ({})",
                        err
                    ));
                }
            }

            log.complete_success();
        });
        log_store.complete_current();
    }

    #[cfg(desktop)]
    crate::set_escape_cancel_shortcut_enabled(&app, false);

    Ok(final_text)
}

/// Test transcription using the last captured audio (WAV bytes).
///
/// This is primarily used by the settings UI to validate STT provider/model.
#[tauri::command]
pub async fn pipeline_test_transcribe_last_audio(
    pipeline: State<'_, SharedPipeline>,
    profile_id: Option<String>,
) -> Result<String, CommandError> {
    pipeline
        .transcribe_last_audio_for_profile(profile_id.as_deref())
        .await
        .map_err(CommandError::from)
}

/// Whether there is a previously captured audio buffer available for STT testing.
#[tauri::command]
pub fn pipeline_has_last_audio(pipeline: State<'_, SharedPipeline>) -> Result<bool, CommandError> {
    Ok(pipeline.has_last_audio())
}

/// Full pipeline helper: Start recording if not recording, or stop and transcribe if recording
#[tauri::command]
pub async fn pipeline_toggle(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
) -> Result<String, CommandError> {
    if pipeline.is_recording() {
        pipeline_dictate(app, pipeline).await
    } else {
        // Try to start the pipeline FIRST - don't create a log if it fails
        pipeline.start_recording().map_err(|e| {
            log::warn!("Toggle: Failed to start recording: {}", e);
            CommandError::from(e)
        })?;

        #[cfg(desktop)]
        crate::set_escape_cancel_shortcut_enabled(&app, true);

        // Pipeline started successfully - now create the request log
        if let Some(log_store) = app.try_state::<RequestLogStore>() {
            let config = pipeline.config();
            log_store.start_request(
                config.stt_provider.clone(),
                config.stt_model.clone(),
            );
            log_store.with_current(|log| {
                log.llm_provider = if config.llm_config.enabled {
                    Some(config.llm_config.provider.clone())
                } else {
                    None
                };
                log.llm_model = config.llm_config.model.clone();
                log.info("Recording started (toggle)");
            });
        }

        let _ = app.emit("pipeline-recording-started", ());
        Ok(String::new())
    }
}

/// Check if the pipeline is in an error state
#[tauri::command]
pub fn pipeline_is_error(
    pipeline: State<'_, SharedPipeline>,
) -> Result<bool, CommandError> {
    Ok(pipeline.is_error())
}

/// Force reset the pipeline state to Idle
/// Use this to recover from stuck states
#[tauri::command]
pub fn pipeline_force_reset(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
) -> Result<(), CommandError> {
    pipeline.force_reset();
    log::info!("Pipeline force reset to Idle state");

    #[cfg(desktop)]
    crate::set_escape_cancel_shortcut_enabled(&app, false);

    // Emit reset event
    let _ = app.emit("pipeline-reset", ());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_error_from_string() {
        let error = CommandError::from("test error".to_string());
        assert_eq!(error.message, "test error");
    }
}
