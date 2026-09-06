use schemars::schema::RootSchema;

pub(crate) struct SchemaSpec {
    pub out_file: &'static str,
    pub label: &'static str,
    pub generator: fn() -> RootSchema,
}

impl SchemaSpec {
    const fn new(
        out_file: &'static str,
        label: &'static str,
        generator: fn() -> RootSchema,
    ) -> Self {
        Self {
            out_file,
            label,
            generator,
        }
    }
}

macro_rules! schema_spec {
    ($out_file:literal, $label:expr, $generator:ident) => {
        SchemaSpec::new($out_file, $label, $generator)
    };
}

fn gen_export_audio_capture_diagnostics_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::AudioCaptureDiagnostics)
}

fn gen_export_audio_level_stats_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::AudioLevelStats)
}

fn gen_export_audio_settings_test_wavs_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::AudioSettingsTestWavs)
}

fn gen_export_available_providers_response_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::AvailableProvidersResponse)
}

fn gen_export_cache_router_embeddings_response_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::CacheRouterEmbeddingsResponse)
}

fn gen_export_connection_state_changed_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::ConnectionStateChangedPayload)
}

fn gen_export_cost_by_provider_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::CostByProviderResponse)
}

fn gen_export_cost_summary_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::CostSummaryResponse)
}

fn gen_export_data_storage_summary_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::DataStorageSummary)
}

fn gen_export_default_sections_response_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::DefaultSectionsResponse)
}

fn gen_export_history_changed_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::EmptyEventPayload)
}

fn gen_export_history_delete_mode_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::HistoryDeleteMode)
}

fn gen_export_history_delete_options_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::HistoryDeleteOptions)
}

fn gen_export_history_delete_result_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::HistoryDeleteResult)
}

fn gen_export_history_page_query_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::HistoryPageQuery)
}

fn gen_export_history_page_result_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::HistoryPageResult)
}

fn gen_export_history_detail_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::HistoryDetail)
}
fn gen_export_history_edit_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::HistoryEdit)
}
fn gen_export_history_edit_input_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::HistoryEditInput)
}

fn gen_export_hotkey_config_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::HotkeyConfig)
}

fn gen_export_intent_router_settings_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::IntentRouterSettings)
}

fn gen_export_iterate_rewrite_prompt_response_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::IterateRewritePromptResponse)
}

fn gen_export_llm_complete_response_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::LlmCompleteResponse)
}

fn gen_export_llm_provider_info_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::LlmProviderInfo)
}

fn gen_export_local_whisper_backend_status_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::LocalWhisperBackendStatus)
}

fn gen_export_local_whisper_model_load_event_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::LocalWhisperModelLoadEvent)
}

fn gen_export_mic_test_audio_level_payload_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::MicTestAudioLevelPayload)
}

fn gen_export_model_option_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::ModelOption)
}

fn gen_export_model_pricing_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::ModelPricingResponse)
}

fn gen_export_open_window_info_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::OpenWindowInfo)
}

fn gen_export_overlay_audio_level_payload_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::OverlayAudioLevelPayload)
}

fn gen_export_overlay_hide_requested_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::EmptyEventPayload)
}

fn gen_export_pipeline_cancelled_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::EmptyEventPayload)
}

fn gen_export_pipeline_error_payload_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::PipelineErrorPayload)
}

fn gen_export_pipeline_recording_started_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::EmptyEventPayload)
}

fn gen_export_pipeline_reset_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::EmptyEventPayload)
}

fn gen_export_pipeline_rewriting_started_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::EmptyEventPayload)
}

fn gen_export_pipeline_routing_started_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::EmptyEventPayload)
}

fn gen_export_pipeline_state_changed_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::PipelineStateEvent)
}

fn gen_export_pipeline_transcript_ready_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::PipelineTranscriptReadyPayload)
}

fn gen_export_pipeline_transcription_started_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::EmptyEventPayload)
}

fn gen_export_proxy_settings_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::ProxySettings)
}

fn gen_export_quick_ask_answer_payload_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::QuickAskAnswerPayload)
}

fn gen_export_quick_ask_started_payload_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::QuickAskStartedPayload)
}

fn gen_export_recording_start_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::EmptyEventPayload)
}

fn gen_export_recording_stop_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::EmptyEventPayload)
}

