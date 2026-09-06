//! Tauri commands for the recording pipeline.
//!
//! These commands expose the recording pipeline functionality to the frontend,
//! enabling voice dictation directly from the Tauri app.

use crate::audio_capture::{AudioCaptureDiagnostics, VadAutoStopConfig};
use crate::commands::event_sink::AppEventSink;
use crate::commands::history::{get_history_max_entries, get_max_saved_recordings};
use crate::commands::recording_lifecycle;
use crate::commands::CommandError;
use crate::events;
use crate::history::{HistoryStorage, RequestHistoryUpdate};
use crate::history_request_lifecycle;
use crate::pipeline::{
    program_basename_for_log, resolve_profile_by_id, resolve_profile_for_foreground_app,
    PipelineConfig, PipelineError, PipelineState, SharedPipeline,
};
use crate::recording_completion;
use crate::recording_request_initialization::{
    record_request_id_on_current_span, start_request_log_with_seed, HistorySelectionMode,
    LogLlmSeedMode, RecordingRequestSeed,
};
use crate::recordings::options::{RecordingMode, RecordingPreferences};
use crate::recordings::{RecordingStore, RecordingsStats};
use crate::request_log::RequestLogStore;
use crate::sessions::{recording_finalization, retention};
use crate::stats::{self, EventStatus};
use crate::PipelineStateEvent;
use schemars::JsonSchema;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use tracing::Instrument;
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

    let path = store
        .wav_path_if_exists(&request_id)
        .map_err(CommandError::from)?;
    path.map(|p| {
        let canonical = p
            .canonicalize()
            .map_err(|_| CommandError::from("Recording unavailable"))?;
        let directory = store
            .directory()
            .canonicalize()
            .map_err(|_| CommandError::from("Recording unavailable"))?;
        if canonical.parent() != Some(directory.as_path()) {
            return Err(CommandError::from(
                "Recording path is outside the audio store",
            ));
        }
        app.asset_protocol_scope()
            .allow_file(&canonical)
            .map_err(|_| CommandError::from("Could not allow recording playback"))?;
        Ok(canonical.to_string_lossy().into_owned())
    })
    .transpose()
}

/// Generate/cache a bounded local waveform off the webview and async runtime.
#[tauri::command]
pub async fn recording_get_waveform(
    app: AppHandle,
    request_id: String,
) -> Result<Option<crate::recordings::RecordingWaveform>, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app
            .try_state::<RecordingStore>()
            .ok_or_else(|| CommandError::from("Recording store not available"))?;
        store.waveform(&request_id).map_err(CommandError::from)
    })
    .await
    .map_err(|_| CommandError::from("Waveform analysis interrupted"))?
}

/// Returns the number of `.wav` files deleted.
#[tauri::command]
pub fn recordings_delete_all(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
) -> Result<u64, CommandError> {
    if pipeline.is_recovering() || !pipeline.state().can_start_recording() {
        return Err(CommandError::from(
            "Stop recording or transcription before deleting audio".to_string(),
        ));
    }
    let recovery_ids = recording_list_recovery(app.clone(), pipeline)?;
    let store = app
        .try_state::<RecordingStore>()
        .ok_or_else(|| CommandError::from("Recording store not available".to_string()))?;

    let mut deleted = store.delete_all_wavs().map_err(CommandError::from)?;
    for id in recovery_ids {
        let path = recovery_file(&app, &id)?;
        remove_recovery_files(&path)?;
        deleted += 1;
    }
    Ok(deleted)
}

#[tauri::command]
pub fn recordings_open_folder(app: AppHandle) -> Result<(), CommandError> {
    let store = app
        .try_state::<RecordingStore>()
        .ok_or_else(|| CommandError::from("Recording store not available".to_string()))?;

    open::that(store.directory())
        .map_err(|e| CommandError::from(format!("Failed to open recordings folder: {}", e)))
}

#[tauri::command]
pub fn recordings_get_storage_bytes(app: AppHandle) -> Result<u64, CommandError> {
    recordings_get_stats(app).map(|stats| stats.bytes)
}

#[tauri::command]
pub fn recordings_get_stats(app: AppHandle) -> Result<RecordingsStats, CommandError> {
    let store = app
        .try_state::<RecordingStore>()
        .ok_or_else(|| CommandError::from("Recording store not available".to_string()))?;

    let mut stats = store.stats().map_err(CommandError::from)?;
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|e| CommandError::from(e.to_string()))?
        .join("meeting-recovery");
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(stats),
        Err(error) => return Err(CommandError::from(error.to_string())),
    };
    for entry in entries {
        let entry = entry.map_err(|e| CommandError::from(e.to_string()))?;
        if !entry
            .file_type()
            .map_err(|e| CommandError::from(e.to_string()))?
            .is_file()
        {
            continue;
        }
        let path = entry.path();
        let extension = path.extension().and_then(|x| x.to_str());
        if !matches!(extension, Some("pcm" | "progress" | "transcripts")) {
            continue;
        }
        if !path
            .file_stem()
            .and_then(|x| x.to_str())
            .is_some_and(|x| uuid::Uuid::parse_str(x).is_ok())
        {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(CommandError::from(error.to_string())),
        };
        stats.bytes = stats.bytes.saturating_add(metadata.len());
        if extension == Some("pcm") {
            stats.count += 1;
        }
    }
    Ok(stats)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ActiveProfileInfo {
    pub profile_id: Option<String>,
    pub profile_name: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionPresetLockInfo {
    pub profile_id: Option<String>,
    pub preset_id: Option<String>,
}

/// Resolve the currently active program profile based on the foreground application.
///
/// This is used by the overlay to show per-program UX (e.g. a one-shot preset lock)
/// without re-implementing OS-specific process detection in the frontend.
#[tauri::command]
pub fn pipeline_get_active_profile_for_foreground_app(
    pipeline: State<'_, SharedPipeline>,
) -> Result<ActiveProfileInfo, CommandError> {
    let config = pipeline.config();
    let (profile_id, profile_name) = resolve_profile_for_foreground_app(&config);
    Ok(ActiveProfileInfo {
        profile_id,
        profile_name,
    })
}

/// Set (or clear) a one-shot, non-persisted preset override.
///
/// The next transcription will prefer this preset over any persisted manual override
/// and over intent routing.
#[tauri::command]
pub fn pipeline_set_session_preset_lock(
    pipeline: State<'_, SharedPipeline>,
    profile_id: Option<String>,
    preset_id: Option<String>,
) -> Result<(), CommandError> {
    pipeline
        .set_session_preset_lock(profile_id, preset_id)
        .map_err(CommandError::from)
}

/// Read the current in-memory session preset lock (without clearing it).
#[tauri::command]
pub fn pipeline_get_session_preset_lock(
    pipeline: State<'_, SharedPipeline>,
) -> Result<SessionPresetLockInfo, CommandError> {
    let lock = pipeline.peek_session_preset_lock();
    Ok(match lock {
        Some((profile_id, preset_id)) => SessionPresetLockInfo {
            profile_id,
            preset_id: Some(preset_id),
        },
        None => SessionPresetLockInfo {
            profile_id: None,
            preset_id: None,
        },
    })
}

/// Start recording audio using the pipeline
#[tauri::command]
pub fn recording_computer_audio_available() -> bool {
    crate::audio_capture::computer_audio::available()
}

#[tauri::command]
pub fn recording_get_preferences(app: AppHandle) -> Result<RecordingPreferences, CommandError> {
    let store = crate::settings::store::get_fresh_settings_store(&app)
        .ok_or_else(|| CommandError::from("Settings unavailable"))?;
    let value = store.get("recording_preferences");
    let options: RecordingPreferences = value
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| CommandError::from("Recording preferences could not be read"))?
        .unwrap_or_default();
    options.validate().map_err(CommandError::from)?;
    Ok(options)
}

