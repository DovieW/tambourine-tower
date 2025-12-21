//! Tauri commands for the recording pipeline.
//!
//! These commands expose the recording pipeline functionality to the frontend,
//! enabling voice dictation directly from the Tauri app.

use crate::audio_capture::VadAutoStopConfig;
use crate::pipeline::{LlmOutcome, PipelineConfig, PipelineError, PipelineState, SharedPipeline};
use crate::request_log::RequestLogStore;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

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
    // Emit transcription started event
    let _ = app.emit("pipeline-transcription-started", ());

    // Log transcription start
    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        log_store.with_current(|log| {
            log.info("Recording stopped, starting transcription");
        });
    }

    let result = pipeline
        .stop_and_transcribe_detailed()
        .await
        .map_err(|e| {
            if let Some(log_store) = app.try_state::<RequestLogStore>() {
                log_store.with_current(|log| {
                    log.error(format!("Transcription failed: {}", e));
                    log.complete_error(e.to_string());
                });
                log_store.complete_current();
            }
            CommandError::from(e)
        })?;

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

    // Emit transcript ready event
    let _ = app.emit("pipeline-transcript-ready", &final_text);

    Ok(final_text)
}

/// Cancel the current recording/transcription
#[tauri::command]
pub fn pipeline_cancel(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
) -> Result<(), CommandError> {
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
    // Log transcription start
    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        log_store.with_current(|log| {
            log.info("Recording stopped, starting transcription");
        });
    }

    // Stop and transcribe
    let _ = app.emit("pipeline-transcription-started", ());

    let result = pipeline
        .stop_and_transcribe_detailed()
        .await
        .map_err(|e| {
            if let Some(log_store) = app.try_state::<RequestLogStore>() {
                log_store.with_current(|log| {
                    log.error(format!("Transcription failed: {}", e));
                    log.complete_error(e.to_string());
                });
                log_store.complete_current();
            }
            CommandError::from(e)
        })?;

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

    Ok(final_text)
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
