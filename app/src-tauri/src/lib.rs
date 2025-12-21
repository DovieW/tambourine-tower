use std::sync::atomic::Ordering;
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
use history::HistoryStorage;
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
    set_if_missing("stt_timeout_seconds", json!(60.0));
    set_if_missing("overlay_mode", json!("always"));
    set_if_missing("widget_position", json!("bottom-right"));
    set_if_missing("output_mode", json!("paste"));
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

    // Pipeline started successfully - now update state and do side effects
    state.is_recording.store(true, Ordering::SeqCst);

    // Show overlay if in "recording_only" mode
    let overlay_mode: String = get_setting_from_store(app, "overlay_mode", "always".to_string());
    if overlay_mode == "recording_only" {
        if let Some(window) = app.get_webview_window("overlay") {
            let _ = window.show();
        }
    }

    // Play sound BEFORE muting so it's audible
    if sound_enabled {
        audio::play_sound(audio::SoundType::RecordingStart);
        // Brief delay to let sound play before muting
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    // Mute system audio if enabled
    if playing_audio_handling.wants_mute() {
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

    let _ = app.emit("recording-start", ());
}

/// Stop recording with sound and audio unmute handling
#[cfg(desktop)]
fn stop_recording(
    app: &AppHandle,
    state: &AppState,
    sound_enabled: bool,
    audio_mute_manager: &Option<tauri::State<'_, AudioMuteManager>>,
    playing_audio_handling: PlayingAudioHandling,
    source: &str,
) {
    state.is_recording.store(false, Ordering::SeqCst);
    log::info!("{}: stopping recording", source);
    emit_system_event(app, "shortcut", &format!("{}: stopping recording", source), None);
    // Unmute system audio if it was muted
    if playing_audio_handling.wants_mute() {
        if let Some(manager) = audio_mute_manager {
            if let Err(e) = manager.unmute() {
                log::warn!("Failed to unmute audio: {}", e);
            }
        }
    }
    if sound_enabled {
        audio::play_sound(audio::SoundType::RecordingStop);
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
    let overlay_mode: String = get_setting_from_store(app, "overlay_mode", "always".to_string());

    // Get output mode for how to output text
    let output_mode_str: String = get_setting_from_store(app, "output_mode", "paste".to_string());
    let output_mode = commands::text::OutputMode::from_str(&output_mode_str);

    // Stop pipeline and trigger transcription in background
    if let Some(pipeline) = app.try_state::<pipeline::SharedPipeline>() {
        let pipeline_clone = (*pipeline).clone();
        let app_clone = app.clone();
        let overlay_mode_clone = overlay_mode.clone();

        // Capture current request id (for history + retry audio).
        let request_id: Option<String> = app
            .try_state::<RequestLogStore>()
            .and_then(|store| store.with_current(|log| log.id.clone()));

        tauri::async_runtime::spawn(async move {
            let _ = app_clone.emit("pipeline-transcription-started", ());

            // Create an in-progress history entry while we transcribe.
            if let Some(ref req_id) = request_id {
                if let Some(history) = app_clone.try_state::<HistoryStorage>() {
                    let _ = history.add_request_entry(req_id.clone());
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
                            let _ = store.save_wav(req_id, &wav);
                        }
                    }

                    if let Some(ref text) = filtered_transcript {
                        let _ = app_clone.emit("pipeline-transcript-ready", text);

                        // Output the transcript based on mode
                        if let Err(e) = commands::text::output_text_with_mode(text, output_mode) {
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
                                let current_mode: String = get_setting_from_store(&app_check, "overlay_mode", "always".to_string());
                                if current_mode == "recording_only" {
                                    let _ = window_clone.hide();
                                }
                            });
                        }
                    }
                }
                Err(e) => {
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
                            let _ = store.save_wav(req_id, &wav);
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
                    if let Some(window) = app_clone.get_webview_window("overlay") {
                        let _ = window.show();
                    }

                }
            }
        });
    }

    let _ = app.emit("recording-stop", ());
}

/// Handle a shortcut event - public so it can be called from commands/settings.rs
#[cfg(desktop)]
pub fn handle_shortcut_event(app: &AppHandle, shortcut: &Shortcut, event: &ShortcutEvent) {
    let state = app.state::<AppState>();

    // Get current settings from store
    let sound_enabled: bool = get_setting_from_store(app, "sound_enabled", true);
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
                            &audio_mute_manager,
                            playing_audio_handling,
                            "Toggle",
                        );
                    } else {
                        start_recording(
                            app,
                            &state,
                            sound_enabled,
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

                    let history_storage = app.state::<HistoryStorage>();

                    if let Ok(entries) = history_storage.get_all(Some(1)) {
                        if let Some(entry) = entries.first() {
                            if let Err(e) = commands::text::output_text_with_mode(&entry.text, output_mode) {
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
            .title("Voice Overlay")
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

                let widget_position: String = get_setting_from_store(app.handle(), "widget_position", "bottom-right".to_string());

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
                let overlay_mode: String = get_setting_from_store(app.handle(), "overlay_mode", "always".to_string());
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

    // Load the template icon for macOS menu bar
    // The @2x version is automatically used for retina displays
    let icon_bytes = include_bytes!("../icons/tray-iconTemplate@2x.png");
    let icon = tauri::image::Image::from_bytes(icon_bytes)?;

    let _tray = TrayIconBuilder::new()
        .icon(icon)
        .icon_as_template(true)
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

    // Read STT timeout from store (seconds)
    let stt_timeout_seconds_raw: f64 = get_setting_from_store(app, "stt_timeout_seconds", 60.0);
    let stt_timeout_seconds: f64 = if stt_timeout_seconds_raw.is_finite() && stt_timeout_seconds_raw > 0.0 {
        stt_timeout_seconds_raw
    } else {
        log::warn!(
            "Invalid stt_timeout_seconds value in store ({}); falling back to 60s",
            stt_timeout_seconds_raw
        );
        60.0
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

    let config = pipeline::PipelineConfig {
        stt_provider,
        stt_api_key,
        stt_api_keys,
        stt_model,
        max_duration_secs: 300.0,
        retry_config: stt::RetryConfig::default(),
        vad_config: vad_settings.to_vad_auto_stop_config(),
        transcription_timeout: Duration::from_secs_f64(stt_timeout_seconds),
        max_recording_bytes: 50 * 1024 * 1024, // 50MB

        quiet_audio_gate_enabled,
        quiet_audio_min_duration_secs,
        quiet_audio_rms_dbfs_threshold,
        quiet_audio_peak_dbfs_threshold,

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