#[tauri::command]
pub fn recording_set_preferences(
    app: AppHandle,
    preferences: RecordingPreferences,
) -> Result<(), CommandError> {
    preferences.validate().map_err(CommandError::from)?;
    let store = crate::settings::store::get_fresh_settings_store(&app)
        .ok_or_else(|| CommandError::from("Settings unavailable"))?;
    store.set(
        "recording_preferences",
        serde_json::to_value(preferences)
            .map_err(|_| CommandError::from("Invalid recording preferences"))?,
    );
    store
        .save()
        .map_err(|_| CommandError::from("Could not save recording preferences"))
}

fn recovery_file(app: &AppHandle, id: &str) -> Result<std::path::PathBuf, CommandError> {
    let id = uuid::Uuid::parse_str(id)
        .map_err(|_| CommandError::from("Invalid recovery id".to_string()))?;
    let path = app
        .path()
        .app_data_dir()
        .map_err(|e| CommandError::from(e.to_string()))?
        .join("meeting-recovery")
        .join(format!("{id}.pcm"));
    if !std::fs::symlink_metadata(&path)
        .map_err(|e| CommandError::from(e.to_string()))?
        .file_type()
        .is_file()
    {
        return Err(CommandError::from("Invalid recovery file".to_string()));
    }
    Ok(path)
}

fn remove_recovery_files(path: &std::path::Path) -> Result<(), CommandError> {
    crate::audio_capture::journal::discard(path)
        .map_err(|error| CommandError::from(error.to_string()))
}

#[tauri::command]
pub fn recording_list_recovery(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
) -> Result<Vec<String>, CommandError> {
    if pipeline.state() == PipelineState::Recording || pipeline.is_recovering() {
        return Ok(vec![]);
    }
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|e| CommandError::from(e.to_string()))?
        .join("meeting-recovery");
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(CommandError::from(e.to_string())),
    };
    let mut ids: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            let path = entry.path();
            if path.extension()?.to_str()? != "pcm" {
                return None;
            }
            let id = path.file_stem()?.to_str()?;
            uuid::Uuid::parse_str(id).ok().map(|id| id.to_string())
        })
        .collect();
    ids.sort();
    Ok(ids)
}

#[tauri::command]
pub async fn recording_recover(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
    id: String,
) -> Result<(), CommandError> {
    recover_recording_inner(app, pipeline.inner().clone(), id).await
}

async fn recover_recording_inner(
    app: AppHandle,
    pipeline: SharedPipeline,
    id: String,
) -> Result<(), CommandError> {
    let cancel = pipeline.begin_recovery().map_err(CommandError::from)?;
    struct RecoveryGuard(SharedPipeline);
    impl Drop for RecoveryGuard {
        fn drop(&mut self) {
            self.0.end_recovery();
        }
    }
    let _guard = RecoveryGuard(pipeline.clone());
    let path = recovery_file(&app, &id)?;
    let options = RecordingPreferences::load_journal(&path).map_err(CommandError::from)?;
    let recording_id = format!("{id}-final");
    // A successful History commit is the completion marker. If the process
    // crashes before journal deletion, recovering must not submit it again.
    let completed = || -> Result<bool, CommandError> {
        Ok(app
            .state::<HistoryStorage>()
            .get_all(None)
            .map_err(CommandError::from)?
            .iter()
            .any(|entry| {
                entry.recording_request_id.as_deref() == Some(recording_id.as_str())
                    && entry.status == crate::history::HistoryStatus::Success
            }))
    };
    if !completed()? {
        let export_path = path.clone();
        let export_cancel = cancel.clone();
        let export_app = app.clone();
        let export_id = recording_id.clone();
        // Prepare one playback WAV off the async runtime, only after capture
        // ends. STT uploads split later; never replace or truncate the journal.
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let wav = crate::audio_capture::journal::final_wav(&export_path, 0, || {
                export_cancel.is_cancelled()
            })
            .map_err(|e| e.to_string())?;
            let store = export_app.state::<RecordingStore>();
            store.save_options(&export_id, &options)?;
            store.save_wav(&export_id, &wav)
        })
        .await
        .map_err(|_| {
            CommandError::from("Audio preparation failed; your audio is saved".to_string())
        })?
        .map_err(CommandError::from)?;
        if cancel.is_cancelled() {
            return Err(CommandError::from(
                "Transcription cancelled. Your audio is saved for recovery.".to_string(),
            ));
        }
        retry_transcription_inner(app.clone(), pipeline.clone(), recording_id.clone(), true)
            .await?;
        if !completed()? {
            return Err(CommandError::from(
                "The final transcript could not be saved. Your audio is retained.".to_string(),
            ));
        }
    }
    remove_recovery_files(&path)?;
    Ok(())
}

#[tauri::command]
pub fn recording_discard_recovery(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
    id: String,
) -> Result<(), CommandError> {
    if pipeline.is_recovering() || !pipeline.state().can_start_recording() {
        return Err(CommandError::from("Recording pipeline is busy".to_string()));
    }
    let path = recovery_file(&app, &id)?;
    remove_recovery_files(&path)?;
    Ok(())
}

