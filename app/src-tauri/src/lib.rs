use std::sync::atomic::Ordering;

use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_cli::CliExt;
use tracing::Instrument;

// Extraction buckets: settings defaults, overlay wiring, shortcuts lifecycle, bootstrap wiring.

// Re-export core/adapter types that lib.rs and commands need.
#[cfg(desktop)]
pub(crate) use crate::adapters::media_controls::toggle_media_play_pause;
#[cfg(desktop)]
pub(crate) use crate::core::recording::{
    get_playing_audio_handling, start_recording, PlayingAudioHandling,
};

mod active_window_capture;
mod app_paths;
mod audio;
mod audio_capture;
mod audio_mute;
mod audio_normalization;
mod bootstrap;
mod cli;
mod clipboard_context;
mod commands;
mod cost;
mod embeddings;
pub mod events;
mod fs;
mod history;
mod history_request_lifecycle;
mod http;
mod licensing;
mod llm;
mod managed_inference;
mod network;
mod ocr;
mod overlay;
mod pipeline;
mod platform_capabilities;
mod policy;
mod prompt_builders;
mod recording_completion;
mod recording_orchestration;
mod recording_request_initialization;
mod recordings;
mod request_log;
mod router_embeddings_cache;
mod secrets;
mod sentry_init;
mod sessions;
#[path = "settings.rs"]
mod settings;
mod settings_view;
mod shortcuts;
mod shortcuts_lock;
mod state;
mod stats;
mod stt;
mod telemetry;
mod text;
mod tracing_init;
mod vad;
mod windows_apps;
mod windows_uia;

mod adapters;
mod core;

mod app_shared;
mod event_payloads;

pub use event_payloads::{
    ConnectionStateChangedPayload, ConnectionStateEvent, EmptyEventPayload,
    OverlayAudioLevelPayload, OverlayOcrContextUnavailablePayload, PipelineErrorPayload,
    PipelineStateEvent, PipelineTranscriptReadyPayload, QuickAskAnswerErrorPayload,
    QuickAskAnswerOkPayload, QuickAskAnswerPayload, QuickAskStartedPayload, SettingsChangedPayload,
    SystemEvent,
};

pub use audio_capture::AudioCaptureDiagnostics;
pub use audio_capture::AudioLevelStats;
pub use commands::audio::MicTestAudioLevelPayload;
pub use commands::config::AvailableProvidersResponse;
pub use commands::config::DefaultSectionsResponse;
pub use commands::config::ProviderInfo;
pub use commands::data::DataStorageSummary;
pub use commands::fireworks::ModelOption;
pub use commands::history::HistoryDeleteMode;
pub use commands::history::HistoryDeleteOptions;
pub use commands::history::HistoryDeleteResult;
pub use commands::llm::IterateRewritePromptResponse;
pub use commands::llm::LlmCompleteResponse;
pub use commands::llm::LlmProviderInfo;
pub use commands::llm::TestLlmRewriteResponse;
pub use commands::llm::TestRewriteWithPromptResponse;
pub use commands::network::SystemProxyInfo;
pub use commands::network::WindowsInternetProxySettings;
pub use commands::pricing::LlmModelPricing;
pub use commands::pricing::ModelPricingResponse;
pub use commands::pricing::SttModelPricing;
pub use commands::recording::AudioSettingsTestWavs;
pub use commands::router::CacheRouterEmbeddingsResponse;
pub use commands::stats::CostByProviderResponse;
pub use commands::stats::CostSummaryResponse;
pub use commands::stats::ProviderCostTotal;
pub use commands::whisper::LocalWhisperBackendStatusResponse as LocalWhisperBackendStatus;
pub use commands::whisper::LocalWhisperComputeBackend;
pub use commands::whisper::LocalWhisperModelLoadEvent;
pub use commands::whisper::LocalWhisperModelLoadStatus;
pub use commands::whisper::WhisperModelDownloadProgress;
pub use commands::whisper::WhisperModelDownloadStatus;
pub use commands::whisper::WhisperModelInfo;
pub use history::HistoryPageQuery;
pub use history::HistoryPageResult;
pub use history::{HistoryDetail, HistoryEdit, HistoryEditInput};
pub use recordings::options::{RecordingMode, RecordingPreferences};
pub use recordings::RecordingWaveform;
pub use recordings::RecordingsStats;
pub use request_log::RequestLog;
pub use settings::HotkeyConfig;
pub use settings::IntentRouterSettings;
pub use settings::ProxySettings;
pub use settings::RewritePreset;
pub use settings::RewriteProgramPromptProfile;
pub use windows_apps::OpenWindowInfo;

#[cfg(target_os = "windows")]
mod windows_modifier_hotkeys;

#[cfg(test)]
mod tests;

use audio_mute::AudioMuteManager;
use history::{HistoryStorage, RequestModelInfo};
use recordings::RecordingStore;
use request_log::RequestLogStore;
use state::{AppState, MicTestMeterState, QuickAskConversationMemory, TrayKeepAlive};

#[cfg(desktop)]
pub(crate) use shortcuts::{cancel_pipeline_session, set_escape_cancel_shortcut_enabled};

#[cfg(desktop)]
pub(crate) use app_shared::{emit_system_event, get_setting_from_store};

pub(crate) use app_shared::sanitize_transcript;

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

