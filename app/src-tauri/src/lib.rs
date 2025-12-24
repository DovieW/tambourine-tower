use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};
use tauri_utils::config::BackgroundThrottlingPolicy;

mod audio;
mod audio_capture;
mod audio_mute;
mod commands;
mod history;
mod llm;
mod pipeline;
mod recordings;
mod request_log;
mod settings;
mod state;
mod stt;
mod vad;
mod windows_apps;

#[cfg(test)]
mod tests;

use audio_mute::AudioMuteManager;
use history::{HistoryStorage, RequestModelInfo};
use recordings::RecordingStore;
use request_log::RequestLogStore;
use settings::HotkeyConfig;
use state::AppState;

#[cfg(desktop)]
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

#[cfg(desktop)]
use tauri_plugin_store::StoreExt;

#[cfg(desktop)]
use tauri_plugin_global_shortcut::{Shortcut, ShortcutEvent, ShortcutState};

#[cfg(desktop)]
use tauri_plugin_global_shortcut::GlobalShortcutExt;

// Define NSPanel type for overlay on macOS
#[cfg(target_os = "macos")]
tauri_nspanel::tauri_panel! {
    panel!(OverlayPanel {
        config: {
            can_become_key_window: false,
            is_floating_panel: true
        }
    })
}

/// Normalize a shortcut string for comparison (handles "ctrl" vs "control" differences)
#[cfg(desktop)]
pub(crate) fn normalize_shortcut_string(s: &str) -> String {
    s.to_lowercase()
        .replace("ctrl", "control")
        .replace("cmd", "super")
        .replace("meta", "super")
        .replace("win", "super")
}

/// Helper to read a setting from the store with a default fallback
#[cfg(desktop)]
fn get_setting_from_store<T: serde::de::DeserializeOwned>(
    app: &AppHandle,
    key: &str,
    default: T,
) -> T {
    app.store("settings.json")
        .ok()
        .and_then(|store| store.get(key))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or(default)
}

/// Ensure settings shown in the UI match what the backend will use.
///
/// The frontend often treats missing keys as "unset" and shows fallback defaults.
/// If the backend uses different fallbacks, this can cause confusing mismatches.
///
/// To prevent that, we eagerly seed `settings.json` with defaults for missing/null keys
/// (without overwriting any existing values).
#[cfg(desktop)]
fn ensure_default_settings(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    use serde_json::{json, Value};
    use tauri_plugin_store::StoreExt;

    let store = app.store("settings.json")?;

    let is_missing = |v: Option<Value>| -> bool {
        matches!(v, None | Some(Value::Null))
    };

    let mut dirty = false;
    let mut set_if_missing = |key: &str, value: Value| {
        if is_missing(store.get(key)) {
            store.set(key.to_string(), value);
            dirty = true;
        }
    };

    // Keep these defaults aligned with pipeline defaults / expected backend behavior.
    set_if_missing("stt_provider", json!("groq"));
    set_if_missing("stt_transcription_prompt", json!(null));
    set_if_missing("stt_timeout_seconds", json!(10.0));
    // How many recordings/history items to retain (impacts disk usage).
    // Keep this aligned with the UI default.
    set_if_missing("max_saved_recordings", json!(1000));
    set_if_missing("overlay_mode", json!("recording_only"));
    set_if_missing("widget_position", json!("bottom-center"));
    set_if_missing("output_mode", json!("paste"));
    set_if_missing("output_hit_enter", json!(false));
    set_if_missing("playing_audio_handling", json!("mute"));
    set_if_missing("sound_enabled", json!(true));
    set_if_missing("rewrite_llm_enabled", json!(false));
    set_if_missing("rewrite_program_prompt_profiles", json!([]));

    // Hotkeys: seed explicit defaults so both Rust and UI see the same persisted values.
    set_if_missing(
        "toggle_hotkey",
        serde_json::to_value(HotkeyConfig::default_toggle())?,
    );
    set_if_missing(
        "hold_hotkey",
        serde_json::to_value(HotkeyConfig::default_hold())?,
    );
    set_if_missing(
        "paste_last_hotkey",
        serde_json::to_value(HotkeyConfig::default_paste_last())?,
    );

    // VAD settings are used by the pipeline.
    set_if_missing(
        "vad_settings",
        serde_json::to_value(settings::VadSettings::default())?,
    );

    if dirty {
        // Persist seeded defaults.
        // If saving fails, we don't want to crash the app; the runtime fallbacks will still work.
        if let Err(e) = store.save() {
            log::warn!("Failed to save seeded default settings: {}", e);
        }
    }

    Ok(())
}

/// Emit a system event to the frontend for debugging
#[cfg(desktop)]
fn emit_system_event(app: &AppHandle, event_type: &str, message: &str, details: Option<&str>) {
    #[derive(serde::Serialize, Clone)]
    struct SystemEvent {
        timestamp: String,
        event_type: String,
        message: String,
        details: Option<String>,
    }

    let event = SystemEvent {
        timestamp: chrono::Utc::now().to_rfc3339(),
        event_type: event_type.to_string(),
        message: message.to_string(),
        details: details.map(|s| s.to_string()),
    };

    let _ = app.emit("system-event", event);
}

/// Normalize transcript text for output.
///
/// We intentionally keep this conservative: the pipeline now performs a
/// quiet-audio gate before STT to avoid "silent audio" hallucinations.
fn sanitize_transcript(transcript: &str) -> Option<String> {
    let trimmed = transcript.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

// ============================================================================
// Playing audio handling during recording
// ============================================================================

#[cfg(desktop)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlayingAudioHandling {
    None,
    Mute,
    Pause,
    MuteAndPause,
}

#[cfg(desktop)]
impl PlayingAudioHandling {
    fn from_str(s: &str) -> Self {
        match s {
            "none" => Self::None,
            "mute" => Self::Mute,
            "pause" => Self::Pause,
            "mute_and_pause" => Self::MuteAndPause,
            // Unknown values: fall back to the default.
            _ => Self::Mute,
        }
    }

    fn wants_mute(self) -> bool {
        matches!(self, Self::Mute | Self::MuteAndPause)
    }

    fn wants_pause(self) -> bool {
        matches!(self, Self::Pause | Self::MuteAndPause)
    }
}

#[cfg(desktop)]
fn get_playing_audio_handling(app: &AppHandle) -> PlayingAudioHandling {
    // Prefer the new enum setting.
    let raw: serde_json::Value =
        get_setting_from_store(app, "playing_audio_handling", serde_json::Value::Null);

    if let serde_json::Value::String(s) = raw {
        return PlayingAudioHandling::from_str(&s);
    }

    // Legacy fallback: auto_mute_audio boolean.
    // If the legacy key is missing entirely, we default to Mute.
    let legacy_raw: serde_json::Value =
        get_setting_from_store(app, "auto_mute_audio", serde_json::Value::Null);

    match legacy_raw {
        serde_json::Value::Bool(true) => PlayingAudioHandling::Mute,
        serde_json::Value::Bool(false) => PlayingAudioHandling::None,
        _ => PlayingAudioHandling::Mute,
    }
}