/// Start recording audio using the pipeline
#[tauri::command]
pub fn pipeline_start_recording(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
    history_only: Option<bool>,
    computer_audio: Option<bool>,
) -> Result<(), CommandError> {
    if pipeline.is_recovering() || !pipeline.state().can_start_recording() {
        return Err(CommandError::from("Recording pipeline is busy".to_string()));
    }
    let preferences = if history_only.unwrap_or(false) {
        recording_get_preferences(app.clone())?
    } else {
        RecordingPreferences::default()
    };
    if preferences.mode == RecordingMode::Meeting && preferences.meeting_model.is_none() {
        return Err(CommandError::from(
            "Choose a meeting model in Recording options first",
        ));
    }
    let recovery_path = if history_only.unwrap_or(false) {
        let directory = app
            .path()
            .app_data_dir()
            .map_err(|e| CommandError::from(e.to_string()))?
            .join("meeting-recovery");
        std::fs::create_dir_all(&directory).map_err(|e| CommandError::from(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| CommandError::from(e.to_string()))?;
        }
        let path = directory.join(format!("{}.pcm", uuid::Uuid::new_v4()));
        preferences
            .save_journal(&path)
            .map_err(CommandError::from)?;
        Some(path)
    } else {
        None
    };
    let span = tracing::info_span!(
        "pipeline_start_recording",
        request_id = tracing::field::Empty
    );
    let _span_guard = span.enter();

    // Resolve the profile immediately (best-effort) while the user is likely still
    // in the target app, then pin it for this recording session so stop/transcribe
    // isn't impacted by focus stealing from our always-on-top windows.
    //
    // IMPORTANT: Only pin a *real* program profile id. Do NOT pin explicit "default",
    // because that would force the whole request to Default even if matching could
    // have succeeded later.
    let config = pipeline.config();

    #[cfg(desktop)]
    let foreground = crate::windows_apps::get_foreground_process_path();
    #[cfg(not(desktop))]
    let foreground: Option<String> = None;

    let matched_profile = crate::pipeline::select_profile_for_foreground_app(&config.llm_config);
    if pipeline.peek_session_profile_override().is_none() {
        if let Some(p) = matched_profile.as_ref() {
            let _ = pipeline.set_session_profile_override(Some(p.id.clone()));
        } else {
            let _ = pipeline.set_session_profile_override(None);
        }
    }

    // One-time per recording: log what we saw so we can debug "always Default" reports.
    let foreground_log = foreground.as_deref().map(program_basename_for_log);
    log::info!(
        "[profile] start_recording foreground={:?} profiles={} matched={}",
        foreground_log,
        config.llm_config.program_prompt_profiles.len(),
        matched_profile
            .as_ref()
            .map(|p| format!("{} ({})", p.name, p.id))
            .unwrap_or_else(|| "<none>".to_string())
    );

    // Preserve existing semantics for UI chips/logs:
    // - foreground unknown -> None
    // - foreground known but no match -> Default
    // - match -> profile
    let (profile_id, profile_name) = if foreground.is_none() {
        (None, None)
    } else if let Some(p) = matched_profile.as_ref() {
        (Some(p.id.clone()), Some(p.name.clone()))
    } else {
        (Some("default".to_string()), Some("Default".to_string()))
    };

    let request_seed = RecordingRequestSeed::from_config(&config)
        .with_profile(profile_id.clone(), profile_name.clone());

    // Start request logging.
    let request_log_store = app.try_state::<RequestLogStore>();
    let request_id = recording_lifecycle::start_recording_request(
        request_log_store.as_ref().map(|state| state.inner()),
        &request_seed,
        LogLlmSeedMode::PreserveConfigured,
        |log| {
            log.info("Recording started");
        },
    );

    // Bind OCR to this request id so OCR survives internal pipeline transitions and cannot
    // leak across requests.
    if let Some(id) = request_id.clone() {
        pipeline.begin_ocr_session(id);
    }

    pipeline
        .start_recording_with_output(
            history_only.unwrap_or(false),
            recovery_path.clone(),
            preferences.mode == RecordingMode::Meeting && computer_audio.unwrap_or(false),
        )
        .map_err(|e| {
            if let Some(path) = &recovery_path {
                // A capture startup failure can still leave recoverable audio.
                // Never discard the mode/model ownership while that journal exists.
                if !path.exists() {
                    let _ = std::fs::remove_file(path.with_extension("options.json"));
                }
            }
            // If we fail to start, clear any pinned session profile so it doesn't leak.
            let _ = pipeline.set_session_profile_override(None);

            if let Some(log_store) = app.try_state::<RequestLogStore>() {
                log_store.with_current(|log| {
                    log.error(format!("Failed to start recording: {}", e));
                    log.complete_error(e.to_string());
                });
            }

            recording_finalization::complete_current_request_without_cost(
                &app,
                pipeline.inner(),
                request_id.as_deref(),
            );
            CommandError::from(e)
        })?;

    // While recording/transcribing, allow Escape to cancel without triggering transcription.
    #[cfg(desktop)]
    crate::set_escape_cancel_shortcut_enabled(&app, true);

    // Emit event to frontend
    recording_completion::emit_pipeline_recording_started(&AppEventSink(&app));

    if history_only.unwrap_or(false) {
        let monitor_pipeline = pipeline.inner().clone();
        let monitor_app = app.clone();
        let session_path = pipeline.recovery_path();
        tauri::async_runtime::spawn(async move {
            while monitor_pipeline.state() == PipelineState::Recording
                && monitor_pipeline.recovery_path() == session_path
            {
                if let Err(error) = monitor_pipeline.recording_progress() {
                    recording_completion::emit_pipeline_error(
                        &monitor_app,
                        &error.to_string(),
                        None,
                    );
                    #[cfg(desktop)]
                    crate::set_escape_cancel_shortcut_enabled(&monitor_app, false);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        });
    }

    Ok(())
}

/// Stop recording and transcribe the audio
#[tauri::command]
pub async fn pipeline_stop_and_transcribe(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
) -> Result<String, CommandError> {
    let span = tracing::info_span!(
        "pipeline_stop_and_transcribe",
        request_id = tracing::field::Empty
    );
    pipeline_stop_and_transcribe_inner(app, pipeline)
        .instrument(span)
        .await
}

async fn pipeline_stop_and_transcribe_inner(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
) -> Result<String, CommandError> {
    if pipeline.is_history_only_recording() {
        let path = pipeline
            .recovery_path()
            .ok_or_else(|| CommandError::from("Recovery audio is unavailable".to_string()))?;
        pipeline.stop_recording().map_err(CommandError::from)?;
        if let Some(log_store) = app.try_state::<RequestLogStore>() {
            log_store.with_current(|log| {
                log.info("Meeting audio saved locally for transcription");
                log.complete_success();
            });
            log_store.complete_current();
        }
        let id = path
            .file_stem()
            .and_then(|id| id.to_str())
            .ok_or_else(|| CommandError::from("Invalid recovery id".to_string()))?
            .to_string();
        recover_recording_inner(app, pipeline.inner().clone(), id).await?;
        return Ok(String::new());
    }
    let max_saved_recordings = get_max_saved_recordings(&app);
    let max_history_entries = get_history_max_entries(&app);

    // Ensure Escape-to-cancel is available during the transcription phase.
    #[cfg(desktop)]
    crate::set_escape_cancel_shortcut_enabled(&app, true);

    let config = pipeline.config();
    let (profile_id, profile_name) = resolve_profile_for_foreground_app(&config);
    let request_seed = RecordingRequestSeed::from_config(&config)
        .with_profile(profile_id.clone(), profile_name.clone());

    // Try to capture the active request id for history + persistent audio.
    //
    // In some edge cases (e.g., backend-initiated recordings or unexpected state
    // resets), request logging may not have been started at recording-start.
    // For UX consistency, ensure we still create a request log + history entry.
    let request_log_store = app.try_state::<RequestLogStore>();
    let active_request = recording_lifecycle::ensure_current_transcription_request(
        request_log_store.as_ref().map(|state| state.inner()),
        &request_seed,
        HistorySelectionMode::OmitSeededSelection,
        max_history_entries,
        "Request log was missing at stop; started a new request log entry",
    );
    let active_request_id = active_request.request_id.clone();

    // Bind OCR to this request id so OCR survives internal pipeline transitions and cannot leak
    // across requests. This also handles edge cases where request logging begins at stop rather
    // than recording-start.
    active_request.bind_ocr_session_for_transcription(pipeline.inner(), &config.ocr_config);
    active_request.apply_history_updates(&app);
    active_request.spawn_watchers(
        app.clone(),
        pipeline.inner().clone(),
        crate::recording_orchestration::RecordingPhaseWatcherBundle::StopAndTranscribe,
    );

    // Log transcription start
    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        log_store.with_current(|log| {
            request_seed.seed_log(log, LogLlmSeedMode::LeaveExisting);
            // Important: do not include recording time in the request "Total" duration.
            // The request log is created at recording-start, so we explicitly mark the
            // processing start when transcription begins.
            log.mark_processing_started();
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
            }

            recording_finalization::complete_current_request_without_cost(
                &app,
                pipeline.inner(),
                active_request_id.as_deref(),
            );

            // Best-effort: remove the in-progress history entry so it doesn't linger as
            // "Transcribing..." forever.
            if let Some(req_id) = active_request_id.as_deref() {
                let _ = history_request_lifecycle::apply_request_history_update(
                    &app,
                    RequestHistoryUpdate::Delete {
                        request_id: req_id.to_string(),
                    },
                );
            }

            recording_completion::emit_cancelled(&app);
            return Ok(String::new());
        }
        Err(e) => {
            #[cfg(desktop)]
            crate::set_escape_cancel_shortcut_enabled(&app, false);

            let wav_bytes = pipeline.clone_last_wav_bytes();

            if let Some(log_store) = app.try_state::<RequestLogStore>() {
                log_store.with_current(|log| {
                    log.error_with_details(
                        format!("Transcription failed: {}", e),
                        crate::request_log::format_error_chain(&e),
                    );
                    log.complete_error(e.to_string());
                });
            }

            let _ = history_request_lifecycle::sync_request_profile_from_current_log(
                &app,
                active_request_id.as_deref(),
            );

            recording_finalization::complete_current_request_with_cost(
                &app,
                pipeline.inner(),
                active_request_id.as_deref(),
                EventStatus::Error,
                wav_bytes.as_deref(),
            );

            // Update history entry with error (keep it visible for retry)
            if let Some(req_id) = active_request_id.as_deref() {
                let _ = history_request_lifecycle::apply_request_history_update(
                    &app,
                    RequestHistoryUpdate::CompleteError {
                        request_id: req_id.to_string(),
                        error_message: e.to_string(),
                    },
                );
            }

            // Time-based retention (best-effort). Runs only after a transcription attempt.
            retention::apply_transcription_retention(&app);

            // Persist audio for retry (best-effort)
            if let Err(err) = recording_completion::persist_request_recording(
                &app,
                active_request_id.as_deref(),
                wav_bytes.as_deref(),
                max_saved_recordings,
            ) {
                log::warn!("{}", err);
            }

            // Emit pipeline-error event with request_id so the overlay can show a retry button.
            recording_completion::emit_pipeline_error(
                &app,
                &e.to_string(),
                active_request_id.as_deref(),
            );

            let mut error = CommandError::from(e);
            if let Some(req_id) = active_request_id.clone() {
                error = error.with_request_id(req_id);
            }
            return Err(error);
        }
    };

    let final_text = result.final_text.clone();

    // Capture WAV bytes once (used for duration + retry persistence + cost).
    let wav_bytes = pipeline.clone_last_wav_bytes();
    let audio_secs_from_wav = wav_bytes.as_deref().and_then(stats::wav_duration_secs);
    let audio_size_bytes = wav_bytes.as_ref().map(|v| v.len());

    // Log success
    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        log_store.with_current(|log| {
            recording_finalization::record_transcription_success(
                log,
                recording_finalization::TranscriptionSuccessLogUpdate {
                    result: &result,
                    formatted_transcript: Some(result.final_text.as_str()),
                    audio_duration_secs: audio_secs_from_wav,
                    audio_size_bytes,
                    stt_summary_label: "STT",
                    completion_log_message: None,
                    warn_if_no_formatted_transcript: false,
                },
            );
            log.complete_success();
        });
    }

    let _ = history_request_lifecycle::sync_request_profile_from_current_log(
        &app,
        active_request_id.as_deref(),
    );

    // The finalization Module keeps log closure/cost/OCR ordering consistent across command flows.
    recording_finalization::persist_current_request_preset_to_history(
        &app,
        active_request_id.as_deref(),
    );
    recording_finalization::persist_history_llm_metadata(
        &app,
        active_request_id.as_deref(),
        &result,
    );
    recording_finalization::complete_current_request_with_cost(
        &app,
        pipeline.inner(),
        active_request_id.as_deref(),
        EventStatus::Success,
        wav_bytes.as_deref(),
    );

    // Persist audio for retry (best-effort)
    if let Err(err) = recording_completion::persist_request_recording(
        &app,
        active_request_id.as_deref(),
        wav_bytes.as_deref(),
        max_saved_recordings,
    ) {
        log::warn!("{}", err);
    }

    // Update history entry with success text
    if let Some(req_id) = active_request_id.as_deref() {
        history_request_lifecycle::apply_request_history_update(
            &app,
            RequestHistoryUpdate::CompleteSuccess {
                request_id: req_id.to_string(),
                text: final_text.clone(),
            },
        )
        .map_err(CommandError::from)?;
    }

    // Time-based retention (best-effort). Runs only after a transcription attempt.
    retention::apply_transcription_retention(&app);

    // Emit transcript ready event
    recording_completion::emit_transcript_ready(&app, &final_text);

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
    let span = tracing::info_span!(
        "pipeline_retry_transcription",
        retry_from_request_id = %request_id,
        request_id = tracing::field::Empty
    );
    pipeline_retry_transcription_impl(app, pipeline.inner().clone(), request_id)
        .instrument(span)
        .await
}