fn gen_export_recordings_stats_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::RecordingsStats)
}

fn gen_export_recording_waveform_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::RecordingWaveform)
}

fn gen_export_recording_preferences_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::RecordingPreferences)
}

fn gen_export_request_log_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::RequestLog)
}

fn gen_export_rewrite_preset_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::RewritePreset)
}

fn gen_export_rewrite_program_profile_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::RewriteProgramPromptProfile)
}

fn gen_export_settings_changed_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::SettingsChangedPayload)
}

fn gen_export_stats_changed_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::EmptyEventPayload)
}

fn gen_export_system_event_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::SystemEvent)
}

fn gen_export_system_proxy_info_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::SystemProxyInfo)
}

fn gen_export_test_llm_rewrite_response_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::TestLlmRewriteResponse)
}

fn gen_export_test_rewrite_with_prompt_response_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::TestRewriteWithPromptResponse)
}

fn gen_export_whisper_model_download_progress_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::WhisperModelDownloadProgress)
}

fn gen_export_whisper_model_download_status_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::WhisperModelDownloadStatus)
}

fn gen_export_whisper_model_info_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::WhisperModelInfo)
}

fn gen_export_windows_internet_proxy_settings_schema() -> RootSchema {
    schemars::schema_for!(kolboo_lib::WindowsInternetProxySettings)
}