#[cfg(desktop)]
fn toggle_media_play_pause(app: &AppHandle) -> Result<(), String> {
    // On macOS, enigo requires running on the main thread.
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        app.run_on_main_thread(move || {
            let mut enigo = match Enigo::new(&Settings::default()) {
                Ok(e) => e,
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
            };

            let result = enigo
                .key(Key::MediaPlayPause, Direction::Click)
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        })
        .map_err(|e| e.to_string())?;
        return rx.recv().map_err(|e| e.to_string())?;
    }

    #[cfg(not(target_os = "macos"))]
    {
        // `app` is only needed on macOS (main-thread requirement). Silence the
        // unused-parameter warning on other platforms without changing behavior.
        let _ = app;
        let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
        enigo
            .key(Key::MediaPlayPause, Direction::Click)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn is_non_system_audio_session_active() -> Result<bool, String> {
    use windows::Win32::Media::Audio::{
        eConsole, eRender, AudioSessionStateActive, IAudioSessionManager2, IMMDevice,
        IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    unsafe {
        // Initialize COM (ignore error if already initialized)
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("Failed to create device enumerator: {}", e))?;

        let device: IMMDevice = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| format!("Failed to get default audio endpoint: {}", e))?;

        // Enumerate sessions on the default render endpoint.
        let session_manager = device
            .Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)
            .map_err(|e| format!("Failed to activate session manager: {}", e))?;

        let sessions = session_manager
            .GetSessionEnumerator()
            .map_err(|e| format!("Failed to get session enumerator: {}", e))?;

        let count = sessions
            .GetCount()
            .map_err(|e| format!("Failed to get session count: {}", e))?;

        for i in 0..count {
            let session = sessions
                .GetSession(i)
                .map_err(|e| format!("Failed to get session {}: {}", i, e))?;

            let state = session
                .GetState()
                .map_err(|e| format!("Failed to get session state: {}", e))?;
            if state == AudioSessionStateActive {
                return Ok(true);
            }
        }

        Ok(false)
    }
}

#[cfg(not(target_os = "windows"))]
fn is_non_system_audio_session_active() -> Result<bool, String> {
    // Best-effort on non-Windows platforms: we don't currently have a reliable
    // cross-platform way to detect whether audio is actively playing.
    Ok(true)
}

/// Start recording with sound and audio mute handling
#[cfg(desktop)]
fn start_recording(
    app: &AppHandle,
    state: &AppState,
    sound_enabled: bool,
    audio_cue: audio::AudioCue,
    audio_mute_manager: &Option<tauri::State<'_, AudioMuteManager>>,
    playing_audio_handling: PlayingAudioHandling,
    source: &str,
) {
    // Log current pipeline state before attempting to start
    let current_state = app
        .try_state::<pipeline::SharedPipeline>()
        .map(|p| p.state());
    log::info!("{}: starting recording (current pipeline state: {:?})", source, current_state);
    emit_system_event(app, "shortcut", &format!("{}: starting recording", source), Some(&format!("Pipeline state: {:?}", current_state)));

    // Start pipeline recording FIRST - if it fails, don't do anything else
    if let Some(pipeline) = app.try_state::<pipeline::SharedPipeline>() {
        if let Err(e) = pipeline.start_recording() {
            log::error!("{}: Failed to start pipeline recording: {} (state was: {:?})", source, e, current_state);
            let error_msg = format!("{} (pipeline state: {:?})", e, current_state);
            emit_system_event(app, "error", &format!("{}: Failed to start recording", source), Some(&error_msg));
            let payload = serde_json::json!({
                "message": error_msg,
                "request_id": null,
            });
            let _ = app.emit("pipeline-error", payload);
            return;
        }

        // Pipeline started successfully - now start request logging.
        if let Some(log_store) = app.try_state::<RequestLogStore>() {
            let config = pipeline.config();
            log_store.start_request(config.stt_provider.clone(), config.stt_model.clone());
            log_store.with_current(|log| {
                log.llm_provider = if config.llm_config.enabled {
                    Some(config.llm_config.provider.clone())
                } else {
                    None
                };
                log.llm_model = config.llm_config.model.clone();
                log.info(format!("Recording started ({})", source));
            });
        }
    }

    // While recording/transcribing, allow Escape to cancel without triggering transcription.
    set_escape_cancel_shortcut_enabled(app, true);

    // Pipeline started successfully - now update state and do side effects
    state.is_recording.store(true, Ordering::SeqCst);

    // Start the recording chime ASAP.
    // Showing/snapping the overlay window can be a bit slow on some systems (monitor queries,
    // position math, window show), so we kick off audio playback *before* that work.
    //
    // If we're about to mute system audio, defer the mute until the cue has finished playing,
    // but do so off-thread so the overlay can appear immediately.
    if sound_enabled {
        if playing_audio_handling.wants_mute() {
            let app_for_audio = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = audio::play_sound_blocking(audio::SoundType::RecordingStart, audio_cue)
                {
                    log::warn!("Failed to play start sound: {}", e);
                }

                if let Some(manager) = app_for_audio.try_state::<AudioMuteManager>() {
                    if let Err(e) = manager.mute() {
                        log::warn!("Failed to mute audio: {}", e);
                    }
                }
            });
        } else {
            // No immediate mute: play asynchronously to keep the UI responsive.
            audio::play_sound(audio::SoundType::RecordingStart, audio_cue);
        }
    }

    // Show overlay if in "recording_only" mode
    let overlay_mode: String =
        get_setting_from_store(app, "overlay_mode", "recording_only".to_string());
    if overlay_mode == "recording_only" {
        let _ = commands::overlay::show_overlay_with_reset_if_not_always(app);
    }

    // Prime the overlay waveform immediately. The background publisher loop will follow up
    // with real levels as soon as the first CPAL callback arrives.
    //
    // This also helps when the overlay is shown at start: the listener may not yet be
    // registered when the very first publisher tick runs.
    {
        let payload = serde_json::json!({
            "seq": 0,
            "rms": 0.0,
            "peak": 0.0,
            "wave_seq": 0,
            "mins": Vec::<f32>::new(),
            "maxes": Vec::<f32>::new(),
        });
        if let Some(overlay) = app.get_webview_window("overlay") {
            let _ = overlay.emit("overlay-audio-level", payload);
        } else {
            let _ = app.emit("overlay-audio-level", payload);
        }
    }

    // Notify frontend ASAP so the overlay can update/animate without waiting for
    // audio side-effects (which may block, e.g. when we ensure the cue finishes
    // before muting system audio).
    let _ = app.emit("recording-start", ());

    // Mute system audio if enabled.
    // If sound is enabled, mute is deferred until after the cue finishes (see above).
    if playing_audio_handling.wants_mute() && !sound_enabled {
        if let Some(manager) = audio_mute_manager {
            if let Err(e) = manager.mute() {
                log::warn!("Failed to mute audio: {}", e);
            }
        }
    }

    // Pause playing audio (best-effort).
    if playing_audio_handling.wants_pause() {
        match is_non_system_audio_session_active() {
            Ok(true) => match toggle_media_play_pause(app) {
                Ok(()) => {
                    state.play_pause_toggled.store(true, Ordering::SeqCst);
                }
                Err(e) => {
                    log::warn!("Failed to toggle media play/pause: {}", e);
                    state.play_pause_toggled.store(false, Ordering::SeqCst);
                }
            },
            Ok(false) => {
                // Nothing appears to be playing: don't send play/pause,
                // otherwise we might accidentally start playback.
                state.play_pause_toggled.store(false, Ordering::SeqCst);
            }
            Err(e) => {
                // Detection failed: be conservative and avoid toggling.
                log::warn!("Failed to detect active audio session; skipping pause: {}", e);
                state.play_pause_toggled.store(false, Ordering::SeqCst);
            }
        }
    } else {
        state.play_pause_toggled.store(false, Ordering::SeqCst);
    }

}