/// Implementation for retry transcription that can be called both from the Tauri command
/// and from internal shortcut handlers.
pub(crate) async fn pipeline_retry_transcription_impl(
    app: AppHandle,
    pipeline: SharedPipeline,
    request_id: String,
) -> Result<String, CommandError> {
    retry_transcription_inner(app, pipeline, request_id, false).await
}

async fn retry_transcription_inner(
    app: AppHandle,
    pipeline: SharedPipeline,
    request_id: String,
    recovery: bool,
) -> Result<String, CommandError> {
    if !recovery && pipeline.is_recovering() {
        return Err(CommandError::from(
            "A meeting transcription is already running".to_string(),
        ));
    }
    let max_history_entries = get_history_max_entries(&app);

    // Allow Escape-to-cancel while the retry transcription is running.
    #[cfg(desktop)]
    crate::set_escape_cancel_shortcut_enabled(&app, true);

    let recording_store = app
        .try_state::<RecordingStore>()
        .ok_or_else(|| CommandError::from("Recording store not available".to_string()))?;

    let original_entry = app
        .try_state::<HistoryStorage>()
        .and_then(|history| history.get_by_id(&request_id).ok().flatten());

    // Resolve which request id actually owns the recording.
    // - For normal requests, this is the same as `request_id`.
    // - For reruns (including failed reruns), the entry should point back to the original.
    let recording_source_id: String = original_entry
        .as_ref()
        .and_then(|entry| entry.recording_request_id.clone())
        .unwrap_or_else(|| request_id.clone());

    let meeting_id = recording_source_id
        .strip_suffix("-final")
        .filter(|id| uuid::Uuid::parse_str(id).is_ok());
    let recording_options = recording_store
        .options(&recording_source_id)
        .map_err(CommandError::from)?;
    struct MeetingReplayGuard(SharedPipeline);
    impl Drop for MeetingReplayGuard {
        fn drop(&mut self) {
            self.0.end_recovery();
        }
    }
    // History reruns of a completed meeting also use small uploads, but are
    // deliberately fresh attempts rather than silently reusing old text.
    let _meeting_replay = if meeting_id.is_some() && !recovery {
        pipeline.begin_recovery().map_err(CommandError::from)?;
        Some(MeetingReplayGuard(pipeline.clone()))
    } else {
        None
    };

    // Preserve the preset used by the original request (if we can find it).
    // This was added after presets existed, but retry transcription historically did not
    // carry preset selection forward.
    let original_preset_id: Option<String> =
        original_entry.as_ref().and_then(|e| e.preset_id.clone());
    let original_preset_name: Option<String> =
        original_entry.as_ref().and_then(|e| e.preset_name.clone());

    let wav = recording_store
        .load_wav(&recording_source_id)
        .map_err(CommandError::from)?;

    // Start a *new* request log for the retry attempt.
    let config = pipeline.config();
    // Use the same profile as the original request (if we can find it).
    // This avoids retry accidentally using Default just because the foreground app changed.
    let original_profile_id: Option<String> =
        original_entry.as_ref().and_then(|e| e.profile_id.clone());

    // Preserve "unknown" as None so the UI doesn't show a Default chip unless it was
    // explicitly recorded as default on the original entry.
    let (profile_id, profile_name) = if original_profile_id.is_none() {
        (None, None)
    } else {
        resolve_profile_by_id(&config, original_profile_id.as_deref())
    };

    let request_seed = RecordingRequestSeed::from_config(&config)
        .with_profile(profile_id.clone(), profile_name.clone())
        .with_preset(original_preset_id.clone(), original_preset_name.clone());

    let request_log_store = app.try_state::<RequestLogStore>();
    let retry_request = recording_lifecycle::start_retry_transcription_request(
        request_log_store.as_ref().map(|state| state.inner()),
        &request_seed,
        &recording_source_id,
        max_history_entries,
    );
    let new_request_id = retry_request.request_id.clone();

    // Bind OCR to this retry request id so OCR cannot leak across requests.
    retry_request.bind_ocr_session(&pipeline);
    retry_request.apply_history_updates(&app);
    let history_preparation = if let Some(id) = new_request_id.as_deref() {
        app.state::<HistoryStorage>().set_recording_details(
            id,
            recording_options.mode,
            stats::wav_duration_secs(&wav),
            Vec::new(),
            None,
        )
    } else {
        Ok(())
    };

    let _ = app.emit(events::EVENT_PIPELINE_TRANSCRIPTION_STARTED, ());
    let _ = app.emit(
        events::EVENT_PIPELINE_STATE_CHANGED,
        PipelineStateEvent::Transcribing,
    );

    // If we know the original preset, set a one-shot session lock so the retry uses the
    // same preset as the entry being rerun (instead of whatever the profile/router would
    // pick right now).
    //
    // This lock is normally consumed (take+clear) by the pipeline once it reaches routing,
    // but we also defensively clear it on drop so early STT failures don't leak the lock
    // into the next transcription attempt.
    struct ClearPresetLockOnDrop {
        pipeline: SharedPipeline,
    }
    impl Drop for ClearPresetLockOnDrop {
        fn drop(&mut self) {
            let _ = self.pipeline.set_session_preset_lock(None, None);
        }
    }

    let _preset_lock_guard = if original_preset_id.is_some() {
        let _ = pipeline.set_session_preset_lock(profile_id.clone(), original_preset_id.clone());
        Some(ClearPresetLockOnDrop {
            pipeline: pipeline.clone(),
        })
    } else {
        None
    };

    retry_request.spawn_watchers(
        app.clone(),
        pipeline.clone(),
        crate::recording_orchestration::RecordingPhaseWatcherBundle::RetryTranscription,
    );

    // Run the retry transcription (STT + optional LLM)
    let transcription = if let Err(error) = history_preparation {
        Err(PipelineError::Config(error))
    } else if recovery {
        let id = recording_source_id
            .strip_suffix("-final")
            .ok_or_else(|| CommandError::from("Invalid meeting recording id".to_string()))?;
        let checkpoint = recovery_file(&app, id)?.with_extension("transcripts");
        pipeline
            .transcribe_journal_wav(
                wav.clone(),
                profile_id.as_deref(),
                Some(&checkpoint),
                &recording_options,
            )
            .await
    } else if meeting_id.is_some() {
        pipeline
            .transcribe_journal_wav(wav.clone(), profile_id.as_deref(), None, &recording_options)
            .await
    } else {
        pipeline
            .transcribe_wav_bytes_detailed_for_profile(wav.clone(), profile_id.as_deref())
            .await
    };
    // Persist all original output before reporting success or clearing recovery.
    // Storage failures use the same request cleanup/error path as provider failures.
    let transcription = transcription.and_then(|result| {
        if let Some(req_id) = new_request_id.as_deref() {
            app.state::<HistoryStorage>()
                .set_recording_details(
                    req_id,
                    recording_options.mode,
                    stats::wav_duration_secs(&wav),
                    result.speaker_segments.clone(),
                    (!result.speaker_segments.is_empty()).then(|| result.stt_text.clone()),
                )
                .map_err(PipelineError::Config)?;
            history_request_lifecycle::apply_request_history_update(
                &app,
                RequestHistoryUpdate::CompleteSuccess {
                    request_id: req_id.to_string(),
                    text: result.final_text.clone(),
                },
            )
            .map_err(PipelineError::Config)?;
        }
        Ok(result)
    });
    let result = match transcription {
        Ok(r) => r,
        Err(PipelineError::Cancelled) => {
            #[cfg(desktop)]
            crate::set_escape_cancel_shortcut_enabled(&app, false);

            if let Some(log_store) = app.try_state::<RequestLogStore>() {
                log_store.with_current(|log| {
                    log.warn("Retry transcription cancelled by user");
                    log.complete_cancelled();
                });
            }

            recording_finalization::complete_current_request_without_cost(
                &app,
                &pipeline,
                new_request_id.as_deref(),
            );

            recording_completion::emit_cancelled(&app);
            if recovery {
                return Err(CommandError::from(
                    "Transcription cancelled. Your audio is saved for recovery.".to_string(),
                ));
            }
            return Ok(String::new());
        }
        Err(e) => {
            #[cfg(desktop)]
            crate::set_escape_cancel_shortcut_enabled(&app, false);

            if let Some(log_store) = app.try_state::<RequestLogStore>() {
                log_store.with_current(|log| {
                    log.error_with_details(
                        format!("Retry transcription failed: {}", e),
                        crate::request_log::format_error_chain(&e),
                    );
                    log.complete_error(e.to_string());
                });
            }

            let _ = history_request_lifecycle::sync_request_profile_from_current_log(
                &app,
                new_request_id.as_deref(),
            );

            recording_finalization::complete_current_request_with_cost(
                &app,
                &pipeline,
                new_request_id.as_deref(),
                EventStatus::Error,
                Some(wav.as_slice()),
            );

            if let Some(req_id) = new_request_id.as_deref() {
                let _ = history_request_lifecycle::apply_request_history_update(
                    &app,
                    RequestHistoryUpdate::CompleteError {
                        request_id: req_id.to_string(),
                        error_message: e.to_string(),
                    },
                );
            }

            // Also emit pipeline-error so the overlay can present the always-on-top retry UI.
            recording_completion::emit_pipeline_error(
                &app,
                &e.to_string(),
                new_request_id.as_deref(),
            );

            let mut error = CommandError::from(e);
            if let Some(req_id) = new_request_id.clone() {
                error = error.with_request_id(req_id);
            }
            return Err(error);
        }
    };

    // IMPORTANT: Do NOT copy the WAV under the new request id.
    // Reruns always point back to the original recording via `recording_request_id`.

    let final_text = result.final_text.clone();

    // Update log store on success
    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        log_store.with_current(|log| {
            recording_finalization::record_transcription_success(
                log,
                recording_finalization::TranscriptionSuccessLogUpdate {
                    result: &result,
                    formatted_transcript: Some(result.final_text.as_str()),
                    audio_duration_secs: stats::wav_duration_secs(wav.as_slice()),
                    audio_size_bytes: Some(wav.len()),
                    stt_summary_label: "Retry STT",
                    completion_log_message: None,
                    warn_if_no_formatted_transcript: false,
                },
            );
            log.complete_success();
        });
    }

    let _ = history_request_lifecycle::sync_request_profile_from_current_log(
        &app,
        new_request_id.as_deref(),
    );

    recording_finalization::persist_current_request_preset_to_history(
        &app,
        new_request_id.as_deref(),
    );
    recording_finalization::persist_history_llm_metadata(&app, new_request_id.as_deref(), &result);
    recording_finalization::complete_current_request_with_cost(
        &app,
        &pipeline,
        new_request_id.as_deref(),
        EventStatus::Success,
        Some(wav.as_slice()),
    );

    // Emit transcript ready event
    recording_completion::emit_transcript_ready(&app, &final_text);

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
        if pipeline.is_error() {
            return Err(CommandError::from("Capture stopped, but recovery audio could not be discarded. Use the saved-audio controls to try again.".to_string()));
        }
        Ok(())
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
        let _ = app.emit(events::EVENT_PIPELINE_CANCELLED, ());
        let _ = app.emit(
            events::EVENT_PIPELINE_STATE_CHANGED,
            PipelineStateEvent::Idle,
        );

        Ok(())
    }
}

