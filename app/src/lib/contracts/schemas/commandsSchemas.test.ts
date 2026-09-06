import { describe, expect, it } from "vitest";
import type {
	AudioCaptureDiagnostics,
	AudioLevelStats,
	AudioSettingsTestWavs,
	CacheRouterEmbeddingsResponse,
	CostByProvider,
	CostSummary,
	DataStorageSummary,
	DefaultSectionsResponse,
	HistoryDeleteMode,
	HistoryDeleteOptions,
	HistoryDeleteResult,
	HistoryDetail,
	HistoryEditInput,
	HistoryPageQuery,
	HistoryPageResult,
	IterateRewritePromptResponse,
	LlmCompleteResponse,
	LlmModelPricing,
	LlmProviderInfo,
	LocalWhisperBackendStatus,
	LocalWhisperModelLoadEvent,
	LocalWhisperModelLoadStatus,
	ModelOption,
	ModelPricing,
	OpenWindowInfo,
	ProviderCostTotal,
	RecordingPreferences,
	RecordingsStats,
	RecordingWaveform,
	RequestLog,
	SttModelPricing,
	SystemProxyInfo,
	TestLlmRewriteResponse,
	TestRewriteWithPromptResponse,
	WhisperModelDownloadProgress,
	WhisperModelDownloadStatus,
	WhisperModelInfo,
	WindowsInternetProxySettings,
} from "../../tauri";
import { configAPI } from "../../tauri";
import { hasSchemas, readSchemaJson } from "./schemaTestUtils";

type AvailableProvidersResponse = Awaited<
	ReturnType<typeof configAPI.getAvailableProviders>
>;

type SchemaDefinition = {
	properties?: Record<string, unknown>;
	enum?: string[];
	oneOf?: Array<{ enum?: string[] }>;
};

type SchemaVariant = {
	properties?: Record<string, unknown>;
	enum?: string[];
};

type JsonSchema = {
	properties?: Record<string, unknown>;
	definitions?: Record<string, SchemaDefinition>;
	oneOf?: SchemaVariant[];
	anyOf?: SchemaVariant[];
	enum?: string[];
	oneOfEnum?: Array<{ enum?: string[] }>;
};

function readSchema(schemaFile: string): JsonSchema {
	return readSchemaJson<JsonSchema>(schemaFile);
}