pub(crate) const SCHEMAS: &[SchemaSpec] = &[
    schema_spec!(
        "audio-capture-diagnostics.schema.json",
        "AudioCaptureDiagnostics",
        gen_export_audio_capture_diagnostics_schema
    ),
    schema_spec!(
        "audio-level-stats.schema.json",
        "AudioLevelStats",
        gen_export_audio_level_stats_schema
    ),
    schema_spec!(
        "audio-settings-test-wavs.schema.json",
        "AudioSettingsTestWavs",
        gen_export_audio_settings_test_wavs_schema
    ),
    schema_spec!(
        "available-providers-response.schema.json",
        "AvailableProvidersResponse",
        gen_export_available_providers_response_schema
    ),
    schema_spec!(
        "cache-router-embeddings-response.schema.json",
        "CacheRouterEmbeddingsResponse",
        gen_export_cache_router_embeddings_response_schema
    ),
    schema_spec!(
        "connection-state-changed.schema.json",
        stringify!(kolboo_lib::events::EVENT_CONNECTION_STATE_CHANGED),
        gen_export_connection_state_changed_schema
    ),
    schema_spec!(
        "cost-by-provider.schema.json",
        "CostByProviderResponse",
        gen_export_cost_by_provider_schema
    ),
    schema_spec!(
        "cost-summary.schema.json",
        "CostSummaryResponse",
        gen_export_cost_summary_schema
    ),
    schema_spec!(
        "data-storage-summary.schema.json",
        "DataStorageSummary",
        gen_export_data_storage_summary_schema
    ),
    schema_spec!(
        "default-sections-response.schema.json",
        "DefaultSectionsResponse",
        gen_export_default_sections_response_schema
    ),
    schema_spec!(
        "history-changed.schema.json",
        stringify!(kolboo_lib::events::EVENT_HISTORY_CHANGED),
        gen_export_history_changed_schema
    ),
    schema_spec!(
        "history-delete-mode.schema.json",
        "HistoryDeleteMode",
        gen_export_history_delete_mode_schema
    ),
    schema_spec!(
        "history-delete-options.schema.json",
        "HistoryDeleteOptions",
        gen_export_history_delete_options_schema
    ),
    schema_spec!(
        "history-delete-result.schema.json",
        "HistoryDeleteResult",
        gen_export_history_delete_result_schema
    ),
    schema_spec!(
        "history-page-query.schema.json",
        "HistoryPageQuery",
        gen_export_history_page_query_schema
    ),
    schema_spec!(
        "history-page-result.schema.json",
        "HistoryPageResult",
        gen_export_history_page_result_schema
    ),
    schema_spec!(
        "history-detail.schema.json",
        "HistoryDetail",
        gen_export_history_detail_schema
    ),
    schema_spec!(
        "history-edit.schema.json",
        "HistoryEdit",
        gen_export_history_edit_schema
    ),
    schema_spec!(
        "history-edit-input.schema.json",
        "HistoryEditInput",
        gen_export_history_edit_input_schema
    ),
    schema_spec!(
        "hotkey-config.schema.json",
        "HotkeyConfig",
        gen_export_hotkey_config_schema
    ),
    schema_spec!(
        "intent-router-settings.schema.json",
        "IntentRouterSettings",
        gen_export_intent_router_settings_schema
    ),
    schema_spec!(
        "iterate-rewrite-prompt-response.schema.json",
        "IterateRewritePromptResponse",
        gen_export_iterate_rewrite_prompt_response_schema
    ),
    schema_spec!(
        "llm-complete-response.schema.json",
        "LlmCompleteResponse",
        gen_export_llm_complete_response_schema
    ),
    schema_spec!(
        "llm-provider-info.schema.json",
        "LlmProviderInfo",
        gen_export_llm_provider_info_schema
    ),
    schema_spec!(
        "local-whisper-backend-status.schema.json",
        "LocalWhisperBackendStatus",
        gen_export_local_whisper_backend_status_schema
    ),
    schema_spec!(
        "local-whisper-model-load-event.schema.json",
        "LocalWhisperModelLoadEvent",
        gen_export_local_whisper_model_load_event_schema
    ),
    schema_spec!(
        "mic-test-audio-level-payload.schema.json",
        "MicTestAudioLevelPayload",
        gen_export_mic_test_audio_level_payload_schema
    ),
    schema_spec!(
        "model-option.schema.json",
        "ModelOption",
        gen_export_model_option_schema
    ),
    schema_spec!(
        "model-pricing.schema.json",
        "ModelPricingResponse",
        gen_export_model_pricing_schema
    ),
    schema_spec!(
        "open-window-info.schema.json",
        "OpenWindowInfo",
        gen_export_open_window_info_schema
    ),
    schema_spec!(
        "overlay-audio-level-payload.schema.json",
        "OverlayAudioLevelPayload",
        gen_export_overlay_audio_level_payload_schema
    ),
    schema_spec!(
        "overlay-hide-requested.schema.json",
        stringify!(kolboo_lib::events::EVENT_OVERLAY_HIDE_REQUESTED),
        gen_export_overlay_hide_requested_schema
    ),
    schema_spec!(
        "pipeline-cancelled.schema.json",
        stringify!(kolboo_lib::events::EVENT_PIPELINE_CANCELLED),
        gen_export_pipeline_cancelled_schema
    ),
    schema_spec!(
        "pipeline-error-payload.schema.json",
        "PipelineErrorPayload",
        gen_export_pipeline_error_payload_schema
    ),
    schema_spec!(
        "pipeline-recording-started.schema.json",
        stringify!(kolboo_lib::events::EVENT_PIPELINE_RECORDING_STARTED),
        gen_export_pipeline_recording_started_schema
    ),
    schema_spec!(
        "pipeline-reset.schema.json",
        stringify!(kolboo_lib::events::EVENT_PIPELINE_RESET),
        gen_export_pipeline_reset_schema
    ),
    schema_spec!(
        "pipeline-rewriting-started.schema.json",
        stringify!(kolboo_lib::events::EVENT_PIPELINE_REWRITING_STARTED),
        gen_export_pipeline_rewriting_started_schema
    ),
    schema_spec!(
        "pipeline-routing-started.schema.json",
        stringify!(kolboo_lib::events::EVENT_PIPELINE_ROUTING_STARTED),
        gen_export_pipeline_routing_started_schema
    ),
    schema_spec!(
        "pipeline-state-changed.schema.json",
        "PipelineStateEvent",
        gen_export_pipeline_state_changed_schema
    ),
    schema_spec!(
        "pipeline-transcript-ready.schema.json",
        "PipelineTranscriptReadyPayload",
        gen_export_pipeline_transcript_ready_schema
    ),
    schema_spec!(
        "pipeline-transcription-started.schema.json",
        stringify!(kolboo_lib::events::EVENT_PIPELINE_TRANSCRIPTION_STARTED),
        gen_export_pipeline_transcription_started_schema
    ),
    schema_spec!(
        "proxy-settings.schema.json",
        "ProxySettings",
        gen_export_proxy_settings_schema
    ),
    schema_spec!(
        "quick-ask-answer-payload.schema.json",
        "QuickAskAnswerPayload",
        gen_export_quick_ask_answer_payload_schema
    ),
    schema_spec!(
        "quick-ask-started-payload.schema.json",
        "QuickAskStartedPayload",
        gen_export_quick_ask_started_payload_schema
    ),
    schema_spec!(
        "recording-start.schema.json",
        stringify!(kolboo_lib::events::EVENT_RECORDING_START),
        gen_export_recording_start_schema
    ),
    schema_spec!(
        "recording-stop.schema.json",
        stringify!(kolboo_lib::events::EVENT_RECORDING_STOP),
        gen_export_recording_stop_schema
    ),
    schema_spec!(
        "recordings-stats.schema.json",
        "RecordingsStats",
        gen_export_recordings_stats_schema
    ),
    schema_spec!(
        "recording-waveform.schema.json",
        "RecordingWaveform",
        gen_export_recording_waveform_schema
    ),
    schema_spec!(
        "recording-preferences.schema.json",
        "RecordingPreferences",
        gen_export_recording_preferences_schema
    ),
    schema_spec!(
        "request-log.schema.json",
        "RequestLog",
        gen_export_request_log_schema
    ),
    schema_spec!(
        "rewrite-preset.schema.json",
        "RewritePreset",
        gen_export_rewrite_preset_schema
    ),
    schema_spec!(
        "rewrite-program-profile.schema.json",
        "RewriteProgramPromptProfile",
        gen_export_rewrite_program_profile_schema
    ),
    schema_spec!(
        "settings-changed.schema.json",
        stringify!(kolboo_lib::events::EVENT_SETTINGS_CHANGED),
        gen_export_settings_changed_schema
    ),
    schema_spec!(
        "stats-changed.schema.json",
        stringify!(kolboo_lib::events::EVENT_STATS_CHANGED),
        gen_export_stats_changed_schema
    ),
    schema_spec!(
        "system-event.schema.json",
        "SystemEvent",
        gen_export_system_event_schema
    ),
    schema_spec!(
        "system-proxy-info.schema.json",
        "SystemProxyInfo",
        gen_export_system_proxy_info_schema
    ),
    schema_spec!(
        "test-llm-rewrite-response.schema.json",
        "TestLlmRewriteResponse",
        gen_export_test_llm_rewrite_response_schema
    ),
    schema_spec!(
        "test-rewrite-with-prompt-response.schema.json",
        "TestRewriteWithPromptResponse",
        gen_export_test_rewrite_with_prompt_response_schema
    ),
    schema_spec!(
        "whisper-model-download-progress.schema.json",
        "WhisperModelDownloadProgress",
        gen_export_whisper_model_download_progress_schema
    ),
    schema_spec!(
        "whisper-model-download-status.schema.json",
        "WhisperModelDownloadStatus",
        gen_export_whisper_model_download_status_schema
    ),
    schema_spec!(
        "whisper-model-info.schema.json",
        "WhisperModelInfo",
        gen_export_whisper_model_info_schema
    ),
    schema_spec!(
        "windows-internet-proxy-settings.schema.json",
        "WindowsInternetProxySettings",
        gen_export_windows_internet_proxy_settings_schema
    ),
];

#[cfg(test)]
mod tests {
    use super::SCHEMAS;
    use std::collections::HashSet;

    #[test]
    fn schema_registry_has_unique_output_files() {
        let mut seen = HashSet::new();

        for schema in SCHEMAS {
            assert!(
                seen.insert(schema.out_file),
                "duplicate schema output file {}",
                schema.out_file
            );
        }
    }

    #[test]
    fn schema_registry_entries_are_complete() {
        for schema in SCHEMAS {
            assert!(
                schema.out_file.ends_with(".schema.json"),
                "schema output file should be generated JSON schema: {}",
                schema.out_file
            );
            assert!(!schema.label.trim().is_empty(), "schema label is empty");

            // Generating each schema here keeps the registry Interface honest:
            // adding an entry with a stale generator breaks a small xtask test
            // instead of a later frontend type-generation step.
            let generated = (schema.generator)();
            assert!(
                !generated
                    .schema
                    .metadata()
                    .title
                    .as_deref()
                    .unwrap_or("")
                    .is_empty()
                    || !generated.definitions.is_empty(),
                "schema generator for {} produced an unexpectedly empty schema",
                schema.label
            );
        }
    }
}