/// Get the current pipeline state
#[tauri::command]
pub fn pipeline_set_recording_paused(
    pipeline: State<'_, SharedPipeline>,
    paused: bool,
) -> Result<(), CommandError> {
    pipeline
        .set_recording_paused(paused)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn pipeline_get_recording_paused(pipeline: State<'_, SharedPipeline>) -> bool {
    pipeline.is_recording_paused()
}

#[tauri::command]
pub fn pipeline_get_recording_seconds(
    pipeline: State<'_, SharedPipeline>,
) -> Result<f64, CommandError> {
    pipeline.recording_progress().map_err(CommandError::from)
}

#[tauri::command]
pub fn pipeline_can_pause_recording(pipeline: State<'_, SharedPipeline>) -> bool {
    pipeline.state() == PipelineState::Recording && pipeline.is_history_only_recording()
}

/// Get the current pipeline state
#[tauri::command]
pub fn pipeline_get_state(pipeline: State<'_, SharedPipeline>) -> Result<String, CommandError> {
    if pipeline.is_recovering() {
        return Ok("transcribing".to_string());
    }
    let state = pipeline.state();
    let state_str = match state {
        PipelineState::Idle => "idle",
        PipelineState::Recording => "recording",
        PipelineState::Routing => "routing",
        PipelineState::Transcribing => "transcribing",
        PipelineState::Rewriting => "rewriting",
        PipelineState::Error => "error",
    };
    Ok(state_str.to_string())
}

/// Check if the pipeline is currently recording
#[tauri::command]
pub fn pipeline_is_recording(pipeline: State<'_, SharedPipeline>) -> Result<bool, CommandError> {
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

    let new_config = PipelineConfig {
        stt_provider: config.stt_provider.unwrap_or_else(|| "groq".to_string()),
        stt_api_key: config.stt_api_key.unwrap_or_default(),
        stt_api_keys: HashMap::new(),
        stt_model: config.stt_model,
        max_duration_secs: config.max_duration_secs.unwrap_or(300.0),
        retry_config,
        vad_config,
        transcription_timeout: config
            .transcription_timeout_secs
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(60)),
        max_recording_bytes: config.max_recording_bytes.unwrap_or(50 * 1024 * 1024),
        llm_config: crate::llm::LlmConfig::default(),
        llm_api_keys: HashMap::new(),
        ..Default::default()
    };

    pipeline
        .update_config(new_config)
        .map_err(CommandError::from)?;
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
    let span = tracing::info_span!("pipeline_dictate", request_id = tracing::field::Empty);
    pipeline_dictate_inner(app, pipeline).instrument(span).await
}