/// Stop recording with sound and audio unmute handling
#[cfg(desktop)]
fn stop_recording(
    app: &AppHandle,
    state: &AppState,
    sound_enabled: bool,
    audio_cue: audio::AudioCue,
    audio_mute_manager: &Option<tauri::State<'_, AudioMuteManager>>,
    playing_audio_handling: PlayingAudioHandling,
    source: &str,
) {
    state.is_recording.store(false, Ordering::SeqCst);
    log::info!("{}: stopping recording", source);
    emit_system_event(app, "shortcut", &format!("{}: stopping recording", source), None);

    // If hallucination protection (quiet-audio gate) is enabled and the recording is considered
    // effectively quiet, the pipeline will skip STT and immediately return to Idle.
    // In that case, playing the stop sound is misleading, so we only play it if we actually
    // enter Transcribing/Rewriting.
    let quiet_audio_gate_enabled: bool =
        get_setting_from_store(app, "quiet_audio_gate_enabled", true);
    let play_stop_sound_when_transcribing = sound_enabled && quiet_audio_gate_enabled;

    // Keep Escape-to-cancel enabled during the transcription phase too.
    set_escape_cancel_shortcut_enabled(app, true);
    // Unmute system audio if it was muted
    if playing_audio_handling.wants_mute() {
        if let Some(manager) = audio_mute_manager {
            if let Err(e) = manager.unmute() {
                log::warn!("Failed to unmute audio: {}", e);
            }
        }
    }
    // If the quiet-audio gate is disabled, play the stop sound immediately as before.
    if sound_enabled && !quiet_audio_gate_enabled {
        audio::play_sound(audio::SoundType::RecordingStop, audio_cue);
    }

    // Resume playing audio if we previously toggled it.
    if playing_audio_handling.wants_pause()
        && state.play_pause_toggled.swap(false, Ordering::SeqCst)
    {
        if let Err(e) = toggle_media_play_pause(app) {
            log::warn!("Failed to restore media play/pause: {}", e);
        }
    }

    // Get overlay mode for hiding after transcription
    let overlay_mode: String =
        get_setting_from_store(app, "overlay_mode", "recording_only".to_string());

    // Get output mode for how to output text
    let output_mode_str: String = get_setting_from_store(app, "output_mode", "paste".to_string());
    let output_mode = commands::text::OutputMode::from_str(&output_mode_str);

    // Optional: after pasting, press Enter.
    let output_hit_enter: bool = get_setting_from_store(app, "output_hit_enter", false);

    // Stop pipeline and trigger transcription in background
    if let Some(pipeline) = app.try_state::<pipeline::SharedPipeline>() {
        let pipeline_clone = (*pipeline).clone();
        let app_clone = app.clone();
        let overlay_mode_clone = overlay_mode.clone();

        // Capture current request id (for history + retry audio).
        let request_id: Option<String> = app
            .try_state::<RequestLogStore>()
            .and_then(|store| store.with_current(|log| log.id.clone()));

        // Capture model info from pipeline config for persistence in history.
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

        tauri::async_runtime::spawn(async move {
            // Emit transcription started only once the pipeline actually transitions
            // into Transcribing (quiet-audio gate skips should fade out without ever
            // showing "TRANSCRIBING...").
            {
                let app_for_evt = app_clone.clone();
                let pipeline_for_evt = pipeline_clone.clone();
                let audio_cue_for_stop = audio_cue;
                let should_play_stop_sound = play_stop_sound_when_transcribing;
                tauri::async_runtime::spawn(async move {
                    let start = std::time::Instant::now();
                    loop {
                        match pipeline_for_evt.state() {
                            pipeline::PipelineState::Transcribing
                            | pipeline::PipelineState::Rewriting => {
                                let _ = app_for_evt.emit("pipeline-transcription-started", ());

                                if should_play_stop_sound {
                                    crate::audio::play_sound(
                                        crate::audio::SoundType::RecordingStop,
                                        audio_cue_for_stop,
                                    );
                                }
                                break;
                            }
                            pipeline::PipelineState::Idle | pipeline::PipelineState::Error => {
                                // Idle can happen immediately due to quiet-audio skip.
                                break;
                            }
                            pipeline::PipelineState::Recording => {}
                        }

                        if start.elapsed() > std::time::Duration::from_secs(2) {
                            break;
                        }

                        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                    }
                });
            }

            // Create an in-progress history entry while we transcribe.
            if let Some(ref req_id) = request_id {
                if let Some(history) = app_clone.try_state::<HistoryStorage>() {
                    let max_saved_recordings: usize = (get_setting_from_store(
                        &app_clone,
                        "max_saved_recordings",
                        1000u64,
                    ))
                    .clamp(1, 100_000) as usize;

                    let _ = history.add_request_entry(
                        req_id.clone(),
                        model_info,
                        max_saved_recordings,
                    );
                    let _ = app_clone.emit("history-changed", ());
                }
            }

            // Log transcription start for this request
            if let Some(log_store) = app_clone.try_state::<RequestLogStore>() {
                log_store.with_current(|log| {
                    log.info("Recording stopped, starting transcription");
                });
            }

            match pipeline_clone.stop_and_transcribe_detailed().await {
                Ok(result) => {
                    log::info!("Transcription complete: {} chars", result.final_text.len());

                    // Final output after pipeline (STT + optional LLM) normalization.
                    // Quiet recordings should already have been skipped in the pipeline.
                    let filtered_transcript = sanitize_transcript(&result.final_text);

                    // Update request log store
                    if let Some(log_store) = app_clone.try_state::<RequestLogStore>() {
                        log_store.with_current(|log| {
                            // Raw STT output (pre-LLM)
                            log.raw_transcript = Some(result.stt_text.clone());

                            // Final output after pipeline + hallucination filtering (if any)
                            if let Some(ref text) = filtered_transcript {
                                log.formatted_transcript = Some(text.clone());
                            }

                            log.stt_duration_ms = Some(result.stt_duration_ms);
                            log.llm_duration_ms = result.llm_duration_ms;

                            log.info(format!(
                                "STT completed in {}ms ({} chars)",
                                result.stt_duration_ms,
                                result.stt_text.len()
                            ));

                            match &result.llm_outcome {
                                pipeline::LlmOutcome::NotAttempted => {
                                    log.info("LLM formatting not attempted (disabled or unavailable)");
                                }
                                pipeline::LlmOutcome::Succeeded => {
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
                                pipeline::LlmOutcome::TimedOut => {
                                    if let Some(ms) = result.llm_duration_ms {
                                        log.warn(format!(
                                            "LLM formatting timed out after {}ms; fell back to STT transcript",
                                            ms
                                        ));
                                    } else {
                                        log.warn("LLM formatting timed out; fell back to STT transcript");
                                    }
                                }
                                pipeline::LlmOutcome::Failed(err) => {
                                    log.warn(format!(
                                        "LLM formatting failed; fell back to STT transcript ({})",
                                        err
                                    ));
                                }
                            }

                            if filtered_transcript.is_none() {
                                log.warn("No transcript output (empty/whitespace)");
                            }

                            log.complete_success();
                        });
                        log_store.complete_current();
                    }

                    // Persist audio for retry (best-effort)
                    if let (Some(ref req_id), Some(store)) = (
                        request_id.as_ref(),
                        app_clone.try_state::<RecordingStore>(),
                    ) {
                        if let Some(wav) = pipeline_clone.clone_last_wav_bytes() {
                            if store.save_wav(req_id, &wav).is_ok() {
                                let max_saved_recordings: usize = (get_setting_from_store(
                                    &app_clone,
                                    "max_saved_recordings",
                                    1000u64,
                                ))
                                .clamp(1, 100_000) as usize;

                                let _ = store.prune_to_max_files(max_saved_recordings);
                            }
                        }
                    }

                    if let Some(ref text) = filtered_transcript {
                        let _ = app_clone.emit("pipeline-transcript-ready", text);

                        // Output the transcript based on mode
                        if let Err(e) = commands::text::output_text_with_mode(text, output_mode, output_hit_enter) {
                            log::error!("Failed to output transcript: {}", e);

                            if let Some(log_store) = app_clone.try_state::<RequestLogStore>() {
                                log_store.with_current(|log| {
                                    log.warn(format!("Output failed: {}", e));
                                });
                            }
                        }

                        // Save to history
                        if let Some(ref req_id) = request_id {
                            if let Some(history) = app_clone.try_state::<HistoryStorage>() {
                                if let Err(e) = history.complete_request_success(req_id, text.clone()) {
                                    log::warn!("Failed to update history: {}", e);
                                }
                                let _ = app_clone.emit("history-changed", ());
                            }
                        }
                    } else {
                        // Emit empty transcript event so UI can update appropriately
                        let _ = app_clone.emit("pipeline-transcript-ready", "");
                        log::info!("No transcript output (empty/whitespace), not outputting");

                        // Mark history entry as success with empty text (keeps timeline consistent)
                        if let Some(ref req_id) = request_id {
                            if let Some(history) = app_clone.try_state::<HistoryStorage>() {
                                let _ = history.complete_request_success(req_id, String::new());
                                let _ = app_clone.emit("history-changed", ());
                            }
                        }
                    }

                    // Hide overlay after transcription completes if in "recording_only" mode.
                    // We request a hide so the frontend can animate (zoom-out) before the webview hides.
                    if overlay_mode_clone == "recording_only" {
                        let _ = app_clone.emit("overlay-hide-requested", ());

                        // Fallback: if the overlay frontend isn't running/listening, hide anyway.
                        // Re-check the current overlay_mode before hiding to avoid races with settings changes.
                        if let Some(window) = app_clone.get_webview_window("overlay") {
                            let window_clone = window.clone();
                            let app_check = app_clone.clone();
                            tauri::async_runtime::spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_millis(220)).await;
                                let current_mode: String = get_setting_from_store(
                                    &app_check,
                                    "overlay_mode",
                                    "recording_only".to_string(),
                                );
                                if current_mode == "recording_only" {
                                    let _ = window_clone.hide();
                                }
                            });
                        }
                    }
                }
                Err(e) => {
                    if matches!(e, pipeline::PipelineError::Cancelled) {
                        log::info!("Transcription cancelled");

                        // Mark request as cancelled (best-effort)
                        if let Some(log_store) = app_clone.try_state::<RequestLogStore>() {
                            log_store.with_current(|log| {
                                log.warn("Recording cancelled by user");
                                log.complete_cancelled();
                            });
                            log_store.complete_current();
                        }

                        // Notify frontend and hide overlay if needed.
                        let _ = app_clone.emit("pipeline-cancelled", ());

                        if overlay_mode_clone == "recording_only" {
                            let _ = app_clone.emit("overlay-hide-requested", ());
                            if let Some(window) = app_clone.get_webview_window("overlay") {
                                let _ = window.hide();
                            }
                        }

                        // Done - stop stealing Escape.
                        crate::set_escape_cancel_shortcut_enabled(&app_clone, false);
                        return;
                    }

                    log::error!("Transcription failed: {}", e);
                    let payload = serde_json::json!({
                        "message": e.to_string(),
                        "request_id": request_id.clone(),
                    });
                    let _ = app_clone.emit("pipeline-error", payload);

                    if let Some(log_store) = app_clone.try_state::<RequestLogStore>() {
                        log_store.with_current(|log| {
                            log.error(format!("Transcription failed: {}", e));
                            log.complete_error(e.to_string());
                        });
                        log_store.complete_current();
                    }

                    // Persist audio for retry (best-effort)
                    if let (Some(ref req_id), Some(store)) = (
                        request_id.as_ref(),
                        app_clone.try_state::<RecordingStore>(),
                    ) {
                        if let Some(wav) = pipeline_clone.clone_last_wav_bytes() {
                            if store.save_wav(req_id, &wav).is_ok() {
                                let max_saved_recordings: usize = (get_setting_from_store(
                                    &app_clone,
                                    "max_saved_recordings",
                                    1000u64,
                                ))
                                .clamp(1, 100_000) as usize;

                                let _ = store.prune_to_max_files(max_saved_recordings);
                            }
                        }
                    }

                    // Mark history entry as error and keep it
                    if let Some(ref req_id) = request_id {
                        if let Some(history) = app_clone.try_state::<HistoryStorage>() {
                            let _ = history.complete_request_error(req_id, e.to_string());
                            let _ = app_clone.emit("history-changed", ());
                        }
                    }

                    // Force-show overlay for retry UI regardless of overlay_mode.
                    // If the user is not in always-visible mode, also snap back to the saved preset.
                    let _ = commands::overlay::show_overlay_with_reset_if_not_always(&app_clone);

                }
            }

            // Transcription finished (success or error) - stop stealing Escape.
            crate::set_escape_cancel_shortcut_enabled(&app_clone, false);
        });
    }

    let _ = app.emit("recording-stop", ());
}

