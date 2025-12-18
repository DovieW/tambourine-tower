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
mod request_log;
mod settings;
mod state;
mod stt;
mod vad;

#[cfg(test)]
mod tests;

use audio_mute::AudioMuteManager;
use history::HistoryStorage;
use request_log::RequestLogStore;
use settings::HotkeyConfig;
use state::AppState;

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

/// Filter out common Whisper hallucinations from transcription
/// These are phrases that Whisper generates for silent or unclear audio
fn filter_whisper_hallucinations(transcript: &str) -> Option<String> {
    let trimmed = transcript.trim();

    // Common Whisper hallucinations (case-insensitive)
    let hallucinations = [
        "thank you",
        "thanks for watching",
        "thanks for listening",
        "see you next time",
        "bye",
        "goodbye",
        "see you",
        "the end",
        "...",
        "you",
        "i'm sorry",
        "sorry",
        "okay",
        "ok",
        "yes",
        "no",
        "um",
        "uh",
        "hmm",
        "mm",
        "ah",
        "oh",
        "huh",
        "silence",
        "[silence]",
        "(silence)",
        "[music]",
        "(music)",
        "[applause]",
        "(applause)",
        "[laughter]",
        "(laughter)",
        "please subscribe",
        "like and subscribe",
    ];

    let lower = trimmed.to_lowercase();

    // Check if the entire transcript matches a hallucination phrase
    for phrase in hallucinations {
        // Match exact phrase or phrase with trailing punctuation
        if lower == phrase
            || lower == format!("{}.", phrase)
            || lower == format!("{}!", phrase)
            || lower == format!("{}?", phrase)
            || lower == format!("{}...", phrase)
        {
            log::info!("Filtered Whisper hallucination: '{}'", trimmed);
            return None;
        }
    }

    // Return the original (with whitespace trimmed) if not a hallucination
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Start recording with sound and audio mute handling
#[cfg(desktop)]
fn start_recording(
    app: &AppHandle,
    state: &AppState,
    sound_enabled: bool,
    audio_mute_manager: &Option<tauri::State<'_, AudioMuteManager>>,
    auto_mute_audio: bool,
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
            let _ = app.emit("pipeline-error", error_msg);
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
    if auto_mute_audio {
        if let Some(manager) = audio_mute_manager {
            if let Err(e) = manager.mute() {
                log::warn!("Failed to mute audio: {}", e);
            }
        }
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
    auto_mute_audio: bool,
    source: &str,
) {
    state.is_recording.store(false, Ordering::SeqCst);
    log::info!("{}: stopping recording", source);
    emit_system_event(app, "shortcut", &format!("{}: stopping recording", source), None);
    // Unmute system audio if it was muted
    if auto_mute_audio {
        if let Some(manager) = audio_mute_manager {
            if let Err(e) = manager.unmute() {
                log::warn!("Failed to unmute audio: {}", e);
            }
        }
    }
    if sound_enabled {
        audio::play_sound(audio::SoundType::RecordingStop);
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
        tauri::async_runtime::spawn(async move {
            let _ = app_clone.emit("pipeline-transcription-started", ());

            // Log transcription start for this request
            if let Some(log_store) = app_clone.try_state::<RequestLogStore>() {
                log_store.with_current(|log| {
                    log.info("Recording stopped, starting transcription");
                });
            }

            match pipeline_clone.stop_and_transcribe_detailed().await {
                Ok(result) => {
                    log::info!("Transcription complete: {} chars", result.final_text.len());

                    // Filter out common Whisper hallucinations (applied to the pipeline's final text)
                    let filtered_transcript = filter_whisper_hallucinations(&result.final_text);

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
                                log.warn("Transcript filtered as hallucination, nothing output");
                            }

                            log.complete_success();
                        });
                        log_store.complete_current();
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
                        if let Some(history) = app_clone.try_state::<HistoryStorage>() {
                            if let Err(e) = history.add_entry(text.clone()) {
                                log::warn!("Failed to save to history: {}", e);
                            }
                        }
                    } else {
                        // Emit empty transcript event so UI can update appropriately
                        let _ = app_clone.emit("pipeline-transcript-ready", "");
                        log::info!("Transcript filtered as hallucination, not outputting");
                    }

                    // Hide overlay after transcription completes if in "recording_only" mode
                    if overlay_mode_clone == "recording_only" {
                        if let Some(window) = app_clone.get_webview_window("overlay") {
                            let _ = window.hide();
                        }
                    }
                }
                Err(e) => {
                    log::error!("Transcription failed: {}", e);
                    let _ = app_clone.emit("pipeline-error", e.to_string());

                    if let Some(log_store) = app_clone.try_state::<RequestLogStore>() {
                        log_store.with_current(|log| {
                            log.error(format!("Transcription failed: {}", e));
                            log.complete_error(e.to_string());
                        });
                        log_store.complete_current();
                    }

                    // Hide overlay even on error if in "recording_only" mode
                    if overlay_mode_clone == "recording_only" {
                        if let Some(window) = app_clone.get_webview_window("overlay") {
                            let _ = window.hide();
                        }
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
    let auto_mute_audio: bool = get_setting_from_store(app, "auto_mute_audio", false);

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
                            auto_mute_audio,
                            "Toggle",
                        );
                    } else {
                        start_recording(
                            app,
                            &state,
                            sound_enabled,
                            &audio_mute_manager,
                            auto_mute_audio,
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
                            auto_mute_audio,
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
                            auto_mute_audio,
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
        ])
        .setup(|app| {
            // Initialize history storage
            let app_data_dir = app
                .path()
                .app_data_dir()
                .expect("Failed to get app data directory");

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

                // Assume initial window size of 48x48 (before content loads)
                let window_width = 48.0;
                let window_height = 48.0;
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

    // Read STT settings from store
    let stt_provider: String = get_setting_from_store(app, "stt_provider", "groq".to_string());

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

    // Read LLM settings from store
    let llm_provider: Option<String> = get_setting_from_store(app, "llm_provider", None);
    let llm_model: Option<String> = get_setting_from_store(app, "llm_model", None);

    let llm_api_key: String = llm_provider
        .as_deref()
        .map(|provider| {
            let key_name = format!("{}_api_key", provider);
            get_setting_from_store(app, &key_name, String::new())
        })
        .unwrap_or_default();

    let llm_enabled = match llm_provider.as_deref() {
        None => false,
        Some("ollama") => true,
        Some(_) => !llm_api_key.is_empty(),
    };

    let config = pipeline::PipelineConfig {
        stt_provider,
        stt_api_key,
        stt_model: None,
        max_duration_secs: 300.0,
        retry_config: stt::RetryConfig::default(),
        vad_config: vad_settings.to_vad_auto_stop_config(),
        transcription_timeout: Duration::from_secs(60),
        max_recording_bytes: 50 * 1024 * 1024, // 50MB
        llm_config: llm::LlmConfig {
            enabled: llm_enabled,
            provider: llm_provider.unwrap_or_else(|| "openai".to_string()),
            api_key: llm_api_key,
            model: llm_model,
            ..Default::default()
        },
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