/// Stop recording with sound and audio unmute handling
#[cfg(desktop)]
pub(crate) fn stop_recording(
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
    emit_system_event(
        app,
        "shortcut",
        &format!("{}: stopping recording", source),
        None,
    );

    // If this recording was started via the Quick Ask hotkey, branch the post-transcription
    // flow into an LLM answer overlay (instead of output/paste).
    let is_quick_ask_session = state.quick_ask_session_active.swap(false, Ordering::SeqCst);

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
    // Note: we unmute above, so muted-during-recording should not suppress the stop cue.
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

    // Windows-only: capture a focused target snapshot near recording stop.
    #[cfg(target_os = "windows")]
    {
        if let Ok(snapshot) = crate::windows_uia::snapshot::capture_focused_snapshot() {
            if let Ok(mut guard) = state.windows_text_target_snapshot.lock() {
                *guard = Some(snapshot);
            }
        }
    }

    // Resolve the program profile id captured at recording start (before overlays can steal focus).
    // We "take" it (read and clear) so it can't leak across sessions.
    let session_profile_id: Option<String> = state
        .recording_session_profile_id
        .lock()
        .ok()
        .and_then(|mut g| g.take());

    // Apply per-program output overrides (if configured in settings UI).
    // IMPORTANT: We only apply these when we have a real matched program profile id.
    // The explicit "default" marker is for UI/log semantics and should not change runtime behavior.
    let output_intent = {
        let (mut profile_output_mode, mut profile_output_hit_enter) = (None::<String>, None);
        if let Some(pid) = session_profile_id.as_deref() {
            if pid != "default" {
                let profiles: Vec<crate::settings::RewriteProgramPromptProfile> =
                    get_setting_from_store(app, "rewrite_program_prompt_profiles", Vec::new());
                if let Some(p) = profiles.iter().find(|p| p.id == pid) {
                    profile_output_mode = p.output_mode.clone();
                    profile_output_hit_enter = p.output_hit_enter;
                }
            }
        }

        crate::core::output_settings::resolve_output_intent_from_store(
            app,
            profile_output_mode.as_deref(),
            profile_output_hit_enter,
        )
    };

    // Stop pipeline and trigger transcription in background
    if let Some(pipeline) = app.try_state::<pipeline::SharedPipeline>() {
        let pipeline_clone = (*pipeline).clone();
        let app_clone = app.clone();
        let overlay_mode_clone = overlay_mode.clone();

        // Capture model info from pipeline config for persistence in history.
        let config = pipeline.config();
        let request_profile_context = pipeline::resolve_request_profile_context(
            &config.llm_config,
            session_profile_id.as_deref(),
            pipeline::select_profile_for_foreground_app(&config.llm_config),
            pipeline::ActiveWindowOcrModeFallbacks {
                rewrite: config.ocr_config.rewrite_mode.as_str(),
                quick_ask: config.ocr_config.quick_ask_mode.as_str(),
                quick_replace: config.ocr_config.quick_replace_mode.as_str(),
            },
            pipeline::DefaultProfileSelectionPolicy::KeepDefaultAsFallbackOnly,
        );
        let profile: Option<crate::llm::ProgramPromptProfile> =
            request_profile_context.active_profile().cloned();

        let model_info = RequestModelInfo {
            stt_provider: Some(config.stt_provider.clone()),
            stt_model: config.stt_model.clone(),
            llm_provider: if config.llm_config.enabled {
                Some(config.llm_config.provider.clone())
            } else {
                None
            },
            llm_model: config.llm_config.model.clone(),
            profile_id: profile.as_ref().map(|p| p.id.clone()),
            profile_name: profile.as_ref().map(|p| p.name.clone()),
            preset_id: None,
            preset_name: None,
        };

        let default_profile = request_profile_context.default_profile().cloned();

        let ocr_config = config.ocr_config.clone();

        let ocr_modes = request_profile_context.ocr_modes().clone();
        let rewrite_ocr_mode = ocr_modes.rewrite().to_string();
        let quick_ask_ocr_mode = ocr_modes.quick_ask().to_string();
        let quick_replace_ocr_mode = ocr_modes.quick_replace().to_string();

        // Debug breadcrumbs for OCR gating.
        // This is intentionally high-level (no screenshot bytes, no secrets).
        if let Some(log_store) = app_clone.try_state::<RequestLogStore>() {
            let rewrite_ocr_mode = rewrite_ocr_mode.clone();
            let quick_ask_ocr_mode = quick_ask_ocr_mode.clone();
            let quick_replace_ocr_mode = quick_replace_ocr_mode.clone();
            log_store.with_current(|log| {
                // Record the effective mode for the active flow.
                log.ocr_effective_mode = Some(
                    ocr_modes
                        .effective_mode_for_session(is_quick_ask_session)
                        .to_string(),
                );

                log.debug(format!(
                    "OCR: effective modes (rewrite={}, quick_ask={}, quick_replace={})",
                    rewrite_ocr_mode, quick_ask_ocr_mode, quick_replace_ocr_mode
                ));
            });
        }

        // IMPORTANT: Only auto-start OCR for the *current* flow.
        // Previously, rewrite_mode == "auto" caused OCR to start even during Quick Ask sessions,
        // which made the tri-state dropdown feel broken (Quick Ask always did OCR work).
        let should_auto_ocr = ocr_modes.should_auto_start(is_quick_ask_session);

        if let Some(log_store) = app_clone.try_state::<RequestLogStore>() {
            let quick_ask_ocr_mode = quick_ask_ocr_mode.clone();
            log_store.with_current(|log| {
                log.debug(format!(
                    "OCR: auto-start decision (is_quick_ask_session={}, should_auto={})",
                    is_quick_ask_session, should_auto_ocr
                ));

                if is_quick_ask_session {
                    // Make the Quick Ask tri-state behavior explicit in the logs.
                    if !should_auto_ocr {
                        log.ocr_status = Some("not_started".to_string());
                        log.ocr_not_attempted_reason = Some(
                            match quick_ask_ocr_mode.as_str() {
                                "off" => "mode_off",
                                "manual" => "mode_manual",
                                _ => "unknown",
                            }
                            .to_string(),
                        );
                        log.info(format!(
                            "OCR: not auto-started for Quick Ask (mode={})",
                            quick_ask_ocr_mode
                        ));
                    }
                }
            });
        }
        // Keep stop-time OCR start policy centralized so normal dictation and Quick Actions do
        // not each drift on what counts as "already started enough" for the current session.
        sessions::ocr_usage::ensure_stop_time_ocr_started(
            &pipeline_clone,
            &ocr_config,
            should_auto_ocr,
        );

        let quick_ask_profile_cfg =
            sessions::quick_action_lifecycle::QuickAskProfileConfig::from_profiles(
                profile.as_ref(),
                default_profile.as_ref(),
            );

        // Context grabbing method (highlighted selection capture).
        // This is a per-profile setting; when unset, we default to Ctrl+C.
        let context_grab_method = sessions::quick_action_lifecycle::resolve_context_grab_method(
            profile.as_ref(),
            default_profile.as_ref(),
        );

        // Backward-compatible fallback to the legacy global key (pre per-profile settings).
        let quick_replace_enabled_legacy: bool =
            get_setting_from_store(app, "quick_replace_enabled", false);

        let quick_replace_cfg = sessions::quick_action_lifecycle::QuickReplaceConfig::resolve(
            profile.as_ref(),
            default_profile.as_ref(),
            &config.llm_config,
            is_quick_ask_session,
            quick_replace_enabled_legacy,
        );

        let recording_intent = sessions::quick_action_lifecycle::RecordingIntent::from_flags(
            is_quick_ask_session,
            quick_replace_cfg.enabled,
        );

        // Quick Replace: probe for currently highlighted text while transcription runs.
        let quick_replace_epoch: u64 = if recording_intent.may_attempt_quick_replace() {
            sessions::selection_probe::spawn_probe(
                app,
                sessions::quick_action_lifecycle::QuickActionKind::QuickReplace.probe_kind(),
                context_grab_method,
            )
        } else {
            0
        };
        let quick_replace_probe_plan = sessions::quick_action_lifecycle::QuickActionProbePlan::new(
            sessions::quick_action_lifecycle::QuickActionKind::QuickReplace,
            quick_replace_epoch,
            context_grab_method,
            sessions::quick_action_lifecycle::DEFAULT_QUICK_ACTION_PROBE_TIMEOUT_MS,
        );

        // Quick Ask: probe for currently highlighted text to use as additional context.
        let quick_ask_include_selected_text: bool =
            get_setting_from_store(app, "quick_ask_include_selected_text", false);

        let quick_ask_epoch: u64 =
            if recording_intent.is_quick_ask() && quick_ask_include_selected_text {
                sessions::selection_probe::spawn_probe(
                    app,
                    sessions::quick_action_lifecycle::QuickActionKind::QuickAsk.probe_kind(),
                    context_grab_method,
                )
            } else {
                0
            };
        let quick_ask_probe_plan = sessions::quick_action_lifecycle::QuickActionProbePlan::new(
            sessions::quick_action_lifecycle::QuickActionKind::QuickAsk,
            quick_ask_epoch,
            context_grab_method,
            sessions::quick_action_lifecycle::DEFAULT_QUICK_ACTION_PROBE_TIMEOUT_MS,
        );

        // Capture current request id (for history + retry audio).
        //
        // In some edge cases, request logging may not have been started at
        // recording-start (e.g., hotkey pressed during startup or other state
        // desync). If so, create a request log now so failures still show up in
        // Request Logs + History.
        let mut request_id: Option<String> = app
            .try_state::<RequestLogStore>()
            .and_then(|store| store.with_current(|log| log.id.clone()));

        if request_id.is_none() {
            if let Some(log_store) = app.try_state::<RequestLogStore>() {
                let id =
                    log_store.start_request(config.stt_provider.clone(), config.stt_model.clone());

                // Tie OCR to this request id (edge case: request log was missing at stop).
                pipeline_clone.begin_ocr_session(id.clone());

                log_store.with_current(|log| {
                    log.profile_id = model_info.profile_id.clone();
                    log.profile_name = model_info.profile_name.clone();
                    log.llm_provider = model_info.llm_provider.clone();
                    // Avoid confusing logs: if LLM rewrite is disabled, do not record an LLM model.
                    log.llm_model = if config.llm_config.enabled {
                        model_info.llm_model.clone()
                    } else {
                        None
                    };
                    log.warn(format!(
                        "Request log was missing at stop; started a new request log entry ({})",
                        source
                    ));
                });
                request_id = Some(id);
            }
        }

        tauri::async_runtime::spawn(async move {
            // Emit transcription started only once the pipeline actually transitions
            // into Transcribing (quiet-audio gate skips should fade out without ever
            // showing "TRANSCRIBING...").
            {
                let app_for_evt = app_clone.clone();
                let pipeline_for_evt = pipeline_clone.clone();
                let audio_cue_for_stop = audio_cue;
                let should_play_stop_sound = play_stop_sound_when_transcribing;
                tauri::async_runtime::spawn(
                    async move {
                        let start = std::time::Instant::now();
                        loop {
                            match pipeline_for_evt.state() {
                                pipeline::PipelineState::Transcribing
                                | pipeline::PipelineState::Rewriting => {
                                    let _ = app_for_evt
                                        .emit(events::EVENT_PIPELINE_TRANSCRIPTION_STARTED, ());
                                    let _ = app_for_evt.emit(
                                        events::EVENT_PIPELINE_STATE_CHANGED,
                                        PipelineStateEvent::Transcribing,
                                    );

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
                                pipeline::PipelineState::Recording
                                | pipeline::PipelineState::Routing => {}
                            }

                            if start.elapsed() > std::time::Duration::from_secs(2) {
                                break;
                            }

                            tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                        }
                    }
                    .in_current_span(),
                );
            }

            // Emit routing started once the pipeline transitions into the Routing phase.
            {
                let app_for_evt = app_clone.clone();
                let pipeline_for_evt = pipeline_clone.clone();
                tauri::async_runtime::spawn(
                    async move {
                        let start = std::time::Instant::now();
                        loop {
                            match pipeline_for_evt.state() {
                                pipeline::PipelineState::Routing => {
                                    let _ = app_for_evt
                                        .emit(events::EVENT_PIPELINE_ROUTING_STARTED, ());
                                    let _ = app_for_evt.emit(
                                        events::EVENT_PIPELINE_STATE_CHANGED,
                                        PipelineStateEvent::Routing,
                                    );
                                    break;
                                }
                                pipeline::PipelineState::Idle | pipeline::PipelineState::Error => {
                                    break;
                                }
                                pipeline::PipelineState::Recording
                                | pipeline::PipelineState::Transcribing
                                | pipeline::PipelineState::Rewriting => {}
                            }

                            if start.elapsed() > std::time::Duration::from_secs(15 * 60) {
                                break;
                            }

                            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                        }
                    }
                    .in_current_span(),
                );
            }

            // Emit rewriting started once the pipeline actually enters the optional LLM phase.
            //
            // This keeps the overlay UI accurate even if state polling is delayed.
            {
                let app_for_evt = app_clone.clone();
                let pipeline_for_evt = pipeline_clone.clone();
                tauri::async_runtime::spawn(
                    async move {
                        let start = std::time::Instant::now();
                        loop {
                            match pipeline_for_evt.state() {
                                pipeline::PipelineState::Rewriting => {
                                    let _ = app_for_evt
                                        .emit(events::EVENT_PIPELINE_REWRITING_STARTED, ());
                                    let _ = app_for_evt.emit(
                                        events::EVENT_PIPELINE_STATE_CHANGED,
                                        PipelineStateEvent::Rewriting,
                                    );
                                    break;
                                }
                                pipeline::PipelineState::Idle | pipeline::PipelineState::Error => {
                                    // No rewrite (disabled/failed early) or pipeline exited.
                                    break;
                                }
                                pipeline::PipelineState::Recording
                                | pipeline::PipelineState::Transcribing
                                | pipeline::PipelineState::Routing => {}
                            }

                            if start.elapsed() > std::time::Duration::from_secs(15 * 60) {
                                break;
                            }

                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        }
                    }
                    .in_current_span(),
                );
            }

            // Create an in-progress history entry while we transcribe.
            // Quick Ask uses a separate UI surface and should not pollute the main dictation history.
            if !is_quick_ask_session {
                if let Some(req_id) = request_id.as_ref() {
                    let _ = history_request_lifecycle::apply_request_history_update(
                        &app_clone,
                        history::RequestHistoryUpdate::CreateInProgress {
                            request_id: req_id.clone(),
                            model_info,
                            max_entries: commands::history::get_history_max_entries(&app_clone),
                        },
                    );
                }
            }

            // Log transcription start for this request
            if let Some(log_store) = app_clone.try_state::<RequestLogStore>() {
                log_store.with_current(|log| {
                    // Do not include recording time in the request "Total" duration.
                    log.mark_processing_started();
                    log.info("Recording stopped, starting transcription");
                });
            }

            let mut complete_request_log_after_output = false;

            match pipeline_clone.stop_and_transcribe_detailed().await {
                Ok(result) => {
                    log::info!("Transcription complete: {} chars", result.final_text.len());

                    // Final output after pipeline (STT + optional LLM) normalization.
                    // Quiet recordings should already have been skipped in the pipeline.
                    let filtered_transcript = sanitize_transcript(&result.final_text);

                    // Update request log store
                    if let Some(log_store) = app_clone.try_state::<RequestLogStore>() {
                        // Like Quick Ask, Quick Replace needs to keep the request log open
                        // until the extra LLM step completes, otherwise the UI won't capture
                        // the additional diagnostics.
                        let should_complete_now = !(is_quick_ask_session
                            || (quick_replace_cfg.enabled
                                && quick_replace_probe_plan.should_await()));

                        let completion_log_message = if should_complete_now {
                            "Transcription completed; output pending"
                        } else if is_quick_ask_session {
                            "Transcription completed; Quick Ask answer pending"
                        } else {
                            // Quick Replace keeps the request log open for its follow-up LLM call,
                            // so avoid labeling that path as Quick Ask in diagnostics.
                            "Transcription completed; Quick Replace rewrite pending"
                        };

                        log_store.with_current(|log| {
                            sessions::recording_finalization::record_transcription_success(
                                log,
                                sessions::recording_finalization::TranscriptionSuccessLogUpdate {
                                    result: &result,
                                    formatted_transcript: filtered_transcript.as_deref(),
                                    audio_duration_secs: None,
                                    audio_size_bytes: None,
                                    stt_summary_label: "STT",
                                    completion_log_message: Some(completion_log_message),
                                    warn_if_no_formatted_transcript: true,
                                },
                            );
                        });

                        if should_complete_now {
                            complete_request_log_after_output = true;
                        }

                        // The pipeline decides preset selection during routing and stores it into
                        // the current RequestLog; mirror it into History so UI badges stay aligned.
                        sessions::recording_finalization::persist_current_request_preset_to_history(
                            &app_clone,
                            request_id.as_deref(),
                        );

                        // Persist cost/usage stats (best-effort).
                        // For Quick Ask sessions we emit stats after the answer step so we can
                        // include the answer LLM details.
                    }

                    // Persist audio for retry (best-effort)
                    if let Some(wav) = pipeline_clone.clone_last_wav_bytes() {
                        let max_saved_recordings: usize =
                            crate::settings::store::get_u64_setting_clamped(
                                &app_clone,
                                crate::settings::store::SettingsReadMode::Cached,
                                "max_saved_recordings",
                                1000u64,
                                1,
                                100_000,
                            ) as usize;

                        if let Err(e) = recording_completion::persist_request_recording(
                            &app_clone,
                            request_id.as_deref(),
                            Some(wav.as_slice()),
                            max_saved_recordings,
                        ) {
                            log::warn!("{}", e);
                        }
                    }

                    if let Some(ref text) = filtered_transcript {
                        let _ = app_clone.emit(events::EVENT_PIPELINE_TRANSCRIPT_READY, text);
                        let _ = app_clone.emit(
                            events::EVENT_PIPELINE_STATE_CHANGED,
                            PipelineStateEvent::Idle,
                        );

                        // Default output is the (possibly rewritten) pipeline transcript.
                        // Quick Replace may overwrite this when a selection is present.
                        let mut output_value = text.clone();

                        // Quick Ask: instead of outputting/pasting the transcript, send it to an LLM
                        // for an answer and show it in a dedicated overlay.
                        if is_quick_ask_session {
                            sessions::quick_action_execution::answer_quick_ask(
                                sessions::quick_action_execution::QuickAskExecution {
                                    app: &app_clone,
                                    pipeline: &pipeline_clone,
                                    request_id: request_id.as_deref(),
                                    result: &result,
                                    fallback_text: text,
                                    profile_config: &quick_ask_profile_cfg,
                                    probe_plan: quick_ask_probe_plan,
                                    ocr_mode: quick_ask_ocr_mode.as_str(),
                                    ocr_config: &ocr_config,
                                },
                            )
                            .await;

                            // Preserve the pre-extraction stop_recording ordering: Quick Ask owns
                            // its request-log/provider lifecycle, then global recording retention
                            // still runs before the overlay-hide follow-up below.
                            sessions::retention::apply_transcription_retention(&app_clone);
                        } else {
                            let quick_replace_result =
                                sessions::quick_action_execution::try_quick_replace(
                                    sessions::quick_action_execution::QuickReplaceExecution {
                                        app: &app_clone,
                                        pipeline: &pipeline_clone,
                                        request_id: request_id.as_deref(),
                                        config: &quick_replace_cfg,
                                        probe_plan: quick_replace_probe_plan,
                                        ocr_mode: quick_replace_ocr_mode.as_str(),
                                        ocr_config: &ocr_config,
                                        output_value,
                                    },
                                )
                                .await;
                            output_value = quick_replace_result.output_value;
                            let quick_replace_failure = quick_replace_result.failure;

                            let normal_output_result =
                                sessions::normal_dictation_output::execute_normal_dictation_output(
                                    &app_clone,
                                    sessions::normal_dictation_output::NormalDictationOutputRequest {
                                        output_value: output_value.as_str(),
                                        output_intent,
                                        live_output_completed: result.live_output_completed,
                                        quick_replace_failure: quick_replace_failure.as_deref(),
                                        request_id: request_id.as_deref(),
                                    },
                                )
                                .await;
                            log::debug!(
                                "Normal dictation output finished (decision={:?}, output_error={})",
                                normal_output_result.decision,
                                normal_output_result.output_error.is_some()
                            );

                            sessions::normal_dictation_output::finalize_normal_dictation_request(
                                &app_clone,
                                sessions::normal_dictation_output::NormalDictationFinalizationRequest {
                                    pipeline: &pipeline_clone,
                                    request_id: request_id.as_deref(),
                                    result: &result,
                                    output_value: output_value.as_str(),
                                    quick_replace_failure: quick_replace_failure.as_deref(),
                                    complete_request_log_after_output,
                                },
                            );
                        }
                    } else {
                        // Emit empty transcript event so UI can update appropriately
                        let _ = app_clone.emit(events::EVENT_PIPELINE_TRANSCRIPT_READY, "");
                        let _ = app_clone.emit(
                            events::EVENT_PIPELINE_STATE_CHANGED,
                            PipelineStateEvent::Idle,
                        );
                        log::info!("No transcript output (empty/whitespace), not outputting");

                        if is_quick_ask_session {
                            sessions::quick_action_execution::complete_quick_ask_empty_transcript_error(
                                &app_clone,
                                &pipeline_clone,
                                request_id.as_deref(),
                            );
                        }

                        // Mark history entry as success with empty text (keeps timeline consistent)
                        if !is_quick_ask_session {
                            if let Some(req_id) = request_id.as_ref() {
                                let _ = history_request_lifecycle::apply_request_history_update(
                                    &app_clone,
                                    history::RequestHistoryUpdate::CompleteSuccess {
                                        request_id: req_id.clone(),
                                        text: String::new(),
                                    },
                                );
                            }

                            sessions::recording_finalization::persist_history_llm_metadata(
                                &app_clone,
                                request_id.as_deref(),
                                &result,
                            );
                        }

                        // Time-based retention (best-effort). This path is used by global shortcuts.
                        sessions::retention::apply_transcription_retention(&app_clone);
                    }

                    // Hide overlay after transcription completes if in "recording_only" mode.
                    // We request a hide so the frontend can animate (zoom-out) before the webview hides.
                    if overlay_mode_clone == "recording_only" {
                        let _ = app_clone.emit(events::EVENT_OVERLAY_HIDE_REQUESTED, ());

                        // Fallback: if the overlay frontend isn't running/listening, hide anyway.
                        // Re-check the current overlay_mode before hiding to avoid races with settings changes.
                        if let Some(window) = app_clone.get_webview_window("overlay") {
                            let window_clone = window.clone();
                            let app_check = app_clone.clone();
                            let expected_epoch = app_clone
                                .state::<AppState>()
                                .overlay_visibility_epoch
                                .load(Ordering::SeqCst);
                            tauri::async_runtime::spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_millis(220)).await;
                                let current_mode: String = get_setting_from_store(
                                    &app_check,
                                    "overlay_mode",
                                    "recording_only".to_string(),
                                );
                                let current_epoch = app_check
                                    .state::<AppState>()
                                    .overlay_visibility_epoch
                                    .load(Ordering::SeqCst);
                                let pipeline_state = app_check
                                    .try_state::<pipeline::SharedPipeline>()
                                    .map(|p| p.state());
                                log::debug!(
                                    "[overlay] transcription fallback hide check (current_mode={}, expected_epoch={}, current_epoch={}, pipeline_state={:?})",
                                    current_mode,
                                    expected_epoch,
                                    current_epoch,
                                    pipeline_state
                                );
                                if current_mode == "recording_only"
                                    && current_epoch == expected_epoch
                                {
                                    let visible_before = window_clone.is_visible().ok();
                                    log::debug!(
                                        "[overlay] transcription fallback hide firing (visible_before={:?})",
                                        visible_before
                                    );
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
                        }

                        sessions::recording_finalization::complete_current_request_with_pipeline_wav(
                            &app_clone,
                            &pipeline_clone,
                            request_id.as_deref(),
                            stats::EventStatus::Cancelled,
                        );

                        // Notify frontend and hide overlay if needed.
                        recording_completion::emit_cancelled(&app_clone);

                        // Best-effort: remove any in-progress history entry for this request
                        // so it doesn't remain stuck in "in_progress".
                        if let Some(req_id) = request_id.as_ref() {
                            let _ = history_request_lifecycle::apply_request_history_update(
                                &app_clone,
                                history::RequestHistoryUpdate::Delete {
                                    request_id: req_id.clone(),
                                },
                            );
                        }

                        if overlay_mode_clone == "recording_only" {
                            let _ = app_clone.emit(events::EVENT_OVERLAY_HIDE_REQUESTED, ());
                            if let Some(window) = app_clone.get_webview_window("overlay") {
                                let visible_before = window.is_visible().ok();
                                let pipeline_state = app_clone
                                    .try_state::<pipeline::SharedPipeline>()
                                    .map(|p| p.state());
                                log::debug!(
                                    "[overlay] cancellation direct hide (visible_before={:?}, pipeline_state={:?})",
                                    visible_before,
                                    pipeline_state
                                );
                                let _ = window.hide();
                            }
                        }

                        // Done - stop stealing Escape.
                        set_escape_cancel_shortcut_enabled(&app_clone, false);
                        return;
                    }

                    log::error!("Transcription failed: {}", e);
                    recording_completion::emit_pipeline_error(
                        &app_clone,
                        &e.to_string(),
                        request_id.as_deref(),
                    );

                    if let Some(log_store) = app_clone.try_state::<RequestLogStore>() {
                        log_store.with_current(|log| {
                            log.error_with_details(
                                format!("Transcription failed: {}", e),
                                crate::request_log::format_error_chain(&e),
                            );
                            log.complete_error(e.to_string());
                        });
                    }

                    sessions::recording_finalization::complete_current_request_with_pipeline_wav(
                        &app_clone,
                        &pipeline_clone,
                        request_id.as_deref(),
                        stats::EventStatus::Error,
                    );

                    // Persist audio for retry (best-effort)
                    if let Some(wav) = pipeline_clone.clone_last_wav_bytes() {
                        let max_saved_recordings: usize =
                            (get_setting_from_store(&app_clone, "max_saved_recordings", 1000u64))
                                .clamp(1, 100_000) as usize;

                        if let Err(err) = recording_completion::persist_request_recording(
                            &app_clone,
                            request_id.as_deref(),
                            Some(wav.as_slice()),
                            max_saved_recordings,
                        ) {
                            log::warn!("{}", err);
                        }
                    }

                    // Mark history entry as error and keep it
                    if let Some(req_id) = request_id.as_ref() {
                        let _ = history_request_lifecycle::apply_request_history_update(
                            &app_clone,
                            history::RequestHistoryUpdate::CompleteError {
                                request_id: req_id.clone(),
                                error_message: e.to_string(),
                            },
                        );
                    }

                    // Time-based retention (best-effort). Still apply even on failures.
                    sessions::retention::apply_transcription_retention(&app_clone);

                    // Force-show overlay for retry UI regardless of overlay_mode.
                    // If the user is not in always-visible mode, also snap back to the saved preset.
                    if let Err(e) =
                        commands::overlay::show_overlay_with_reset_if_not_always(&app_clone)
                    {
                        log::warn!("Failed to force-show overlay after error: {}", e);
                    }
                }
            }

            // Transcription finished (success or error) - stop stealing Escape.
            set_escape_cancel_shortcut_enabled(&app_clone, false);
        });
    }

    let _ = app.emit(events::EVENT_RECORDING_STOP, ());
}

/// Check if audio mute is supported on this platform
#[tauri::command]
fn is_audio_mute_supported() -> bool {
    audio_mute::is_supported()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Standard Wayland toplevels cannot be placed at absolute screen
    // coordinates. Select the Linux window backend before GTK/Tao initializes;
    // on Wayland sessions with XWayland available this keeps Kolboo's anchored
    // overlay deterministic while session capability checks remain Wayland-aware.
    #[cfg(target_os = "linux")]
    let linux_window_backend = platform_capabilities::configure_linux_window_backend();

    // Initialize structured tracing (JSON logs + request spans).
    tracing_init::init();
    sentry_init::init();

    #[cfg(target_os = "linux")]
    log::info!(
        "Linux window backend policy: {:?} (session={})",
        linux_window_backend,
        platform_capabilities::current_linux_display_server().as_str()
    );

    // If we're invoked with a CLI subcommand, we want to behave like a normal CLI tool:
    // - allow running even while the GUI app is already running
    // - avoid global/singleton plugins (single-instance, global shortcuts) that can block
    //   or interfere with a running instance.
    let is_cli_invocation = crate::cli::is_cli_invocation();

    let mut builder = tauri::Builder::default();

    #[cfg(desktop)]
    {
        if !is_cli_invocation {
            builder = builder.plugin(shortcuts::build_global_shortcut_plugin());
            builder = builder.plugin(tauri_plugin_dialog::init());
            builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
                log::info!("Single-instance: focusing existing app window");
                bootstrap::show_main_window(
                    app,
                    "single-instance",
                    Some(events::EVENT_SINGLE_INSTANCE_ACTIVATED),
                );
            }));
        }
    }

    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_nspanel::init());
    }

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState::default())
        .manage(QuickAskConversationMemory::default())
        .manage(TrayKeepAlive::default())
        .manage(MicTestMeterState::default())
        .manage(commands::whisper::WhisperDownloadManager::default())
        .invoke_handler(tauri::generate_handler![
            commands::audio::play_audio_cue_preview,
            commands::audio::list_audio_input_devices,
            commands::audio::list_audio_input_devices_v2,
            commands::audio::get_default_audio_input_device_name,
            commands::audio::mic_test_start_meter,
            commands::audio::mic_test_stop_meter,
            commands::text::type_text,
            commands::text::get_server_url,
            commands::settings::register_shortcuts,
            commands::settings::unregister_shortcuts,
            commands::settings::set_hotkey_debug_enabled_runtime,
            commands::settings::forward_modifier_key_event,
            commands::settings::settings_doctor,
            commands::settings::settings_apply_patch,
            commands::policy::policy_sync,
            commands::policy::policy_get_state,
            commands::policy::policy_export_diagnostics,
            commands::sync::sync_get_status,
            commands::sync::sync_push_settings,
            commands::sync::sync_pull_settings,
            commands::licensing::license_get_state,
            commands::licensing::license_get_auth_context,
            commands::licensing::license_get_session_access_token,
            commands::licensing::license_start_login,
            commands::licensing::license_sign_up,
            commands::licensing::license_request_password_reset,
            commands::licensing::license_exchange_session,
            commands::licensing::license_logout,
            commands::licensing::license_refresh_entitlement,
            commands::licensing::license_get_management_url,
            managed_inference::managed_inference_get_models,
            managed_inference::managed_inference_get_usage_state,
            commands::settings::hotkey_shortcut_cards_create,
            commands::settings::hotkey_shortcut_cards_update,
            commands::settings::hotkey_shortcut_cards_delete,
            is_audio_mute_supported,
            commands::history::add_history_entry,
            commands::history::get_history,
            commands::history::get_history_page,
            commands::history::get_history_detail,
            commands::history::save_history_edit,
            commands::history::delete_history_entry,
            commands::history::get_history_delete_options,
            commands::history::delete_history_entry_ex,
            commands::history::clear_history,
            commands::overlay::set_overlay_layout,
            commands::overlay::show_overlay,
            commands::overlay::hide_overlay,
            commands::overlay::overlay_frontend_ready,
            commands::overlay::overlay_hover_frontend_ready,
            commands::overlay::quick_ask_frontend_ready,
            commands::overlay::show_overlay_hover,
            commands::overlay::hide_overlay_hover,
            commands::overlay::schedule_hide_overlay_hover,
            commands::overlay::set_overlay_mode,
            commands::overlay::set_widget_position,
            commands::overlay::set_quick_ask_escape_enabled,
            // Pipeline commands for all-in-app STT
            commands::recording::pipeline_start_recording,
            commands::recording::recording_computer_audio_available,
            commands::recording::pipeline_get_recording_seconds,
            commands::recording::pipeline_can_pause_recording,
            commands::recording::recording_list_recovery,
            commands::recording::recording_recover,
            commands::recording::recording_discard_recovery,
            commands::recording::pipeline_set_recording_paused,
            commands::recording::pipeline_get_recording_paused,
            commands::recording::pipeline_get_active_profile_for_foreground_app,
            commands::recording::pipeline_get_session_preset_lock,
            commands::recording::pipeline_set_session_preset_lock,
            commands::recording::pipeline_stop_and_transcribe,
            commands::recording::pipeline_cancel,
            commands::recording::pipeline_get_state,
            commands::ocr::pipeline_get_overlay_state,
            commands::ocr::pipeline_trigger_active_window_ocr,
            commands::ocr::pipeline_cancel_active_window_ocr,
            commands::recording::pipeline_is_recording,
            commands::recording::pipeline_is_error,
            commands::recording::pipeline_update_config,
            commands::recording::pipeline_dictate,
            commands::recording::pipeline_force_reset,
            commands::recording::pipeline_test_transcribe_last_audio,
            commands::recording::pipeline_has_last_audio,
            commands::recording::pipeline_get_last_recording_diagnostics,
            commands::recording::pipeline_test_audio_settings_start_recording,
            commands::recording::pipeline_test_audio_settings_stop_recording,
            commands::recording::pipeline_retry_transcription,
            // Recording file access (for playback)
            commands::recording::recording_get_wav_path,
            commands::recording::recording_get_waveform,
            commands::recording::recording_get_preferences,
            commands::recording::recording_set_preferences,
            // Recording folder helpers
            commands::recording::recordings_open_folder,
            commands::recording::recordings_get_storage_bytes,
            commands::recording::recordings_get_stats,
            commands::recording::recordings_delete_all,
            // Danger-zone data operations
            commands::data::delete_all_api_keys,
            commands::data::delete_all_settings,
            commands::data::delete_all_stats,
            commands::data::get_data_storage_summary,
            commands::data::delete_all_transcripts_keep_recordings,
            commands::data::delete_all_data,
            // Backups (export/import settings; exclude secrets)
            commands::backup::export_settings_backup_json,
            commands::backup::export_settings_backup_to_file,
            commands::backup::import_settings_backup_json,
            commands::backup::import_settings_backup_from_file,
            // Optional GitHub Gist backup
            commands::backup::github_backup_has_token,
            commands::backup::github_backup_set_token,
            commands::backup::github_backup_clear_token,
            commands::backup::github_backup_push_to_gist,
            commands::backup::github_backup_pull_from_gist,
            // Secure secrets (API keys)
            commands::secrets::secrets_has_api_key,
            commands::secrets::secrets_get_api_key,
            commands::secrets::secrets_set_api_key,
            commands::secrets::secrets_clear_api_key,
            // Config commands (replacing Python server)
            commands::config::get_default_sections,
            commands::config::get_runtime_config,
            commands::config::get_available_providers,
            commands::config::sync_pipeline_config,
            // Network commands
            commands::network::get_system_proxy_info,
            commands::network::load_trusted_ca_certificate_from_file,
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
            commands::llm::iterate_rewrite_prompt,
            commands::llm::test_rewrite_with_prompt,
            commands::llm::llm_complete,
            // Router helpers
            commands::router::cache_router_embeddings,
            // Local Whisper model management commands
            commands::whisper::is_local_whisper_available,
            commands::whisper::get_local_whisper_backend_status,
            commands::whisper::is_local_whisper_model_loaded,
            commands::whisper::load_local_whisper_model,
            commands::whisper::unload_local_whisper_model,
            commands::whisper::get_whisper_models,
            commands::whisper::get_whisper_models_dir,
            commands::whisper::is_whisper_model_downloaded,
            commands::whisper::get_whisper_model_url,
            commands::whisper::delete_whisper_model,
            commands::whisper::validate_whisper_model,
            commands::whisper::download_whisper_model,
            commands::whisper::cancel_whisper_model_download,
            // Request logging commands
            commands::logs::get_request_logs,
            commands::logs::clear_request_logs,
            commands::logs::export_request_logs_to_file,
            commands::logs::frontend_log,
            commands::logs::get_app_logs_dir,
            commands::logs::open_app_logs_folder,
            commands::logs::sentry_backend_smoke_test,
            // Fireworks helpers
            commands::fireworks::fireworks_list_models,
            // Ollama helpers
            commands::ollama::ollama_list_models,
            // Usage/cost stats commands
            commands::stats::get_cost_summary,
            commands::stats::get_cost_summary_v2,
            commands::stats::get_cost_by_provider_v2,
            commands::pricing::get_model_pricing,
            // Window/process commands (used for per-program prompts)
            commands::windows::list_open_windows,
            commands::windows::get_foreground_process_path,
        ])
        .setup(|app| {
            app.handle().plugin(tauri_plugin_cli::init())?;

            // Seed defaults into settings.json so UI and backend agree on effective settings.
            // Must run before pipeline initialization and any settings reads.
            #[cfg(desktop)]
            {
                settings::defaults::ensure_default_settings(app.handle())?;
            }

            // If a CLI subcommand is present, run it and exit early.
            // IMPORTANT: do this before setting up overlays/tray/shortcuts so a CLI invocation
            // doesn't interfere with a currently running GUI instance.
            let has_cli_args = std::env::args_os().len() > 1;
            match app.cli().matches() {
                Ok(matches) => {
                    if matches.subcommand.is_some() {
                        // CLI subcommands use pipeline-backed logic (pipeline/config/profiles/diagnostics).
                        // Initialize the pipeline only for CLI runs, and exit before any UI setup.
                        #[cfg(desktop)]
                        {
                            bootstrap::initialize_request_log_store(app.handle());
                            let pipeline = bootstrap::initialize_pipeline_from_settings(app.handle());
                            app.manage(pipeline);
                        }

                        match cli::handle_cli(app.handle(), &matches) {
                            Ok(Some(code)) => {
                                std::process::exit(code);
                            }
                            Ok(None) => {}
                            Err(err) => {
                                let result = cli::CommandResult::<serde_json::Value>::failure(
                                    err.exit_code(),
                                    err.to_string(),
                                );
                                let _ = cli::write_json(&result);
                                std::process::exit(err.exit_code());
                            }
                        }
                    } else if has_cli_args {
                        let result = cli::CommandResult::<serde_json::Value>::failure(
                            2,
                            "CLI arguments provided but no subcommand was recognized."
                                .to_string(),
                        );
                        let _ = cli::write_json(&result);
                        std::process::exit(2);
                    }
                }
                Err(err) => {
                    if has_cli_args {
                        let result = cli::CommandResult::<serde_json::Value>::failure(
                            2,
                            format!("Failed to parse CLI arguments: {err}"),
                        );
                        let _ = cli::write_json(&result);
                        std::process::exit(2);
                    }
                }
            }

            // Dev-only: validate settings shape at startup and print issues to the terminal.
            #[cfg(all(desktop, debug_assertions))]
            {
                use tauri_plugin_store::StoreExt;

                let store = app.store("settings.json")?;
                let mut values = serde_json::Map::new();

                for key in settings::doctor::SETTINGS_DOCTOR_KEYS {
                    if let Some(value) = store.get(*key) {
                        values.insert((*key).to_string(), value);
                    }
                }

                let report = settings::doctor::validate_settings_map(&values);
                if report.issues.is_empty() {
                    log::info!("Settings doctor: no issues found");
                } else {
                    log::warn!(
                        "Settings doctor: found {} issue(s)",
                        report.issues.len()
                    );
                    for issue in report.issues {
                        log::warn!("Settings doctor issue: {} -> {}", issue.key, issue.message);
                    }
                }
            }

            // Startup window visibility:
            // - Show the main window only on first-run (when the setup guide is pending).
            // - Otherwise, keep it hidden; the tray icon is the explicit entrypoint.
            #[cfg(desktop)]
            {
                let guide_state: String = get_setting_from_store(
                    app.handle(),
                    "settings_guide_state",
                    "pending".to_string(),
                );

                if let Some(main) = app.get_webview_window("main") {
                    if guide_state == "pending" {
                        log::info!(
                            "Startup: settings guide is pending -> showing main window"
                        );
                        let _ = main.show();
                        let _ = main.unminimize();
                        let _ = main.set_focus();
                    } else {
                        log::info!(
                            "Startup: settings guide state is '{}' -> keeping main window hidden",
                            guide_state
                        );
                        let _ = main.hide();
                    }
                } else {
                    // If the window isn't present (e.g. it was closed/destroyed or config changed),
                    // the tray will recreate it on demand.
                    log::warn!(
                        "Startup: main window not found; tray will recreate it on demand"
                    );
                }
            }

            // Windows-only: enable modifier-only hotkeys (e.g. Right Alt alone) via a low-level
            // keyboard hook. This is separate from tauri-plugin-global-shortcut.
            #[cfg(target_os = "windows")]
            {
                windows_modifier_hotkeys::init(app.handle().clone());
            }

            // Configure what happens when the user clicks the X on the main/settings window.
            // Default is to close-to-tray (destroy the window; tray can recreate it).
            #[cfg(desktop)]
            {
                if let Some(main) = app.get_webview_window("main") {
                    let app_handle = app.handle().clone();
                    main.on_window_event(move |event| {
                        if let WindowEvent::CloseRequested { api, .. } = event {
                            // Hotkey debug is meant to be temporary. If the user closes the
                            // main window (close-to-tray), disable it so it can't keep
                            // generating high-volume debug events in the background.
                            #[cfg(target_os = "windows")]
                            {
                                crate::windows_modifier_hotkeys::set_hotkey_debug_enabled(false);
                            }

                            // Best-effort: persist the flag off so it doesn't remain enabled
                            // across sessions.
                            {
                                use tauri_plugin_store::StoreExt;

                                if let Ok(store) = app_handle.store("settings.json") {
                                    store.set("hotkey_debug_enabled", serde_json::json!(false));
                                    let _ = store.save();
                                }
                            }

                            let behavior: String = get_setting_from_store(
                                &app_handle,
                                "main_window_close_behavior",
                                "minimize_to_tray".to_string(),
                            );

                            if behavior == "exit_program" {
                                log::info!("Main window close requested -> exiting (exit_program)");
                                api.prevent_close();
                                app_handle.exit(0);
                                return;
                            }

                            // RAM-saving behavior: allow the window to be destroyed.
                            // The tray's Show action will recreate the window if needed.
                            // Also covers the legacy value "close_window".
                            log::info!(
                                "Main window close requested -> closing window (close-to-tray via {behavior})"
                            );
                        }
                    });
                } else {
                    log::warn!("Main window not found during setup; tray will recreate it on demand");
                }
            }

            // Initialize history storage
            let app_data_dir = app_paths::app_data_dir(app.handle())
                .expect("Failed to get app data directory");

            // Initialize recording store (saved WAVs for retry)
            let recording_store = RecordingStore::new(app_data_dir.clone());
            app.manage(recording_store);

            let history_storage = HistoryStorage::new(app_data_dir.clone());
            app.manage(history_storage);

            // Initialize persisted stats store (usage/cost ledger)
            let stats_store = stats::StatsStore::new(app_data_dir);
            app.manage(stats_store);

            #[cfg(desktop)]
            {
                bootstrap::apply_startup_retention(app.handle());
            }

            // Initialize request log store
            bootstrap::initialize_request_log_store(app.handle());

            // Initialize audio mute manager (may be None on unsupported platforms)
            if let Some(audio_mute_manager) = AudioMuteManager::new() {
                app.manage(audio_mute_manager);
            }

            // Initialize pipeline with settings from store
            #[cfg(desktop)]
            {
                let pipeline = bootstrap::initialize_pipeline_from_settings(app.handle());

                // Best-effort: preload local-whisper model at launch.
                // Do this in the background so startup remains snappy.
                #[cfg(feature = "local-whisper")]
                {
                    let preload_pipeline = pipeline.clone();
                    if preload_pipeline.config().local_whisper_load_mode == "on_launch"
                        && !preload_pipeline.is_local_whisper_loaded()
                    {
                        // Emit load events so the UI can update (and so users can see
                        // that on-launch loading is actually happening).
                        let app_handle_for_emit = app.handle().clone();
                        let _ = app_handle_for_emit.emit(
                            commands::whisper::LOCAL_WHISPER_MODEL_LOAD_EVENT,
                            commands::whisper::LocalWhisperModelLoadEvent {
                                status: commands::whisper::LocalWhisperModelLoadStatus::Started,
                                message: None,
                            },
                        );

                        let app_handle_for_emit_done = app_handle_for_emit.clone();
                        tauri::async_runtime::spawn_blocking(move || {
                            let result = preload_pipeline.force_load_local_whisper();

                            let payload = match result {
                                Ok(()) => commands::whisper::LocalWhisperModelLoadEvent {
                                    status: commands::whisper::LocalWhisperModelLoadStatus::Completed,
                                    message: None,
                                },
                                Err(e) => commands::whisper::LocalWhisperModelLoadEvent {
                                    status: commands::whisper::LocalWhisperModelLoadStatus::Error,
                                    message: Some(e.to_string()),
                                },
                            };

                            let _ = app_handle_for_emit_done.emit(
                                commands::whisper::LOCAL_WHISPER_MODEL_LOAD_EVENT,
                                payload,
                            );
                        });
                    }
                }

                // Preload persisted router embeddings cache once at startup.
                // Routing uses the in-memory cache and does not read the store per request.
                let _ = router_embeddings_cache::migrate_router_embeddings_out_of_settings(app.handle());
                let persisted = router_embeddings_cache::load_router_embeddings_from_store(app.handle());
                if !persisted.is_empty() {
                    pipeline.preload_embedding_cache(persisted);
                }

                app.manage(pipeline);

                // Best-effort: silently refresh session material at startup so managed
                // requests don't fail with stale access tokens after long idle periods.
                if crate::licensing::load_session_material(app.handle()).is_some() {
                    let app_handle = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        match commands::licensing::license_refresh_entitlement(
                            app_handle.clone(),
                            Some(false),
                        )
                        .await
                        {
                            Ok(state) => {
                                log::info!(
                                    "Startup silent entitlement refresh succeeded (status={:?})",
                                    state.status
                                );
                            }
                            Err(e) => {
                                log::warn!(
                                    "Startup silent entitlement refresh failed: {}",
                                    e
                                );
                            }
                        }
                    });
                }
            }

            // Backend-driven overlay waveform: publish realtime mic levels to the overlay.
            // This avoids browser getUserMedia startup latency and stays aligned with the
            // actual CPAL capture stream.
            #[cfg(desktop)]
            {
                overlay::spawn_overlay_waveform_publisher(app.handle());
            }

            // Register shortcuts from store (now that store plugin is available)
            #[cfg(desktop)]
            {
                shortcuts::register_initial_shortcuts(app.handle())?;
            }

            overlay::create_overlay_windows(app)?;

            // Setup system tray
            bootstrap::setup_tray(app.handle())?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