// ============================================================================
// Escape-to-cancel support
// ============================================================================

#[cfg(desktop)]
const ESCAPE_CANCEL_SHORTCUT: &str = "Escape";

/// Enable/disable the Escape global shortcut that cancels the current pipeline session.
///
/// We register this shortcut only while the pipeline is Recording/Transcribing so we don't
/// steal Escape from other apps while idle.
#[cfg(desktop)]
pub(crate) fn set_escape_cancel_shortcut_enabled(app: &AppHandle, enabled: bool) {
    // IMPORTANT: this function can be called from within a global-shortcut callback.
    // Registering/unregistering shortcuts re-entrantly can crash/deadlock on some platforms.
    // Schedule the actual work onto the async runtime to avoid re-entrancy.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        set_escape_cancel_shortcut_enabled_inner(&app, enabled);
    });
}

#[cfg(desktop)]
fn set_escape_cancel_shortcut_enabled_inner(app: &AppHandle, enabled: bool) {
    let shortcut_manager = app.global_shortcut();

    let is_registered = shortcut_manager.is_registered(ESCAPE_CANCEL_SHORTCUT);
    log::debug!(
        "Escape shortcut toggle: enabled={} (currently registered={})",
        enabled,
        is_registered
    );

    if enabled {
        if is_registered {
            return;
        }

        if let Err(e) = shortcut_manager.on_shortcut(ESCAPE_CANCEL_SHORTCUT, |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                cancel_pipeline_session(app, "Escape");
            }
        }) {
            log::warn!(
                "Failed to register Escape cancel shortcut ({}): {}",
                ESCAPE_CANCEL_SHORTCUT,
                e
            );
        }
    } else if is_registered {
        if let Err(e) = shortcut_manager.unregister(ESCAPE_CANCEL_SHORTCUT) {
            log::warn!(
                "Failed to unregister Escape cancel shortcut ({}): {}",
                ESCAPE_CANCEL_SHORTCUT,
                e
            );
        }
    }
}

/// Cancel current recording/transcription without triggering transcription output.
///
/// This is used by Escape-to-cancel and can also be reused by commands.
#[cfg(desktop)]
pub(crate) fn cancel_pipeline_session(app: &AppHandle, source: &str) {
    let state = app.state::<AppState>();

    // Best-effort: capture the active request id so we can clean up history.
    let active_request_id: Option<String> = app
        .try_state::<RequestLogStore>()
        .and_then(|store| store.with_current(|log| log.id.clone()));

    // If pipeline isn't in a cancellable state, ignore.
    let can_cancel = app
        .try_state::<pipeline::SharedPipeline>()
        .map(|p| p.state().can_cancel())
        .unwrap_or(false);

    if !can_cancel {
        // Defensive: if we somehow still have the shortcut registered while idle, disable it.
        set_escape_cancel_shortcut_enabled(app, false);
        return;
    }

    log::info!("{}: cancelling recording/transcription", source);
    emit_system_event(app, "shortcut", &format!("{}: cancelling", source), None);

    // Clear recording state flags.
    state.is_recording.store(false, Ordering::SeqCst);
    state.toggle_key_held.store(false, Ordering::SeqCst);
    state.ptt_key_held.store(false, Ordering::SeqCst);

    // Restore audio side effects (unmute + resume playback if we paused).
    let sound_enabled: bool = get_setting_from_store(app, "sound_enabled", true);
    let playing_audio_handling: PlayingAudioHandling = get_playing_audio_handling(app);
    let audio_mute_manager = app.try_state::<AudioMuteManager>();

    if playing_audio_handling.wants_mute() {
        if let Some(manager) = audio_mute_manager.as_ref() {
            if let Err(e) = manager.unmute() {
                log::warn!("Failed to unmute audio after cancel: {}", e);
            }
        }
    }

    if playing_audio_handling.wants_pause()
        && state.play_pause_toggled.swap(false, Ordering::SeqCst)
    {
        if let Err(e) = toggle_media_play_pause(app) {
            log::warn!("Failed to restore media play/pause after cancel: {}", e);
        }
    }

    if sound_enabled {
        let audio_cue_raw: String =
            get_setting_from_store(app, "audio_cue", "tangerine".to_string());
        let audio_cue = audio::AudioCue::from_str(&audio_cue_raw);
        audio::play_sound(audio::SoundType::RecordingStop, audio_cue);
    }

    // Cancel request log
    if let Some(log_store) = app.try_state::<RequestLogStore>() {
        log_store.with_current(|log| {
            log.warn("Recording cancelled by user");
            log.complete_cancelled();
        });
        log_store.complete_current();
    }

    // Best-effort: remove any in-progress history entry for this request.
    if let Some(req_id) = active_request_id.as_deref() {
        if let Some(history) = app.try_state::<HistoryStorage>() {
            let _ = history.delete(req_id);
            let _ = app.emit("history-changed", ());
        }
    }

    // Cancel pipeline
    if let Some(pipeline) = app.try_state::<pipeline::SharedPipeline>() {
        pipeline.cancel();
    }

    // Hide overlay if in recording-only mode.
    let overlay_mode: String =
        get_setting_from_store(app, "overlay_mode", "recording_only".to_string());
    if overlay_mode == "recording_only" {
        let _ = app.emit("overlay-hide-requested", ());

        if let Some(window) = app.get_webview_window("overlay") {
            let window_clone = window.clone();
            let app_check = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(220)).await;
                let current_mode: String = get_setting_from_store(
                    &app_check,
                    "overlay_mode",
                    "recording_only".to_string(),
                );
                if current_mode == "recording_only" {
                    let _ = window_clone.hide();
                }
            });
        }
    }

    // Notify frontend
    let _ = app.emit("pipeline-cancelled", ());

    // Disable Escape shortcut now that we're idle.
    set_escape_cancel_shortcut_enabled(app, false);
}