describe.skipIf(!hasSchemas())("schema contract: command responses", () => {
	it("keeps reader, revisioned edits, waveform and recording preferences aligned", () => {
		const detail: HistoryDetail = {
			entry: {
				id: "meeting",
				timestamp: "2026-01-01T00:00:00Z",
				text: "Speaker A: Hello",
				status: "success",
				speaker_segments: [
					{
						speaker: "A",
						text: "Hello",
						start_seconds: 0,
						end_seconds: 2,
						part: 1,
					},
				],
			},
			original_text: "Speaker A: Hello",
			revision: 1,
			edited: false,
			edit_error: null,
		};
		const edit: HistoryEditInput = {
			id: "meeting",
			expected_revision: 1,
			title: "Notes",
			text: "Correction",
		};
		const preferences: RecordingPreferences = {
			mode: "meeting",
			meeting_model: { provider: "openai", model: "gpt-4o-transcribe-diarize" },
		};
		const waveform: RecordingWaveform = {
			duration_seconds: 14400,
			peaks: [-1, 1],
		};
		for (const [file, sample] of [
			["history-detail.schema.json", detail],
			["history-edit-input.schema.json", edit],
			["recording-preferences.schema.json", preferences],
			["recording-waveform.schema.json", waveform],
		] as const) {
			expect(
				Object.keys(sample).filter(
					(key) => !(key in (readSchema(file).properties ?? {})),
				),
				file,
			).toEqual([]);
		}
		const speaker = readSchema("history-detail.schema.json").definitions
			?.SpeakerSegment;
		expect(
			Object.keys(detail.entry.speaker_segments?.[0] ?? {}).filter(
				(key) => !(key in (speaker?.properties ?? {})),
			),
		).toEqual([]);
	});
	it("keeps RequestLog shape aligned with backend JSON schema", () => {
		const sampleLog: RequestLog = {
			id: "log-1",
			kind: "transcription",
			started_at: new Date().toISOString(),
			ended_at: null,
			stt_provider: "groq",
			stt_model: null,
			llm_provider: null,
			llm_model: null,
			profile_id: null,
			profile_name: null,
			preset_id: null,
			preset_name: null,
			raw_transcript: null,
			final_text: null,
			rewrite_clipboard_context: null,
			quick_ask_question: null,
			quick_ask_context_text: null,
			quick_ask_clipboard_context: null,
			quick_ask_answer: null,
			quick_ask_provider: null,
			quick_ask_model: null,
			quick_ask_duration_ms: null,
			quick_replace_instructions: null,
			quick_replace_selected_text: null,
			quick_replace_output_text: null,
			quick_replace_clipboard_context: null,
			quick_replace_provider: null,
			quick_replace_model: null,
			quick_replace_duration_ms: null,
			total_duration_ms: null,
			stt_duration_ms: null,
			llm_duration_ms: null,
			llm_outcome: null,
			llm_not_attempted_reason: null,
			llm_error_message: null,
			router_duration_ms: null,
			router_strategy: null,
			router_scores: null,
			status: "in_progress",
			error_message: null,
			entries: [],
			stt_is_free_tier: false,
			llm_is_free_tier: false,
			stt_estimated_cost_usd_micros: null,
			llm_estimated_cost_usd_micros: null,
			stt_request_json: undefined,
			stt_response_json: undefined,
			llm_request_json: undefined,
			llm_response_json: undefined,
			quick_ask_request_json: undefined,
			quick_ask_response_json: undefined,
			quick_replace_request_json: undefined,
			quick_replace_response_json: undefined,
			router_request_json: undefined,
			router_response_json: undefined,
		};

		const schema = readSchema("request-log.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sampleLog).filter(
			(k) => !(k in schemaProps),
		);

		expect(
			missingKeys,
			`RequestLog keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps HistoryPageQuery shape aligned with backend JSON schema", () => {
		const sampleQuery: HistoryPageQuery = {
			filterText: "",
			showFailed: true,
			showEmptyTranscript: false,
			selectedSttModelKeys: [],
			selectedLlmModelKeys: [],
			page: 1,
			pageSize: 25,
			includeUsageCounts: true,
		};

		const schema = readSchema("history-page-query.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sampleQuery).filter(
			(k) => !(k in schemaProps),
		);

		expect(
			missingKeys,
			`HistoryPageQuery keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps HistoryPageResult shape aligned with backend JSON schema", () => {
		const sampleEntry: HistoryPageResult["items"][number] = {
			id: "hist-1",
			timestamp: new Date().toISOString(),
			text: "",
			status: "success",
			error_message: null,
			profile_id: null,
			profile_name: null,
			preset_id: null,
			preset_name: null,
			stt_provider: null,
			stt_model: null,
			llm_provider: null,
			llm_model: null,
			recording_request_id: null,
			title: "Test meeting",
			duration_seconds: 3600,
			recording_mode: "meeting",
		};

		const sampleResult: HistoryPageResult = {
			items: [sampleEntry],
			totalAll: 1,
			totalFiltered: 1,
			page: 1,
			pageSize: 25,
			sttModelUsage: [{ key: "groq::whisper", count: 1 }],
			llmModelUsage: [{ key: "openai::gpt-4o", count: 1 }],
		};

		const schema = readSchema("history-page-result.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingResultKeys = Object.keys(sampleResult).filter(
			(k) => !(k in schemaProps),
		);

		const entryProps = schema.definitions?.HistorySummary?.properties ?? {};
		expect(entryProps).not.toHaveProperty("speaker_segments");
		expect(entryProps).not.toHaveProperty("original_stt_text");
		const missingEntryKeys = Object.keys(sampleEntry).filter(
			(k) => !(k in entryProps),
		);

		expect(
			missingResultKeys,
			`HistoryPageResult keys missing in backend schema: ${missingResultKeys.join(
				", ",
			)}`,
		).toEqual([]);

		expect(
			missingEntryKeys,
			`HistoryEntry keys missing in backend schema: ${missingEntryKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps AudioCaptureDiagnostics shape aligned with backend JSON schema", () => {
		const sampleStats: AudioLevelStats = {
			duration_secs: 0,
			rms: 0,
			peak: 0,
		};

		const sampleDiagnostics: AudioCaptureDiagnostics = {
			stats: sampleStats,
			speech_detected: null,
		};

		const schema = readSchema("audio-capture-diagnostics.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sampleDiagnostics).filter(
			(k) => !(k in schemaProps),
		);

		const statsProps = schema.definitions?.AudioLevelStats?.properties ?? {};
		const missingStatsKeys = Object.keys(sampleStats).filter(
			(k) => !(k in statsProps),
		);

		expect(
			missingKeys,
			`AudioCaptureDiagnostics keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);

		expect(
			missingStatsKeys,
			`AudioLevelStats keys missing in backend schema: ${missingStatsKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps AudioLevelStats shape aligned with backend JSON schema", () => {
		const sampleStats: AudioLevelStats = {
			duration_secs: 0,
			rms: 0,
			peak: 0,
		};

		const schema = readSchema("audio-level-stats.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sampleStats).filter(
			(k) => !(k in schemaProps),
		);

		expect(
			missingKeys,
			`AudioLevelStats keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps RecordingsStats shape aligned with backend JSON schema", () => {
		const sampleStats: RecordingsStats = {
			count: 0,
			bytes: 0,
		};

		const schema = readSchema("recordings-stats.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sampleStats).filter(
			(k) => !(k in schemaProps),
		);

		expect(
			missingKeys,
			`RecordingsStats keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps DataStorageSummary shape aligned with backend JSON schema", () => {
		const sampleSummary: DataStorageSummary = {
			recordings_count: 0,
			recordings_bytes: 0,
			history_count: 0,
			history_bytes: 0,
			request_logs_count: 0,
			stats_files_count: 0,
			stats_bytes: 0,
			settings_bytes: 0,
			api_keys_set_count: 0,
		};

		const schema = readSchema("data-storage-summary.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sampleSummary).filter(
			(k) => !(k in schemaProps),
		);

		expect(
			missingKeys,
			`DataStorageSummary keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps DefaultSectionsResponse shape aligned with backend JSON schema", () => {
		const sample: DefaultSectionsResponse = { system: "" };

		const schema = readSchema("default-sections-response.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`DefaultSectionsResponse keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps AvailableProvidersResponse shape aligned with backend JSON schema", () => {
		expect(typeof configAPI.getAvailableProviders).toBe("function");

		const provider = {
			value: "openai",
			label: "OpenAI",
			is_local: false,
		};

		const sample: AvailableProvidersResponse = {
			stt: [provider],
			llm: [provider],
			ocr: { available: true, reason: null },
		};

		const schema = readSchema("available-providers-response.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		const providerProps = schema.definitions?.ProviderInfo?.properties ?? {};
		const missingProviderKeys = Object.keys(provider).filter(
			(k) => !(k in providerProps),
		);

		expect(
			missingKeys,
			`AvailableProvidersResponse keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);

		expect(
			missingProviderKeys,
			`ProviderInfo keys missing in backend schema: ${missingProviderKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps HistoryDeleteOptions shape aligned with backend JSON schema", () => {
		const sample: HistoryDeleteOptions = {
			recording_id: null,
			recording_exists: false,
			recording_ref_count: 0,
		};

		const schema = readSchema("history-delete-options.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`HistoryDeleteOptions keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps HistoryDeleteResult shape aligned with backend JSON schema", () => {
		const sample: HistoryDeleteResult = {
			deleted_entries: 0,
			deleted_recording: false,
		};

		const schema = readSchema("history-delete-result.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`HistoryDeleteResult keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps HistoryDeleteMode values aligned with backend JSON schema", () => {
		const sampleMode: HistoryDeleteMode = "entry_only";

		const schema = readSchema("history-delete-mode.schema.json");
		const enumValues =
			schema.enum ?? schema.oneOf?.flatMap((v) => v.enum ?? []) ?? [];
		expect(enumValues).toContain(sampleMode);
	});

	it("keeps SystemProxyInfo shape aligned with backend JSON schema", () => {
		const sample: SystemProxyInfo = {
			env_http_proxy: null,
			env_https_proxy: null,
			env_no_proxy: null,
			windows_internet_settings: null,
		};

		const schema = readSchema("system-proxy-info.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		const windowsProps =
			schema.definitions?.WindowsInternetProxySettings?.properties ?? {};
		const windowsSample: WindowsInternetProxySettings = {
			proxy_enable: null,
			proxy_server: null,
			proxy_override: null,
			auto_config_url: null,
		};
		const missingWindowsKeys = Object.keys(windowsSample).filter(
			(k) => !(k in windowsProps),
		);

		expect(
			missingKeys,
			`SystemProxyInfo keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);

		expect(
			missingWindowsKeys,
			`WindowsInternetProxySettings keys missing in backend schema: ${missingWindowsKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps AudioSettingsTestWavs shape aligned with backend JSON schema", () => {
		const sample: AudioSettingsTestWavs = {
			raw_wav_base64: "",
			processed_wav_base64: "",
		};

		const schema = readSchema("audio-settings-test-wavs.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`AudioSettingsTestWavs keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps WhisperModelInfo shape aligned with backend JSON schema", () => {
		const sample: WhisperModelInfo = {
			id: "base",
			name: "Base",
			filename: "ggml-base.bin",
			size_bytes: 0,
			size_display: "",
			download_url: "",
			expected_sha256: "",
			is_english_only: false,
			is_downloaded: false,
		};

		const schema = readSchema("whisper-model-info.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`WhisperModelInfo keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps WhisperModelDownloadStatus values aligned with backend JSON schema", () => {
		const sampleStatus: WhisperModelDownloadStatus = "queued";

		const schema = readSchema("whisper-model-download-status.schema.json");
		const enumValues = schema.enum ?? [];
		expect(enumValues).toContain(sampleStatus);
	});

	it("keeps WhisperModelDownloadProgress shape aligned with backend JSON schema", () => {
		const sample: WhisperModelDownloadProgress = {
			model_id: "base",
			status: "queued",
			downloaded_bytes: 0,
			total_bytes: null,
			percent: null,
			message: null,
		};

		const schema = readSchema("whisper-model-download-progress.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`WhisperModelDownloadProgress keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps LocalWhisperBackendStatus shape aligned with backend JSON schema", () => {
		const sample: LocalWhisperBackendStatus = {
			build_has_local_whisper: false,
			build_has_cuda: false,
			compute: "cpu",
			reason: "Local Whisper feature is not enabled in this build.",
			missing_dlls: [],
			observed: {
				nvidia_smi_available: false,
				pid: 0,
				cuda_process_present: null,
				used_gpu_memory_mb: null,
				error: "nvidia-smi observation is only implemented on Windows",
			},
		};

		const schema = readSchema("local-whisper-backend-status.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		const observedProps =
			schema.definitions?.LocalWhisperBackendObserved?.properties ?? {};
		const missingObservedKeys = Object.keys(sample.observed ?? {}).filter(
			(k) => !(k in observedProps),
		);

		expect(
			missingKeys,
			`LocalWhisperBackendStatus keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);

		expect(
			missingObservedKeys,
			`LocalWhisperBackendObserved keys missing in backend schema: ${missingObservedKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps LocalWhisperModelLoadEvent shape aligned with backend JSON schema", () => {
		const status: LocalWhisperModelLoadStatus = "started";
		const sample: LocalWhisperModelLoadEvent = {
			status,
			message: null,
		};

		const schema = readSchema("local-whisper-model-load-event.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		const enumValues =
			schema.definitions?.LocalWhisperModelLoadStatus?.enum ??
			schema.definitions?.LocalWhisperModelLoadStatus?.oneOf?.flatMap(
				(v) => v.enum ?? [],
			) ??
			[];

		expect(
			missingKeys,
			`LocalWhisperModelLoadEvent keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);

		expect(enumValues).toContain(status);
	});

	it("keeps CostSummary shape aligned with backend JSON schema", () => {
		const sample: CostSummary = {
			timeframe: "24h",
			total_usd_micros: 0,
			events_total: 0,
			events_with_cost: 0,
			earliest_included_at: null,
			latest_included_at: null,
		};

		const schema = readSchema("cost-summary.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`CostSummary keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps CostByProvider shape aligned with backend JSON schema", () => {
		const provider: ProviderCostTotal = {
			provider: "openai",
			total_usd_micros: 0,
			events_total: 0,
			events_with_cost: 0,
		};

		const sample: CostByProvider = {
			timeframe: "24h",
			providers: [provider],
		};

		const schema = readSchema("cost-by-provider.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		const providerProps =
			schema.definitions?.ProviderCostTotal?.properties ?? {};
		const missingProviderKeys = Object.keys(provider).filter(
			(k) => !(k in providerProps),
		);

		expect(
			missingKeys,
			`CostByProvider keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);

		expect(
			missingProviderKeys,
			`ProviderCostTotal keys missing in backend schema: ${missingProviderKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps ModelPricing shape aligned with backend JSON schema", () => {
		const stt: SttModelPricing = {
			usd_micros_per_minute: null,
			usd_micros_per_hour: null,
			min_billed_secs: null,
		};

		const llm: LlmModelPricing = {
			input_usd_micros_per_1m: 0,
			cached_input_usd_micros_per_1m: null,
			output_usd_micros_per_1m: 0,
		};

		const sample: ModelPricing = {
			kind: "llm",
			provider: "openai",
			model: "gpt-4o",
			stt,
			llm,
		};

		const schema = readSchema("model-pricing.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		const sttProps = schema.definitions?.SttModelPricing?.properties ?? {};
		const missingSttKeys = Object.keys(stt).filter((k) => !(k in sttProps));

		const llmProps = schema.definitions?.LlmModelPricing?.properties ?? {};
		const missingLlmKeys = Object.keys(llm).filter((k) => !(k in llmProps));

		expect(
			missingKeys,
			`ModelPricing keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);

		expect(
			missingSttKeys,
			`SttModelPricing keys missing in backend schema: ${missingSttKeys.join(
				", ",
			)}`,
		).toEqual([]);

		expect(
			missingLlmKeys,
			`LlmModelPricing keys missing in backend schema: ${missingLlmKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps CacheRouterEmbeddingsResponse shape aligned with backend JSON schema", () => {
		const sample: CacheRouterEmbeddingsResponse = {
			provider: "openai",
			model: "text-embedding-3-small",
			total_hints: 0,
			cached_now: 0,
			skipped_existing: 0,
			stored_inserted: 0,
			stored_updated: 0,
		};

		const schema = readSchema("cache-router-embeddings-response.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`CacheRouterEmbeddingsResponse keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps OpenWindowInfo shape aligned with backend JSON schema", () => {
		const sample: OpenWindowInfo = {
			title: "",
			process_path: "",
		};

		const schema = readSchema("open-window-info.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`OpenWindowInfo keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps ModelOption shape aligned with backend JSON schema", () => {
		const sample: ModelOption = {
			value: "foo",
			label: "Foo",
			disabled: false,
		};

		const schema = readSchema("model-option.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`ModelOption keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps LlmProviderInfo shape aligned with backend JSON schema", () => {
		const sample: LlmProviderInfo = {
			id: "openai",
			name: "OpenAI",
			requires_api_key: true,
			default_model: "gpt-4o",
			models: ["gpt-4o"],
		};

		const schema = readSchema("llm-provider-info.schema.json");
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`LlmProviderInfo keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	function assertLlmResponseSchema<T extends object>(
		sample: T,
		schemaFile: string,
		label: string,
	) {
		const schema = readSchema(schemaFile);
		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`${label} keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	}

	it("keeps TestLlmRewriteResponse shape aligned with backend JSON schema", () => {
		const sample: TestLlmRewriteResponse = {
			output: "",
			provider_used: "openai",
			model_used: "gpt-4o",
		};

		assertLlmResponseSchema(
			sample,
			"test-llm-rewrite-response.schema.json",
			"TestLlmRewriteResponse",
		);
	});

	it("keeps IterateRewritePromptResponse shape aligned with backend JSON schema", () => {
		const sample: IterateRewritePromptResponse = {
			improved_prompt: "",
			provider_used: "openai",
			model_used: "gpt-4o",
		};

		assertLlmResponseSchema(
			sample,
			"iterate-rewrite-prompt-response.schema.json",
			"IterateRewritePromptResponse",
		);
	});

	it("keeps TestRewriteWithPromptResponse shape aligned with backend JSON schema", () => {
		const sample: TestRewriteWithPromptResponse = {
			output: "",
			provider_used: "openai",
			model_used: "gpt-4o",
		};

		assertLlmResponseSchema(
			sample,
			"test-rewrite-with-prompt-response.schema.json",
			"TestRewriteWithPromptResponse",
		);
	});

	it("keeps LlmCompleteResponse shape aligned with backend JSON schema", () => {
		const sample: LlmCompleteResponse = {
			output: "",
			provider_used: "openai",
			model_used: "gpt-4o",
		};

		assertLlmResponseSchema(
			sample,
			"llm-complete-response.schema.json",
			"LlmCompleteResponse",
		);
	});
});