async fn pipeline_dictate_inner(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
) -> Result<String, CommandError> {
    if pipeline.is_history_only_recording() {
        return pipeline_stop_and_transcribe_inner(app, pipeline).await;
    }
    let max_saved_recordings = get_max_saved_recordings(&app);
    let max_history_entries = get_history_max_entries(&app);

    // Ensure Escape-to-cancel remains available while we transcribe.
    #[cfg(desktop)]
    crate::set_escape_cancel_shortcut_enabled(&app, true);

    let cfg = pipeline.config();
    let (profile_id, profile_name) = resolve_profile_for_foreground_app(&cfg);
    let request_seed = RecordingRequestSeed::from_config(&cfg)
        .with_profile(profile_id.clone(), profile_name.clone());

    // Ensure there is a request log. Some flows can reach here without an active
    // recording-start log, so we recover one here for consistent history/log UX.
    let request_log_store = app.try_state::<RequestLogStore>();
    let active_request = recording_lifecycle::ensure_current_transcription_request(
        request_log_store.as_ref().map(|state| state.inner()),
        &request_seed,
        HistorySelectionMode::PreserveSeededSelection,
        max_history_entries,
        "Request log was missing at dictate; started a new request log entry",
    );
    let active_request_id = active_request.request_id.clone();
    active_request.apply_history_updates(&app);

    // Log transcription start
    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        log_store.with_current(|log| {
            request_seed.seed_log(log, LogLlmSeedMode::LeaveExisting);
            // Do not include recording time in the request "Total" duration.
            log.mark_processing_started();
            log.info("Recording stopped, starting transcription");
        });
    }

    active_request.spawn_watchers(
        app.clone(),
        pipeline.inner().clone(),
        crate::recording_orchestration::RecordingPhaseWatcherBundle::Dictate,
    );

    let result = match pipeline.stop_and_transcribe_detailed().await {
        Ok(r) => r,
        Err(PipelineError::Cancelled) => {
            #[cfg(desktop)]
            crate::set_escape_cancel_shortcut_enabled(&app, false);
            recording_completion::emit_cancelled(&app);

            // Best-effort: mark request as cancelled in logs + history.
            if let Some(log_store) = app.try_state::<RequestLogStore>() {
                log_store.with_current(|log| {
                    log.warn("Recording cancelled by user");
                    log.complete_cancelled();
                });
            }

            recording_finalization::complete_current_request_without_cost(
                &app,
                pipeline.inner(),
                active_request_id.as_deref(),
            );

            if let Some(req_id) = active_request_id.as_deref() {
                let _ = history_request_lifecycle::apply_request_history_update(
                    &app,
                    RequestHistoryUpdate::CompleteError {
                        request_id: req_id.to_string(),
                        error_message: "Cancelled".to_string(),
                    },
                );
            }

            return Ok(String::new());
        }
        Err(e) => {
            #[cfg(desktop)]
            crate::set_escape_cancel_shortcut_enabled(&app, false);

            // Best-effort: persist cost/usage stats even when dictate fails.
            // (This flow is commonly used by the global hotkey / toggle path.)
            let wav_bytes = pipeline.clone_last_wav_bytes();

            if let Some(log_store) = app.try_state::<RequestLogStore>() {
                log_store.with_current(|log| {
                    log.error_with_details(
                        format!("Transcription failed: {}", e),
                        crate::request_log::format_error_chain(&e),
                    );
                    log.complete_error(e.to_string());
                });
            }

            let _ = history_request_lifecycle::sync_request_profile_from_current_log(
                &app,
                active_request_id.as_deref(),
            );

            recording_finalization::complete_current_request_with_cost_best_effort(
                &app,
                pipeline.inner(),
                active_request_id.as_deref(),
                EventStatus::Error,
                wav_bytes.as_deref(),
            );

            // Persist audio for retry (best-effort)
            if let Err(err) = recording_completion::persist_request_recording(
                &app,
                active_request_id.as_deref(),
                wav_bytes.as_deref(),
                max_saved_recordings,
            ) {
                if let Some(log_store) = app.try_state::<RequestLogStore>() {
                    let warning = err.clone();
                    log_store.with_current(|log| {
                        log.warn(warning);
                    });
                } else {
                    log::warn!("{}", err);
                }
            }

            // Update history entry with error (keep it visible for retry)
            if let Some(req_id) = active_request_id.as_deref() {
                let _ = history_request_lifecycle::apply_request_history_update(
                    &app,
                    RequestHistoryUpdate::CompleteError {
                        request_id: req_id.to_string(),
                        error_message: e.to_string(),
                    },
                );
            }

            // Time-based retention (best-effort). Still apply even on failures.
            retention::apply_transcription_retention(&app);

            // Emit pipeline-error event with request_id so the overlay can show a retry button.
            recording_completion::emit_pipeline_error(
                &app,
                &e.to_string(),
                active_request_id.as_deref(),
            );

            let mut error = CommandError::from(e);
            if let Some(req_id) = active_request_id.clone() {
                error = error.with_request_id(req_id);
            }
            return Err(error);
        }
    };

    let final_text = result.final_text.clone();

    // Capture WAV bytes once (used for duration + cost).
    let wav_bytes = pipeline.clone_last_wav_bytes();

    // Emit transcript ready event
    let _ = app.emit(events::EVENT_PIPELINE_TRANSCRIPT_READY, &final_text);
    let _ = app.emit(
        events::EVENT_PIPELINE_STATE_CHANGED,
        PipelineStateEvent::Idle,
    );

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
                let mut error = e;
                if let Some(req_id) = active_request_id.clone() {
                    error = error.with_request_id(req_id);
                }
                error
            })?;
    }

    // Log success
    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        log_store.with_current(|log| {
            recording_finalization::record_transcription_success(
                log,
                recording_finalization::TranscriptionSuccessLogUpdate {
                    result: &result,
                    formatted_transcript: Some(result.final_text.as_str()),
                    audio_duration_secs: wav_bytes.as_deref().and_then(stats::wav_duration_secs),
                    audio_size_bytes: wav_bytes.as_ref().map(|b| b.len()),
                    stt_summary_label: "STT",
                    completion_log_message: None,
                    warn_if_no_formatted_transcript: false,
                },
            );
            log.complete_success();
        });
    }

    let _ = history_request_lifecycle::sync_request_profile_from_current_log(
        &app,
        active_request_id.as_deref(),
    );

    // NOTE: `pipeline_stop_and_transcribe` and `pipeline_retry_transcription` already emit cost,
    // but `pipeline_dictate` is a separate hotkey/toggle flow and must be tracked too.
    recording_finalization::persist_current_request_preset_to_history(
        &app,
        active_request_id.as_deref(),
    );
    recording_finalization::persist_history_llm_metadata(
        &app,
        active_request_id.as_deref(),
        &result,
    );
    recording_finalization::complete_current_request_with_cost_best_effort(
        &app,
        pipeline.inner(),
        active_request_id.as_deref(),
        EventStatus::Success,
        wav_bytes.as_deref(),
    );

    // Persist audio for retry/playback (best-effort)
    if let Err(err) = recording_completion::persist_request_recording(
        &app,
        active_request_id.as_deref(),
        wav_bytes.as_deref(),
        max_saved_recordings,
    ) {
        log::warn!("{}", err);
    }

    // Update history entry with success text
    if let Some(req_id) = active_request_id.as_deref() {
        let _ = history_request_lifecycle::apply_request_history_update(
            &app,
            RequestHistoryUpdate::CompleteSuccess {
                request_id: req_id.to_string(),
                text: final_text.clone(),
            },
        );
    }

    // Time-based retention (best-effort). Runs only after a transcription attempt.
    retention::apply_transcription_retention(&app);

    #[cfg(desktop)]
    crate::set_escape_cancel_shortcut_enabled(&app, false);

    Ok(final_text)
}