/// Handle a shortcut event - public so it can be called from commands/settings.rs
#[cfg(desktop)]
pub fn handle_shortcut_event(app: &AppHandle, shortcut: &Shortcut, event: &ShortcutEvent) {
    let state = app.state::<AppState>();

    // Get current settings from store
    let sound_enabled: bool = get_setting_from_store(app, "sound_enabled", true);
    let audio_cue_raw: String = get_setting_from_store(app, "audio_cue", "tangerine".to_string());
    let audio_cue = audio::AudioCue::from_str(&audio_cue_raw);
    let playing_audio_handling: PlayingAudioHandling = get_playing_audio_handling(app);

    // Get shortcut string for comparison (normalized to handle "ctrl" vs "control" differences)
    let shortcut_str = normalize_shortcut_string(&shortcut.to_string());

    // Get configured shortcut strings from store (normalized), with validation fallback
    let toggle_hotkey: HotkeyConfig =
        get_setting_from_store(app, "toggle_hotkey", HotkeyConfig::default_toggle());
    let hold_hotkey: HotkeyConfig =
        get_setting_from_store(app, "hold_hotkey", HotkeyConfig::default_hold());
    let paste_last_hotkey: HotkeyConfig =
        get_setting_from_store(app, "paste_last_hotkey", HotkeyConfig::default_paste_last());

    // Validate hotkeys - if they can't be parsed as shortcuts, use defaults
    let toggle_shortcut_str = normalize_shortcut_string(
        &toggle_hotkey
            .to_shortcut()
            .map(|_| toggle_hotkey.to_shortcut_string())
            .unwrap_or_else(|_| HotkeyConfig::default_toggle().to_shortcut_string()),
    );
    let hold_shortcut_str = normalize_shortcut_string(
        &hold_hotkey
            .to_shortcut()
            .map(|_| hold_hotkey.to_shortcut_string())
            .unwrap_or_else(|_| HotkeyConfig::default_hold().to_shortcut_string()),
    );
    let paste_last_shortcut_str = normalize_shortcut_string(
        &paste_last_hotkey
            .to_shortcut()
            .map(|_| paste_last_hotkey.to_shortcut_string())
            .unwrap_or_else(|_| HotkeyConfig::default_paste_last().to_shortcut_string()),
    );

    // Get audio mute manager if available
    let audio_mute_manager = app.try_state::<AudioMuteManager>();

    // Compare normalized strings directly
    let is_toggle = shortcut_str == toggle_shortcut_str;
    let is_hold = shortcut_str == hold_shortcut_str;
    let is_paste_last = shortcut_str == paste_last_shortcut_str;

    if is_toggle {
        // Toggle mode: action happens on key release (debounced)
        match event.state {
            ShortcutState::Pressed => {
                state.toggle_key_held.swap(true, Ordering::SeqCst);
            }
            ShortcutState::Released => {
                if state.toggle_key_held.swap(false, Ordering::SeqCst) {
                    // Check pipeline state directly instead of AppState
                    let pipeline_state = app
                        .try_state::<pipeline::SharedPipeline>()
                        .map(|p| p.state());

                    log::info!("Toggle released: pipeline state = {:?}", pipeline_state);
                    emit_system_event(app, "shortcut", "Toggle key released", Some(&format!("Pipeline state: {:?}", pipeline_state)));

                    let is_recording = pipeline_state == Some(pipeline::PipelineState::Recording);

                    if is_recording {
                        stop_recording(
                            app,
                            &state,
                            sound_enabled,
                            audio_cue,
                            &audio_mute_manager,
                            playing_audio_handling,
                            "Toggle",
                        );
                    } else {
                        start_recording(
                            app,
                            &state,
                            sound_enabled,
                            audio_cue,
                            &audio_mute_manager,
                            playing_audio_handling,
                            "Toggle",
                        );
                    }
                }
            }
        }
    } else if is_hold {
        // Hold-to-Record: start on press, stop on release
        match event.state {
            ShortcutState::Pressed => {
                if !state.ptt_key_held.swap(true, Ordering::SeqCst) {
                    // Only start if pipeline is not already recording/transcribing
                    let pipeline_state = app
                        .try_state::<pipeline::SharedPipeline>()
                        .map(|p| p.state());

                    log::info!("Hold pressed: pipeline state = {:?}", pipeline_state);
                    emit_system_event(app, "shortcut", "Hold key pressed", Some(&format!("Pipeline state: {:?}", pipeline_state)));

                    let can_start = pipeline_state
                        .map(|s| s.can_start_recording())
                        .unwrap_or(false);

                    if can_start {
                        start_recording(
                            app,
                            &state,
                            sound_enabled,
                            audio_cue,
                            &audio_mute_manager,
                            playing_audio_handling,
                            "Hold",
                        );
                    }
                }
            }
            ShortcutState::Released => {
                if state.ptt_key_held.swap(false, Ordering::SeqCst) {
                    // Only stop if pipeline is actually recording
                    let is_recording = app
                        .try_state::<pipeline::SharedPipeline>()
                        .map(|p| p.state() == pipeline::PipelineState::Recording)
                        .unwrap_or(false);

                    if is_recording {
                        stop_recording(
                            app,
                            &state,
                            sound_enabled,
                            audio_cue,
                            &audio_mute_manager,
                            playing_audio_handling,
                            "Hold",
                        );
                    }
                }
            }
        }
    } else if is_paste_last {
        // Output last transcription: hold-to-output (output happens on release)
        match event.state {
            ShortcutState::Pressed => {
                // Mark key as held (ignore OS key repeat)
                state.paste_key_held.swap(true, Ordering::SeqCst);
            }
            ShortcutState::Released => {
                if state.paste_key_held.swap(false, Ordering::SeqCst) {
                    // Key released - output based on configured mode
                    log::info!("OutputLast: outputting last transcription");

                    // Get output mode from settings
                    let output_mode_str: String = get_setting_from_store(app, "output_mode", "paste".to_string());
                    let output_mode = commands::text::OutputMode::from_str(&output_mode_str);

                    let output_hit_enter: bool = get_setting_from_store(app, "output_hit_enter", false);

                    let history_storage = app.state::<HistoryStorage>();

                    if let Ok(entries) = history_storage.get_all(Some(1)) {
                        if let Some(entry) = entries.first() {
                            if let Err(e) = commands::text::output_text_with_mode(&entry.text, output_mode, output_hit_enter) {
                                log::error!("Failed to output last transcription: {}", e);
                            }
                        } else {
                            log::info!("OutputLast: no history entries available");
                        }
                    }
                }
            }
        }
    } else {
        log::warn!("Unknown shortcut: {}", shortcut_str);
    }
}

/// Check if audio mute is supported on this platform
#[tauri::command]
fn is_audio_mute_supported() -> bool {
    audio_mute::is_supported()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize logger
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut builder = tauri::Builder::default();

    #[cfg(desktop)]
    {
        builder = builder.plugin(build_global_shortcut_plugin());
    }

    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_nspanel::init());
    }

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::audio::play_audio_cue_preview,
            commands::audio::list_audio_input_devices,
            commands::audio::get_default_audio_input_device_name,
            commands::text::type_text,
            commands::text::get_server_url,
            commands::settings::register_shortcuts,
            commands::settings::unregister_shortcuts,
            is_audio_mute_supported,
            commands::history::add_history_entry,
            commands::history::get_history,
            commands::history::delete_history_entry,
            commands::history::clear_history,
            commands::overlay::resize_overlay,
            commands::overlay::show_overlay,
            commands::overlay::hide_overlay,
            commands::overlay::set_overlay_mode,
            commands::overlay::set_widget_position,
            // Pipeline commands for all-in-app STT
            commands::recording::pipeline_start_recording,
            commands::recording::pipeline_stop_and_transcribe,
            commands::recording::pipeline_cancel,
            commands::recording::pipeline_get_state,
            commands::recording::pipeline_is_recording,
            commands::recording::pipeline_is_error,
            commands::recording::pipeline_update_config,
            commands::recording::pipeline_dictate,
            commands::recording::pipeline_toggle,
            commands::recording::pipeline_force_reset,
            commands::recording::pipeline_test_transcribe_last_audio,
            commands::recording::pipeline_has_last_audio,
            commands::recording::pipeline_retry_transcription,
            // Recording file access (for playback)
            commands::recording::recording_get_wav_path,
            commands::recording::recording_get_wav_base64,
            // Recording folder helpers
            commands::recording::recordings_open_folder,
            commands::recording::recordings_get_storage_bytes,
            commands::recording::recordings_get_stats,
            // Config commands (replacing Python server)
            commands::config::get_default_sections,
            commands::config::get_available_providers,
            commands::config::sync_pipeline_config,
            // VAD settings commands
            commands::config::get_vad_settings,
            commands::config::set_vad_settings,
            // LLM formatting commands
            commands::llm::get_llm_default_prompts,
            commands::llm::get_llm_providers,
            commands::llm::update_llm_config,
            commands::llm::update_llm_prompts,
            commands::llm::get_llm_config,
            commands::llm::test_llm_rewrite,
            // Local Whisper model management commands
            commands::whisper::is_local_whisper_available,
            commands::whisper::get_whisper_models,
            commands::whisper::get_whisper_models_dir,
            commands::whisper::is_whisper_model_downloaded,
            commands::whisper::get_whisper_model_url,
            commands::whisper::delete_whisper_model,
            commands::whisper::validate_whisper_model,
            // Request logging commands
            commands::logs::get_request_logs,
            commands::logs::clear_request_logs,
            // Window/process commands (used for per-program prompts)
            commands::windows::list_open_windows,
            commands::windows::get_foreground_process_path,
        ])
        .setup(|app| {
            // Seed defaults into settings.json so UI and backend agree on effective settings.
            // Must run before pipeline initialization and any settings reads.
            #[cfg(desktop)]
            {
                ensure_default_settings(app.handle())?;
            }

            // Initialize history storage
            let app_data_dir = app
                .path()
                .app_data_dir()
                .expect("Failed to get app data directory");

            // Initialize recording store (saved WAVs for retry)
            let recording_store = RecordingStore::new(app_data_dir.clone());
            app.manage(recording_store);

            let history_storage = HistoryStorage::new(app_data_dir);
            app.manage(history_storage);

            // Apply the configured history retention limit immediately so existing installs
            // don't keep more entries than the UI/backend intend.
            #[cfg(desktop)]
            {
                let max_saved_recordings: u64 =
                    get_setting_from_store(app.handle(), "max_saved_recordings", 1000u64);
                if let Some(history) = app.try_state::<HistoryStorage>() {
                    let _ = history.trim_to(max_saved_recordings as usize);
                }
            }

            // Initialize request log store
            let request_log_store = request_log::RequestLogStore::new();
            app.manage(request_log_store);

            // Initialize audio mute manager (may be None on unsupported platforms)
            if let Some(audio_mute_manager) = AudioMuteManager::new() {
                app.manage(audio_mute_manager);
            }

            // Initialize pipeline with settings from store
            #[cfg(desktop)]
            {
                let pipeline = initialize_pipeline_from_settings(app.handle());
                app.manage(pipeline);
            }

            // Backend-driven overlay waveform: publish realtime mic levels to the overlay.
            // This avoids browser getUserMedia startup latency and stays aligned with the
            // actual CPAL capture stream.
            #[cfg(desktop)]
            {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut last_seq: u64 = 0;
                    let mut last_emit = Instant::now();
                    let mut primed_this_recording = false;
                    let mut last_priming_emit: Option<Instant> = None;

                    loop {
                        // 60Hz-ish. If this is too chatty we can reduce to 30Hz later.
                        tokio::time::sleep(Duration::from_millis(16)).await;

                        let Some(pipeline) = app_handle.try_state::<pipeline::SharedPipeline>() else {
                            continue;
                        };

                        // Prefer a non-blocking state check: during pipeline start-up the
                        // mutex may be held while CPAL capture already begins.
                        //
                        // If we can prove we're NOT recording, don't publish. Otherwise,
                        // allow publishing once the meter seq starts moving.
                        if let Some(state) = pipeline.try_state() {
                            if state != pipeline::PipelineState::Recording {
                                last_seq = 0;
                                primed_this_recording = false;
                                last_priming_emit = None;
                                continue;
                            }
                        }

                        // Read the latest snapshots without locking the pipeline.
                        // Drive emission from the level meter so the overlay stays alive
                        // even if waveform buckets are temporarily unavailable.
                        let levels = pipeline.audio_level_snapshot_fast();

                        // If the capture stream has started but we haven't seen a callback yet,
                        // send a one-time "priming" event so the overlay can render immediately
                        // (baseline waveform) instead of waiting for the first buffer.
                        if levels.seq == 0 {
                            // Haven't observed any callbacks yet.
                            // Keep sending priming frames for a short while so the overlay
                            // doesn't miss the first event if its listener registers late.
                            let should_emit = match last_priming_emit {
                                None => true,
                                Some(t) => t.elapsed() >= Duration::from_millis(50),
                            };
                            if should_emit {
                                last_priming_emit = Some(Instant::now());
                                primed_this_recording = true;
                                let payload = serde_json::json!({
                                    "seq": 0,
                                    "rms": 0.0,
                                    "peak": 0.0,
                                    "wave_seq": 0,
                                    "mins": Vec::<f32>::new(),
                                    "maxes": Vec::<f32>::new(),
                                });
                                if let Some(overlay) = app_handle.get_webview_window("overlay") {
                                    let _ = overlay.emit("overlay-audio-level", payload);
                                } else {
                                    let _ = app_handle.emit("overlay-audio-level", payload);
                                }
                            }
                            continue;
                        }

                        if levels.seq == last_seq {
                            continue;
                        }
                        last_seq = levels.seq;
                        primed_this_recording = true;

                        // Waveform buckets (may be all-zeros early or on some devices).
                        let wave = pipeline.audio_waveform_snapshot_fast();

                        // Throttle slightly if needed (defensive).
                        if last_emit.elapsed() < Duration::from_millis(8) {
                            continue;
                        }
                        last_emit = Instant::now();

                        // Emit directly to the overlay window when available.
                        // This avoids any ambiguity around app-wide vs window event targets.
                        let payload = serde_json::json!({
                            "seq": levels.seq,
                            "rms": levels.rms,
                            "peak": levels.peak,
                            "wave_seq": wave.seq,
                            "mins": wave.mins,
                            "maxes": wave.maxes,
                        });
                        if let Some(overlay) = app_handle.get_webview_window("overlay") {
                            let _ = overlay.emit("overlay-audio-level", payload);
                        } else {
                            let _ = app_handle.emit("overlay-audio-level", payload);
                        }
                    }
                });
            }

            // Register shortcuts from store (now that store plugin is available)
            #[cfg(desktop)]
            {
                register_initial_shortcuts(app.handle())?;
            }

            // Create overlay window
            let overlay = tauri::WebviewWindowBuilder::new(
                app,
                "overlay",
                tauri::WebviewUrl::App("overlay.html".into()),
            )
            .title("Tangerine Overlay")
            .inner_size(48.0, 48.0)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .focused(false)
            .focusable(false)
            .accept_first_mouse(true)
            .visible(true)
            .visible_on_all_workspaces(true)
            .background_throttling(BackgroundThrottlingPolicy::Disabled)
            .build()?;

            // On macOS, convert to NSPanel for better fullscreen app behavior
            #[cfg(target_os = "macos")]
            {
                use tauri_nspanel::{CollectionBehavior, PanelLevel, WebviewWindowExt};
                match overlay.to_panel::<OverlayPanel>() {
                    Ok(panel) => {
                        // Configure panel to float above fullscreen apps
                        panel.set_level(PanelLevel::ScreenSaver.value());
                        panel.set_floating_panel(true);

                        // Set collection behavior to appear on all spaces including fullscreen
                        let behavior = CollectionBehavior::new()
                            .can_join_all_spaces()
                            .full_screen_auxiliary();
                        panel.set_collection_behavior(behavior.value());

                        // Set style mask to non-activating panel
                        let style = tauri_nspanel::StyleMask::empty().nonactivating_panel();
                        panel.set_style_mask(style.value());

                        log::info!("[NSPanel] Successfully converted overlay to NSPanel");
                    }
                    Err(e) => {
                        log::error!("[NSPanel] Failed to convert overlay to NSPanel: {:?}", e);
                    }
                }
            }

            // Position overlay based on saved setting
            if let Ok(Some(monitor)) = overlay.current_monitor() {
                let size = monitor.size();
                let scale = monitor.scale_factor();
                let screen_width = size.width as f64 / scale;
                let screen_height = size.height as f64 / scale;

                // Estimate initial widget size (before content loads). The frontend will
                // auto-resize after mount, but using a closer estimate prevents off-screen drift.
                let window_width = 264.0;
                let window_height = 56.0;
                let margin = 50.0;

                let widget_position: String = get_setting_from_store(
                    app.handle(),
                    "widget_position",
                    "bottom-center".to_string(),
                );

                let (x, y) = match widget_position.as_str() {
                    "top-left" => (margin, margin),
                    "top-center" => ((screen_width - window_width) / 2.0, margin),
                    "top-right" => (screen_width - window_width - margin, margin),
                    "center" => (
                        (screen_width - window_width) / 2.0,
                        (screen_height - window_height) / 2.0,
                    ),
                    "bottom-left" => (margin, screen_height - window_height - margin),
                    "bottom-center" => (
                        (screen_width - window_width) / 2.0,
                        screen_height - window_height - margin,
                    ),
                    _ => ( // "bottom-right" or unknown
                        screen_width - window_width - margin,
                        screen_height - window_height - margin,
                    ),
                };

                let _ = overlay.set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
            }

            // Set initial overlay visibility based on saved settings
            #[cfg(desktop)]
            {
                let overlay_mode: String = get_setting_from_store(
                    app.handle(),
                    "overlay_mode",
                    "recording_only".to_string(),
                );
                match overlay_mode.as_str() {
                    "never" | "recording_only" => {
                        let _ = overlay.hide();
                    }
                    _ => {} // "always" - keep visible (default)
                }
            }

            // Setup system tray
            setup_tray(app.handle())?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let show_item = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

    // Use the same tray icon everywhere (full-color, brand-consistent).
    // NOTE: Some platforms (notably macOS) have UI conventions around template icons,
    // but we intentionally keep it consistent with the rest of the app branding.
    let icon_bytes = include_bytes!("../icons/32x32.png");
    let icon = tauri::image::Image::from_bytes(icon_bytes)?;

    let _tray = TrayIconBuilder::new()
        .icon(icon)
        .icon_as_template(false)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "quit" => {
                // Emit disconnect request to frontend before exiting
                if let Some(window) = app.get_webview_window("overlay") {
                    let _ = window.emit("request-disconnect", ());
                }
                // Give frontend time to disconnect gracefully
                std::thread::sleep(std::time::Duration::from_millis(500));
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    if window.is_visible().unwrap_or(false) {
                        let _ = window.hide();
                    } else {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            }
        })
        .build(app)?;

    Ok(())
}