/// Test transcription using the last captured audio (WAV bytes).
///
/// This is primarily used by the settings UI to validate STT provider/model.
#[tauri::command]
pub async fn pipeline_test_transcribe_last_audio(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
    profile_id: Option<String>,
) -> Result<String, CommandError> {
    let span = tracing::info_span!(
        "pipeline_test_transcribe_last_audio",
        request_id = tracing::field::Empty
    );
    pipeline_test_transcribe_last_audio_inner(app, pipeline, profile_id)
        .instrument(span)
        .await
}

async fn pipeline_test_transcribe_last_audio_inner(
    app: AppHandle,
    pipeline: State<'_, SharedPipeline>,
    profile_id: Option<String>,
) -> Result<String, CommandError> {
    // Create a dedicated request-log entry for this test action.
    // This is important because it is a standalone STT call (no recording/start step).
    let stt_started_at = Instant::now();

    let mut request_id: Option<String> = None;
    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        let cfg = pipeline.config();

        let (profile_id_used, profile_name_used) =
            resolve_profile_by_id(&cfg, profile_id.as_deref());

        // Best-effort: pick the *desired* provider/model based on profile overrides.
        // The pipeline may still fall back to global provider/model if overrides are invalid.
        let (desired_provider, desired_model) = profile_id
            .as_deref()
            .and_then(|id| {
                if id == "default" {
                    None
                } else {
                    cfg.llm_config
                        .program_prompt_profiles
                        .iter()
                        .find(|p| p.id == id)
                }
            })
            .map(|p| {
                (
                    p.stt_provider
                        .clone()
                        .unwrap_or_else(|| cfg.stt_provider.clone()),
                    p.stt_model.clone().or_else(|| cfg.stt_model.clone()),
                )
            })
            .unwrap_or_else(|| (cfg.stt_provider.clone(), cfg.stt_model.clone()));

        let request_seed = RecordingRequestSeed::new(desired_provider, desired_model)
            .with_profile(profile_id_used, profile_name_used);
        let id = start_request_log_with_seed(
            &log_store,
            &request_seed,
            LogLlmSeedMode::OmitConfigured,
            |log| {
                log.info("Test transcription started");
            },
        );
        record_request_id_on_current_span(Some(id.as_str()));
        request_id = Some(id.clone());

        // Tie OCR to this request id so OCR (if triggered) is isolated to this test request.
        pipeline.begin_ocr_session(id);
    }

    // Attempt transcription and persist cost events centrally.
    let res = pipeline
        .transcribe_last_audio_for_profile(profile_id.as_deref())
        .await;

    match res {
        Ok(s) => {
            let wav = pipeline.clone_last_wav_bytes();

            if let Some(log_store) = app.try_state::<RequestLogStore>() {
                let stt_duration_ms = stt_started_at.elapsed().as_millis() as u64;
                log_store.with_current(|log| {
                    log.audio_size_bytes = wav.as_ref().map(|b| b.len());
                    log.raw_transcript = Some(s.clone());
                    log.formatted_transcript = Some(s.clone());
                    log.stt_duration_ms = Some(stt_duration_ms);
                    log.info(format!(
                        "Test transcription completed in {}ms ({} chars)",
                        stt_duration_ms,
                        s.len()
                    ));
                    log.complete_success();
                });
            }

            // Test transcription is standalone, so preserve its legacy best-effort stats behavior
            // while still centralizing request-log closure and OCR cleanup.
            recording_finalization::complete_current_request_with_cost_best_effort(
                &app,
                pipeline.inner(),
                request_id.as_deref(),
                EventStatus::Success,
                wav.as_deref(),
            );

            Ok(s)
        }
        Err(e) => {
            let wav = pipeline.clone_last_wav_bytes();

            if let Some(log_store) = app.try_state::<RequestLogStore>() {
                let stt_duration_ms = stt_started_at.elapsed().as_millis() as u64;
                log_store.with_current(|log| {
                    log.audio_size_bytes = wav.as_ref().map(|b| b.len());
                    log.stt_duration_ms = Some(stt_duration_ms);
                    log.error_with_details(
                        format!("Test transcription failed: {}", e),
                        crate::request_log::format_error_chain(&e),
                    );
                    log.complete_error(e.to_string());
                });
            }

            recording_finalization::complete_current_request_with_cost_best_effort(
                &app,
                pipeline.inner(),
                request_id.as_deref(),
                EventStatus::Error,
                wav.as_deref(),
            );

            Err(e.into())
        }
    }
}