#[cfg(desktop)]
fn build_global_shortcut_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    // Just initialize the plugin - shortcuts will be registered in setup() after store is available
    tauri_plugin_global_shortcut::Builder::new().build()
}

/// Initialize the recording pipeline from settings in the store
#[cfg(desktop)]
fn initialize_pipeline_from_settings(app: &AppHandle) -> pipeline::SharedPipeline {
    use std::time::Duration;
    use std::collections::HashMap;

    // Read STT settings from store
    let stt_provider: String = get_setting_from_store(app, "stt_provider", "groq".to_string());

    // Read STT model from store
    let stt_model: Option<String> = get_setting_from_store(app, "stt_model", None);

    // Read global STT transcription prompt from store
    let stt_transcription_prompt: Option<String> =
        get_setting_from_store(app, "stt_transcription_prompt", None);

    // Read STT timeout from store (seconds)
    let stt_timeout_seconds_raw: f64 = get_setting_from_store(app, "stt_timeout_seconds", 10.0);
    let stt_timeout_seconds: f64 = if stt_timeout_seconds_raw.is_finite() && stt_timeout_seconds_raw > 0.0 {
        stt_timeout_seconds_raw
    } else {
        log::warn!(
            "Invalid stt_timeout_seconds value in store ({}); falling back to 10s",
            stt_timeout_seconds_raw
        );
        10.0
    };

    // Read all available STT API keys (for per-profile provider overrides at runtime)
    let mut stt_api_keys: HashMap<String, String> = HashMap::new();
    for provider in ["openai", "groq", "deepgram"] {
        let key_name = format!("{}_api_key", provider);
        let key: String = get_setting_from_store(app, &key_name, String::new());
        if !key.is_empty() {
            stt_api_keys.insert(provider.to_string(), key);
        }
    }

    // Get the appropriate API key based on provider
    let stt_api_key: String = match stt_provider.as_str() {
        "openai" => get_setting_from_store(app, "openai_api_key", String::new()),
        "groq" => get_setting_from_store(app, "groq_api_key", String::new()),
        "deepgram" => get_setting_from_store(app, "deepgram_api_key", String::new()),
        _ => String::new(),
    };

    // Read VAD settings from store
    let vad_settings: settings::VadSettings =
        get_setting_from_store(app, "vad_settings", settings::VadSettings::default());

    // Read quiet-audio gate settings from store
    let default_pipeline_config = pipeline::PipelineConfig::default();

    let sanitize_quiet_duration_secs = |v: f32, fallback: f32| -> f32 {
        if !v.is_finite() {
            return fallback;
        }
        // UI clamps to a small range; keep this defensive in case the store was edited.
        if v < 0.0 {
            return fallback;
        }
        v.min(30.0)
    };

    let sanitize_quiet_dbfs_threshold = |v: f32, fallback: f32| -> f32 {
        if !v.is_finite() {
            return fallback;
        }
        // dBFS thresholds should be negative. If the store contains 0 or a positive number,
        // the quiet gate would (almost) always trigger.
        if v > -1.0 {
            return fallback;
        }
        v.clamp(-120.0, -1.0)
    };
    let quiet_audio_gate_enabled: bool = get_setting_from_store(
        app,
        "quiet_audio_gate_enabled",
        default_pipeline_config.quiet_audio_gate_enabled,
    );
    let quiet_audio_min_duration_secs: f32 = get_setting_from_store(
        app,
        "quiet_audio_min_duration_secs",
        default_pipeline_config.quiet_audio_min_duration_secs,
    );
    let quiet_audio_rms_dbfs_threshold: f32 = get_setting_from_store(
        app,
        "quiet_audio_rms_dbfs_threshold",
        default_pipeline_config.quiet_audio_rms_dbfs_threshold,
    );
    let quiet_audio_peak_dbfs_threshold: f32 = get_setting_from_store(
        app,
        "quiet_audio_peak_dbfs_threshold",
        default_pipeline_config.quiet_audio_peak_dbfs_threshold,
    );

    let quiet_audio_min_duration_secs =
        sanitize_quiet_duration_secs(quiet_audio_min_duration_secs, default_pipeline_config.quiet_audio_min_duration_secs);
    let quiet_audio_rms_dbfs_threshold = sanitize_quiet_dbfs_threshold(
        quiet_audio_rms_dbfs_threshold,
        default_pipeline_config.quiet_audio_rms_dbfs_threshold,
    );
    let quiet_audio_peak_dbfs_threshold = sanitize_quiet_dbfs_threshold(
        quiet_audio_peak_dbfs_threshold,
        default_pipeline_config.quiet_audio_peak_dbfs_threshold,
    );

    // Read experimental noise gate settings from store
    let noise_gate_strength_raw: u64 =
        get_setting_from_store(app, "noise_gate_strength", 0u64);
    let noise_gate_strength: u8 = noise_gate_strength_raw.min(100) as u8;

    // Read LLM settings from store
    let rewrite_llm_enabled: bool = get_setting_from_store(app, "rewrite_llm_enabled", false);
    let llm_provider_setting: Option<String> = get_setting_from_store(app, "llm_provider", None);
    let llm_model_setting: Option<String> = get_setting_from_store(app, "llm_model", None);

    // If the user never explicitly selected a model, treat "default" as the provider's
    // concrete default model so request logs can display the exact model used.
    let llm_provider_effective = llm_provider_setting
        .clone()
        .unwrap_or_else(|| "openai".to_string());
    let llm_model_effective: Option<String> = llm_model_setting.or_else(|| {
        if rewrite_llm_enabled {
            llm::default_llm_model_for_provider(llm_provider_effective.as_str())
                .map(|m| m.to_string())
        } else {
            None
        }
    });

    let llm_api_key: String = llm_provider_setting
        .as_deref()
        .map(|provider| {
            let key_name = format!("{}_api_key", provider);
            get_setting_from_store(app, &key_name, String::new())
        })
        .unwrap_or_default();

    // IMPORTANT: `enabled` is only the global toggle. The effective provider/key is resolved
    // per transcription based on the active profile.
    let llm_enabled = rewrite_llm_enabled;

    // Read all available LLM API keys (for per-profile provider overrides at runtime)
    let mut llm_api_keys: HashMap<String, String> = HashMap::new();
    for provider in ["openai", "anthropic", "groq"] {
        let key_name = format!("{}_api_key", provider);
        let key: String = get_setting_from_store(app, &key_name, String::new());
        if !key.is_empty() {
            llm_api_keys.insert(provider.to_string(), key);
        }
    }

    // Read rewrite prompt sections + per-program profiles from store
    //
    // `cleanup_prompt_sections` is treated as overrides on top of the built-in defaults.
    // Each program profile can further override individual sections.
    let cleanup_prompt_sections: Option<settings::CleanupPromptSectionsSetting> =
        get_setting_from_store(app, "cleanup_prompt_sections", None);
    let base_prompts: llm::PromptSections = cleanup_prompt_sections
        .as_ref()
        .map(|o| o.apply_to(&llm::PromptSections::default()))
        .unwrap_or_else(llm::PromptSections::default);

    let rewrite_program_prompt_profiles: Vec<settings::RewriteProgramPromptProfile> =
        get_setting_from_store(app, "rewrite_program_prompt_profiles", Vec::new());

    let program_prompt_profiles: Vec<llm::ProgramPromptProfile> = rewrite_program_prompt_profiles
        .into_iter()
        .map(|p| llm::ProgramPromptProfile {
            id: p.id,
            name: p.name,
            program_paths: p.program_paths,
            prompts: p
                .cleanup_prompt_sections
                .as_ref()
                .map(|o| o.apply_to(&base_prompts))
                .unwrap_or_else(|| base_prompts.clone()),
            rewrite_llm_enabled: p.rewrite_llm_enabled,
            stt_provider: p.stt_provider,
            stt_model: p.stt_model,
            stt_timeout_seconds: p.stt_timeout_seconds,
            llm_provider: p.llm_provider,
            llm_model: p.llm_model,
        })
        .collect();

    // Microphone selection (backend / CPAL).
    // Historical key name is `selected_mic_id` (originally from browser deviceId).
    // We now treat it as a CPAL device name for backend recording + overlay waveform.
    let input_device_name: Option<String> = {
        let raw: Option<String> = get_setting_from_store(app, "selected_mic_id", None);
        raw.and_then(|s| {
            let t = s.trim().to_string();
            if t.is_empty() || t == "default" {
                None
            } else {
                Some(t)
            }
        })
    };

    let config = pipeline::PipelineConfig {
        input_device_name,
        stt_provider,
        stt_api_key,
        stt_api_keys,
        stt_model,
        stt_transcription_prompt,
        max_duration_secs: 300.0,
        retry_config: stt::RetryConfig::default(),
        vad_config: vad_settings.to_vad_auto_stop_config(),
        transcription_timeout: Duration::from_secs_f64(stt_timeout_seconds),
        max_recording_bytes: 50 * 1024 * 1024, // 50MB

        quiet_audio_gate_enabled,
        quiet_audio_min_duration_secs,
        quiet_audio_rms_dbfs_threshold,
        quiet_audio_peak_dbfs_threshold,

        noise_gate_strength,

        llm_config: llm::LlmConfig {
            enabled: llm_enabled,
            provider: llm_provider_effective,
            api_key: llm_api_key,
            model: llm_model_effective,
            prompts: base_prompts,
            program_prompt_profiles,
            ..Default::default()
        },
        llm_api_keys,
    };

    log::info!(
        "Initializing pipeline with STT provider: {}, VAD enabled: {}",
        config.stt_provider,
        config.vad_config.enabled
    );

    pipeline::SharedPipeline::new(config)
}

/// Register shortcuts from store settings (called from setup() after store plugin is available)
#[cfg(desktop)]
fn register_initial_shortcuts(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    // Read hotkeys from store with defaults
    let toggle_hotkey: HotkeyConfig =
        get_setting_from_store(app, "toggle_hotkey", HotkeyConfig::default_toggle());
    let hold_hotkey: HotkeyConfig =
        get_setting_from_store(app, "hold_hotkey", HotkeyConfig::default_hold());
    let paste_last_hotkey: HotkeyConfig =
        get_setting_from_store(app, "paste_last_hotkey", HotkeyConfig::default_paste_last());

    // Convert to shortcuts with validation (fall back to defaults if invalid)
    let toggle_shortcut = toggle_hotkey.to_shortcut_or_default(HotkeyConfig::default_toggle);
    let hold_shortcut = hold_hotkey.to_shortcut_or_default(HotkeyConfig::default_hold);
    let paste_last_shortcut =
        paste_last_hotkey.to_shortcut_or_default(HotkeyConfig::default_paste_last);

    log::info!(
        "Registering shortcuts - Toggle: {}, Hold: {}, PasteLast: {}",
        toggle_hotkey.to_shortcut_string(),
        hold_hotkey.to_shortcut_string(),
        paste_last_hotkey.to_shortcut_string()
    );

    let shortcuts: Vec<Shortcut> = vec![toggle_shortcut, hold_shortcut, paste_last_shortcut];

    app.global_shortcut()
        .on_shortcuts(shortcuts, |app, shortcut, event| {
            handle_shortcut_event(app, shortcut, &event);
        })?;

    log::info!("Shortcuts registered successfully");
    Ok(())
}