/// Whether there is a previously captured audio buffer available for STT testing.
#[tauri::command]
pub fn pipeline_has_last_audio(pipeline: State<'_, SharedPipeline>) -> Result<bool, CommandError> {
    Ok(pipeline.has_last_audio())
}

#[derive(Debug, Clone, serde::Serialize, JsonSchema)]
pub struct AudioSettingsTestWavs {
    pub raw_wav_base64: String,
    pub processed_wav_base64: String,
}

/// Start a recording intended for audio settings A/B testing.
///
/// This records raw audio (no filters applied during capture). The stop command will
/// return both the raw encode and the processed encode using current settings.
#[tauri::command]
pub fn pipeline_test_audio_settings_start_recording(
    pipeline: State<'_, SharedPipeline>,
) -> Result<(), CommandError> {
    pipeline.start_recording().map_err(CommandError::from)
}

/// Stop the audio settings A/B test recording and return before/after audio.
#[tauri::command]
pub fn pipeline_test_audio_settings_stop_recording(
    pipeline: State<'_, SharedPipeline>,
) -> Result<AudioSettingsTestWavs, CommandError> {
    use base64::Engine;

    let (raw_wav, processed_wav) = pipeline
        .stop_recording_before_after()
        .map_err(CommandError::from)?;

    Ok(AudioSettingsTestWavs {
        raw_wav_base64: base64::engine::general_purpose::STANDARD.encode(raw_wav),
        processed_wav_base64: base64::engine::general_purpose::STANDARD.encode(processed_wav),
    })
}

/// Get a copy of the most recent recording diagnostics (duration/RMS/peak + optional speech flag).
#[tauri::command]
pub fn pipeline_get_last_recording_diagnostics(
    pipeline: State<'_, SharedPipeline>,
) -> Result<Option<AudioCaptureDiagnostics>, CommandError> {
    Ok(pipeline.last_recording_diagnostics())
}

/// Check if the pipeline is in an error state
#[tauri::command]
pub fn pipeline_is_error(pipeline: State<'_, SharedPipeline>) -> Result<bool, CommandError> {
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
    let _ = app.emit(events::EVENT_PIPELINE_RESET, ());
    let _ = app.emit(
        events::EVENT_PIPELINE_STATE_CHANGED,
        PipelineStateEvent::Idle,
    );

    Ok(())
}
