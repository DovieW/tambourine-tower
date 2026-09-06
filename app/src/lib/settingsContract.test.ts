// Deprecated: split into app/src/lib/contracts/** tests.
// Kept as a skipped file to avoid duplicate contract assertions.
import fs from "node:fs";
import { describe as baseDescribe, expect, it, vi } from "vitest";
import type {
	AudioCaptureDiagnostics,
	AudioLevelStats,
	AudioSettingsTestWavs,
	CacheRouterEmbeddingsResponse,
	ConnectionStateChangedPayload,
	CostByProvider,
	CostSummary,
	DataStorageSummary,
	DefaultSectionsResponse,
	EmptyEventPayload,
	HistoryDeleteMode,
	HistoryDeleteOptions,
	HistoryDeleteResult,
	HistoryPageQuery,
	HistoryPageResult,
	HotkeyConfig,
	IntentRouterSettings,
	IterateRewritePromptResponse,
	LlmCompleteResponse,
	LlmModelPricing,
	LlmProviderInfo,
	LocalWhisperBackendStatus,
	LocalWhisperModelLoadEvent,
	LocalWhisperModelLoadStatus,
	MicTestAudioLevelPayload,
	ModelOption,
	ModelPricing,
	OpenWindowInfo,
	OverlayAudioLevelPayload,
	PipelineErrorPayload,
	PipelineStateEvent,
	PipelineTranscriptReadyPayload,
	ProviderCostTotal,
	ProxySettings,
	QuickAskAnswerPayload,
	QuickAskStartedPayload,
	RecordingsStats,
	RequestLog,
	RewritePreset,
	RewriteProgramPromptProfile,
	SettingsChangedPayload,
	SttModelPricing,
	SystemEvent,
	SystemProxyInfo,
	TestLlmRewriteResponse,
	TestRewriteWithPromptResponse,
	TrustedCaCertificate,
	WhisperModelDownloadProgress,
	WhisperModelDownloadStatus,
	WhisperModelInfo,
	WindowsInternetProxySettings,
} from "./tauri";
import { configAPI } from "./tauri";

const describe = baseDescribe.skip;

type AvailableProvidersResponse = Awaited<
	ReturnType<typeof configAPI.getAvailableProviders>
>;
type ProviderInfo = AvailableProvidersResponse["stt"][number];

type StoreLike = {
	get<T = unknown>(key: string): Promise<T | undefined>;
	set<T = unknown>(key: string, value: T): Promise<void>;
	delete(key: string): Promise<void>;
	save(): Promise<void>;
};

class FakeStore implements StoreLike {
	public readonly data = new Map<string, unknown>();
	public saveCalls = 0;

	constructor(initial: Record<string, unknown>) {
		for (const [k, v] of Object.entries(initial)) {
			this.data.set(k, v);
		}
	}

	async get<T = unknown>(key: string): Promise<T | undefined> {
		return this.data.get(key) as T | undefined;
	}

	async set<T = unknown>(key: string, value: T): Promise<void> {
		this.data.set(key, value);
	}

	async delete(key: string): Promise<void> {
		this.data.delete(key);
	}

	async save(): Promise<void> {
		this.saveCalls += 1;
	}
}

let currentStore: FakeStore;

vi.mock("@tauri-apps/api/core", () => ({
	convertFileSrc: (x: string) => x,
	invoke: vi.fn(async () => {
		throw new Error("invoke() not implemented in unit tests");
	}),
}));

vi.mock("@tauri-apps/api/event", () => ({
	emit: vi.fn(async () => {
		// no-op
	}),
	listen: vi.fn(async () => {
		return () => {
			// no-op
		};
	}),
}));

vi.mock("@tauri-apps/api/window", () => ({
	getCurrentWindow: vi.fn(() => ({
		// minimal stub
	})),
}));

vi.mock("@tauri-apps/plugin-store", () => ({
	Store: {
		load: vi.fn(async () => currentStore),
	},
}));

function extractRustSeededSettingsKeys(rustSource: string): string[] {
	// We intentionally keep this parsing dumb + stable.
	// The backend default seeding uses set_default("key", ...).
	const keys = new Set<string>();
	const re = /set_default\(\s*"([^"]+)"/g;
	for (const match of rustSource.matchAll(re)) {
		const k = match[1];
		if (typeof k === "string" && k.trim().length > 0) {
			keys.add(k);
		}
	}

	return [...keys].sort();
}

describe("settings contract: Rust defaults vs TS getSettings", () => {
	it("keeps TS getSettings() in sync with backend-seeded settings keys", async () => {
		vi.resetModules();
		currentStore = new FakeStore({});

		const { tauriAPI } = await import("./tauri");
		const settings = await tauriAPI.getSettings();
		const tsKeys = new Set(Object.keys(settings));

		const rustPath = new URL("../../src-tauri/src/lib.rs", import.meta.url);
		const rustSource = fs.readFileSync(rustPath, "utf8");
		const rustSeededKeys = extractRustSeededSettingsKeys(rustSource);
		const rustSeededKeySet = new Set(rustSeededKeys);

		// Keys that are intentionally backend-only (or legacy) and not part of the UI settings model.
		// If you add a new backend-seeded setting and the UI should see it, DO NOT add it here.
		const allowMissingFromUi = new Set<string>([
			// Backend pipeline config (stored in settings but not exposed in AppSettings today)
			"vad_settings",
			// Legacy key: replaced by quick_ask_hold_hotkey, but still used as a fallback for older installs
			"quick_ask_hotkey",
			// Legacy key: UI migrated to unit+value but backend still seeds this for compatibility
			"transcription_retention_days",
		]);

		const missingInTs = rustSeededKeys.filter(
			(k) => !tsKeys.has(k) && !allowMissingFromUi.has(k),
		);

		// Keys that are intentionally UI-only (or stored but not seeded by the backend).
		// If you add a new UI setting that should have a backend default, DO NOT add it here.
		const allowMissingFromRust = new Set<string>([
			// UI-only runtime state / device selection
			"selected_mic_id",
			// UI-only presentation settings
			"accent_color",
			"audio_cue",
			// UI prompt editing state (backend falls back to its own defaults)
			"cleanup_prompt_sections",
			// Provider/model selections are optional and can be unset
			"stt_model",
			"llm_provider",
			"llm_model",
			"quick_ask_provider",
			"quick_ask_model",
			// Provider-specific thinking knobs (optional)
			"openai_reasoning_effort",
			"anthropic_thinking_budget",
			"gemini_thinking_budget",
			"gemini_thinking_level",
			"quick_ask_openai_reasoning_effort",
			"quick_ask_anthropic_thinking_budget",
			"quick_ask_gemini_thinking_budget",
			"quick_ask_gemini_thinking_level",
			// Provider-specific base URL
			"aquavoice_base_url",
		]);

		const missingInRust = [...tsKeys].filter(
			(k) => !rustSeededKeySet.has(k) && !allowMissingFromRust.has(k),
		);

		expect(
			missingInTs,
			`Rust seeds settings keys not present in tauriAPI.getSettings(): ${missingInTs.join(
				", ",
			)}`,
		).toEqual([]);

		expect(
			missingInRust,
			`tauriAPI.getSettings() returns keys not seeded in Rust defaults: ${missingInRust.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps ProxySettings shape aligned with backend JSON schema", async () => {
		vi.resetModules();
		currentStore = new FakeStore({});

		const { tauriAPI } = await import("./tauri");
		const settings = await tauriAPI.getSettings();
		const proxySettings = settings.proxy_settings as ProxySettings;

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/proxy-settings.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
			definitions?: Record<string, { properties?: Record<string, unknown> }>;
		};

		const schemaProps = schema.properties ?? {};
		const missingProxyKeys = Object.keys(proxySettings).filter(
			(k) => !(k in schemaProps),
		);

		const manualProps =
			schema.definitions?.ManualProxySettings?.properties ?? {};
		const missingManualKeys = Object.keys(proxySettings.manual).filter(
			(k) => !(k in manualProps),
		);

		const sampleCert: TrustedCaCertificate = {
			id: "",
			file_name: "",
			format: "pem",
			data_base64: "",
		};
		const certProps =
			schema.definitions?.TrustedCaCertificate?.properties ?? {};
		const missingCertKeys = Object.keys(sampleCert).filter(
			(k) => !(k in certProps),
		);

		expect(
			missingProxyKeys,
			`ProxySettings keys missing in backend schema: ${missingProxyKeys.join(
				", ",
			)}`,
		).toEqual([]);

		expect(
			missingManualKeys,
			`ManualProxySettings keys missing in backend schema: ${missingManualKeys.join(
				", ",
			)}`,
		).toEqual([]);

		expect(
			missingCertKeys,
			`TrustedCaCertificate keys missing in backend schema: ${missingCertKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps HotkeyConfig shape aligned with backend JSON schema", async () => {
		vi.resetModules();
		currentStore = new FakeStore({});

		const { tauriAPI } = await import("./tauri");
		const settings = await tauriAPI.getSettings();
		const hotkey = settings.toggle_hotkey as HotkeyConfig;

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/hotkey-config.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingHotkeyKeys = Object.keys(hotkey).filter(
			(k) => !(k in schemaProps),
		);

		expect(
			missingHotkeyKeys,
			`HotkeyConfig keys missing in backend schema: ${missingHotkeyKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps IntentRouterSettings shape aligned with backend JSON schema", async () => {
		vi.resetModules();
		currentStore = new FakeStore({});

		const sampleRouter: IntentRouterSettings = {
			enabled: true,
			strategy: "off",
			embedding_provider: null,
			embedding_model: null,
			pick_highest_score: null,
			similarity_threshold: null,
			similarity_margin: null,
			llm_provider: null,
			llm_model: null,
			openai_reasoning_effort: null,
			gemini_thinking_budget: null,
			gemini_thinking_level: null,
			anthropic_thinking_budget: null,
			llm_system_prompt: null,
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/intent-router-settings.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sampleRouter).filter(
			(k) => !(k in schemaProps),
		);

		expect(
			missingKeys,
			`IntentRouterSettings keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
		).toEqual([]);
	});

	it("keeps RewritePreset shape aligned with backend JSON schema", async () => {
		vi.resetModules();
		currentStore = new FakeStore({});

		const samplePreset: RewritePreset = {
			id: "",
			name: "",
			description: null,
			routing_hints: null,
			cleanup_prompt_sections: null,
			rewrite_llm_enabled: true,
			stt_provider: null,
			stt_model: null,
			stt_timeout_seconds: null,
			llm_provider: null,
			llm_model: null,
			openai_reasoning_effort: null,
			gemini_thinking_budget: null,
			gemini_thinking_level: null,
			anthropic_thinking_budget: null,
			sound_enabled: null,
			playing_audio_handling: null,
			overlay_mode: null,
			widget_position: null,
			output_mode: null,
			output_hit_enter: null,
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/rewrite-preset.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(samplePreset).filter(
			(k) => !(k in schemaProps),
		);

		expect(
			missingKeys,
			`RewritePreset keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps RewriteProgramPromptProfile shape aligned with backend JSON schema", async () => {
		vi.resetModules();
		currentStore = new FakeStore({});

		const sampleProfile: RewriteProgramPromptProfile = {
			id: "default",
			name: "Default",
			program_paths: [],
			disabled: false,
			cleanup_prompt_sections: null,
			presets: null,
			default_preset_id: null,
			default_preset_description: null,
			default_target_rewrite_llm_enabled: true,
			active_preset_id: null,
			router: null,
			rewrite_llm_enabled: null,
			stt_provider: null,
			stt_model: null,
			stt_timeout_seconds: null,
			llm_provider: null,
			llm_model: null,
			openai_reasoning_effort: null,
			gemini_thinking_budget: null,
			gemini_thinking_level: null,
			anthropic_thinking_budget: null,
			quick_ask_provider: null,
			quick_ask_model: null,
			quick_ask_system_prompt: null,
			context_grab_method: null,
			rewrite_include_clipboard_context: null,
			quick_replace_include_clipboard_context: null,
			quick_ask_include_clipboard_context: null,
			quick_replace_enabled: null,
			quick_replace_provider: null,
			quick_replace_model: null,
			quick_replace_system_prompt: null,
			quick_ask_openai_reasoning_effort: null,
			quick_ask_gemini_thinking_budget: null,
			quick_ask_gemini_thinking_level: null,
			quick_ask_anthropic_thinking_budget: null,
			sound_enabled: null,
			playing_audio_handling: null,
			overlay_mode: null,
			widget_position: null,
			output_mode: null,
			output_hit_enter: null,
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/rewrite-program-profile.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sampleProfile).filter(
			(k) => !(k in schemaProps),
		);

		expect(
			missingKeys,
			`RewriteProgramPromptProfile keys missing in backend schema: ${missingKeys.join(
				", ",
			)}`,
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
			ocr_context_present: false,
			ocr_context_chars: null,
			ocr_failed_reason: null,
			ocr_effective_mode: null,
			ocr_status: null,
			ocr_not_attempted_reason: null,
			ocr_started_at: null,
			ocr_duration_ms: null,
			ocr_request_json: undefined,
			ocr_response_json: undefined,
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/request-log.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
			definitions?: Record<string, { properties?: Record<string, unknown> }>;
		};

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

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/history-page-query.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sampleQuery).filter(
			(k) => !(k in schemaProps),
		);

		expect(
			missingKeys,
			`HistoryPageQuery keys missing in backend schema: ${missingKeys.join(", ")}`,
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

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/history-page-result.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
			definitions?: Record<string, { properties?: Record<string, unknown> }>;
		};

		const schemaProps = schema.properties ?? {};
		const missingResultKeys = Object.keys(sampleResult).filter(
			(k) => !(k in schemaProps),
		);

		const entryProps = schema.definitions?.HistorySummary?.properties ?? {};
		const missingEntryKeys = Object.keys(sampleEntry).filter(
			(k) => !(k in entryProps),
		);

		expect(
			missingResultKeys,
			`HistoryPageResult keys missing in backend schema: ${missingResultKeys.join(", ")}`,
		).toEqual([]);

		expect(
			missingEntryKeys,
			`HistoryEntry keys missing in backend schema: ${missingEntryKeys.join(", ")}`,
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

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/audio-capture-diagnostics.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
			definitions?: Record<string, { properties?: Record<string, unknown> }>;
		};

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
			`AudioCaptureDiagnostics keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);

		expect(
			missingStatsKeys,
			`AudioLevelStats keys missing in backend schema: ${missingStatsKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps AudioLevelStats shape aligned with backend JSON schema", () => {
		const sampleStats: AudioLevelStats = {
			duration_secs: 0,
			rms: 0,
			peak: 0,
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/audio-level-stats.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sampleStats).filter(
			(k) => !(k in schemaProps),
		);

		expect(
			missingKeys,
			`AudioLevelStats keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps RecordingsStats shape aligned with backend JSON schema", () => {
		const sampleStats: RecordingsStats = {
			count: 0,
			bytes: 0,
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/recordings-stats.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sampleStats).filter(
			(k) => !(k in schemaProps),
		);

		expect(
			missingKeys,
			`RecordingsStats keys missing in backend schema: ${missingKeys.join(", ")}`,
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

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/data-storage-summary.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sampleSummary).filter(
			(k) => !(k in schemaProps),
		);

		expect(
			missingKeys,
			`DataStorageSummary keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps DefaultSectionsResponse shape aligned with backend JSON schema", () => {
		const sample: DefaultSectionsResponse = { system: "" };

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/default-sections-response.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`DefaultSectionsResponse keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps AvailableProvidersResponse shape aligned with backend JSON schema", () => {
		expect(typeof configAPI.getAvailableProviders).toBe("function");

		const provider: ProviderInfo = {
			value: "openai",
			label: "OpenAI",
			is_local: false,
		};

		const sample: AvailableProvidersResponse = {
			stt: [provider],
			llm: [provider],
			ocr: { available: true, reason: null },
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/available-providers-response.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
			definitions?: Record<string, { properties?: Record<string, unknown> }>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		const providerProps = schema.definitions?.ProviderInfo?.properties ?? {};
		const missingProviderKeys = Object.keys(provider).filter(
			(k) => !(k in providerProps),
		);

		expect(
			missingKeys,
			`AvailableProvidersResponse keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);

		expect(
			missingProviderKeys,
			`ProviderInfo keys missing in backend schema: ${missingProviderKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps HistoryDeleteOptions shape aligned with backend JSON schema", () => {
		const sample: HistoryDeleteOptions = {
			recording_id: null,
			recording_exists: false,
			recording_ref_count: 0,
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/history-delete-options.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`HistoryDeleteOptions keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps HistoryDeleteResult shape aligned with backend JSON schema", () => {
		const sample: HistoryDeleteResult = {
			deleted_entries: 0,
			deleted_recording: false,
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/history-delete-result.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`HistoryDeleteResult keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps HistoryDeleteMode values aligned with backend JSON schema", () => {
		const sampleMode: HistoryDeleteMode = "entry_only";

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/history-delete-mode.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			enum?: string[];
			oneOf?: Array<{ enum?: string[] }>;
		};

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

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/system-proxy-info.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
			definitions?: Record<string, { properties?: Record<string, unknown> }>;
		};

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
			`SystemProxyInfo keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);

		expect(
			missingWindowsKeys,
			`WindowsInternetProxySettings keys missing in backend schema: ${missingWindowsKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps AudioSettingsTestWavs shape aligned with backend JSON schema", () => {
		const sample: AudioSettingsTestWavs = {
			raw_wav_base64: "",
			processed_wav_base64: "",
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/audio-settings-test-wavs.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`AudioSettingsTestWavs keys missing in backend schema: ${missingKeys.join(", ")}`,
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

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/whisper-model-info.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`WhisperModelInfo keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps WhisperModelDownloadStatus values aligned with backend JSON schema", () => {
		const sampleStatus: WhisperModelDownloadStatus = "queued";

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/whisper-model-download-status.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			enum?: string[];
			oneOf?: Array<{ enum?: string[] }>;
		};

		const enumValues =
			schema.enum ?? schema.oneOf?.flatMap((v) => v.enum ?? []) ?? [];
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

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/whisper-model-download-progress.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`WhisperModelDownloadProgress keys missing in backend schema: ${missingKeys.join(", ")}`,
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

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/local-whisper-backend-status.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
			definitions?: Record<string, { properties?: Record<string, unknown> }>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		const observedProps =
			schema.definitions?.LocalWhisperBackendObserved?.properties ?? {};
		const missingObservedKeys = Object.keys(sample.observed ?? {}).filter(
			(k) => !(k in observedProps),
		);

		expect(
			missingKeys,
			`LocalWhisperBackendStatus keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);

		expect(
			missingObservedKeys,
			`LocalWhisperBackendObserved keys missing in backend schema: ${missingObservedKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps LocalWhisperModelLoadEvent shape aligned with backend JSON schema", () => {
		const status: LocalWhisperModelLoadStatus = "started";
		const sample: LocalWhisperModelLoadEvent = {
			status,
			message: null,
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/local-whisper-model-load-event.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
			definitions?: Record<
				string,
				{ enum?: string[]; oneOf?: Array<{ enum?: string[] }> }
			>;
		};

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
			`LocalWhisperModelLoadEvent keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);

		expect(enumValues).toContain(status);
	});

	it("keeps SystemEvent shape aligned with backend JSON schema", () => {
		const sample: SystemEvent = {
			timestamp: new Date().toISOString(),
			event_type: "debug",
			message: "hello",
			details: null,
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/system-event.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`SystemEvent keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps PipelineErrorPayload shape aligned with backend JSON schema", () => {
		const sample: PipelineErrorPayload = {
			message: "boom",
			request_id: null,
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/pipeline-error-payload.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`PipelineErrorPayload keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps PipelineStateEvent values aligned with backend JSON schema", () => {
		const sampleState: PipelineStateEvent = "idle";

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/pipeline-state-changed.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			enum?: string[];
			oneOf?: Array<{ enum?: string[] }>;
		};

		const enumValues =
			schema.enum ?? schema.oneOf?.flatMap((v) => v.enum ?? []) ?? [];
		expect(enumValues).toContain(sampleState);
		expect(enumValues).toContain("recording");
		expect(enumValues).toContain("transcribing");
		expect(enumValues).toContain("routing");
		expect(enumValues).toContain("rewriting");
		expect(enumValues).toContain("error");
	});

	it("keeps PipelineTranscriptReadyPayload aligned with backend JSON schema", () => {
		const sample: PipelineTranscriptReadyPayload = "hello";

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/pipeline-transcript-ready.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as { type?: string };

		expect(typeof sample).toBe("string");
		expect(schema.type).toBe("string");
	});

	function assertNullEventSchema(schemaFile: string, label: string) {
		const sample: EmptyEventPayload = null;
		const schemaPath = new URL(
			`../../src-tauri/gen/schemas/${schemaFile}`,
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as { type?: string };

		expect(sample).toBeNull();
		expect(schema.type, `${label} schema should be null`).toBe("null");
	}

	it("keeps pipeline-recording-started payload aligned with backend JSON schema", () => {
		assertNullEventSchema(
			"pipeline-recording-started.schema.json",
			"pipeline-recording-started",
		);
	});

	it("keeps pipeline-transcription-started payload aligned with backend JSON schema", () => {
		assertNullEventSchema(
			"pipeline-transcription-started.schema.json",
			"pipeline-transcription-started",
		);
	});

	it("keeps pipeline-routing-started payload aligned with backend JSON schema", () => {
		assertNullEventSchema(
			"pipeline-routing-started.schema.json",
			"pipeline-routing-started",
		);
	});

	it("keeps pipeline-rewriting-started payload aligned with backend JSON schema", () => {
		assertNullEventSchema(
			"pipeline-rewriting-started.schema.json",
			"pipeline-rewriting-started",
		);
	});

	it("keeps pipeline-cancelled payload aligned with backend JSON schema", () => {
		assertNullEventSchema(
			"pipeline-cancelled.schema.json",
			"pipeline-cancelled",
		);
	});

	it("keeps pipeline-reset payload aligned with backend JSON schema", () => {
		assertNullEventSchema("pipeline-reset.schema.json", "pipeline-reset");
	});

	it("keeps recording-start payload aligned with backend JSON schema", () => {
		assertNullEventSchema("recording-start.schema.json", "recording-start");
	});

	it("keeps recording-stop payload aligned with backend JSON schema", () => {
		assertNullEventSchema("recording-stop.schema.json", "recording-stop");
	});

	it("keeps overlay-hide-requested payload aligned with backend JSON schema", () => {
		assertNullEventSchema(
			"overlay-hide-requested.schema.json",
			"overlay-hide-requested",
		);
	});

	it("keeps history-changed payload aligned with backend JSON schema", () => {
		assertNullEventSchema("history-changed.schema.json", "history-changed");
	});

	it("keeps stats-changed payload aligned with backend JSON schema", () => {
		assertNullEventSchema("stats-changed.schema.json", "stats-changed");
	});

	it("keeps settings-changed payload aligned with backend JSON schema", () => {
		const sample: SettingsChangedPayload = {};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/settings-changed.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as { type?: string };

		expect(sample).toEqual({});
		expect(schema.type).toBe("object");
	});

	it("keeps connection-state-changed payload aligned with backend JSON schema", () => {
		const sample: ConnectionStateChangedPayload = { state: "idle" };

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/connection-state-changed.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
			definitions?: Record<
				string,
				{ enum?: string[]; oneOf?: Array<{ enum?: string[] }> }
			>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));
		expect(
			missingKeys,
			`connection-state-changed keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);

		const enumValues =
			schema.definitions?.ConnectionStateEvent?.enum ??
			schema.definitions?.ConnectionStateEvent?.oneOf?.flatMap(
				(v) => v.enum ?? [],
			) ??
			[];
		expect(enumValues).toContain("idle");
		expect(enumValues).toContain("recording");
		expect(enumValues).toContain("processing");
		expect(enumValues).toContain("connecting");
		expect(enumValues).toContain("disconnected");
	});

	it("keeps OverlayAudioLevelPayload shape aligned with backend JSON schema", () => {
		const sample: OverlayAudioLevelPayload = {
			seq: 1,
			rms: 0,
			peak: 0,
			wave_seq: 1,
			mins: [],
			maxes: [],
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/overlay-audio-level-payload.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`OverlayAudioLevelPayload keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps QuickAskStartedPayload shape aligned with backend JSON schema", () => {
		const sample: QuickAskStartedPayload = {
			question: "hello",
			provider: "openai",
			model: "gpt-4o",
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/quick-ask-started-payload.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`QuickAskStartedPayload keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps QuickAskAnswerPayload shape aligned with backend JSON schema", () => {
		const sampleOk: QuickAskAnswerPayload = {
			ok: true,
			answer: "42",
			provider_used: "openai",
			model_used: "gpt-4o",
			duration_ms: 5,
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/quick-ask-answer-payload.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			oneOf?: Array<{ properties?: Record<string, unknown> }>;
			anyOf?: Array<{ properties?: Record<string, unknown> }>;
			definitions?: Record<string, { properties?: Record<string, unknown> }>;
		};

		const candidates = schema.oneOf ?? schema.anyOf ?? [];
		const okProps = candidates
			.map((c) => {
				const props = c.properties ?? {};
				if (Object.keys(props).length > 0) return props;
				const ref = (c as { $ref?: string }).$ref;
				if (!ref) return {};
				const refName = ref.split("/").pop();
				if (!refName) return {};
				return schema.definitions?.[refName]?.properties ?? {};
			})
			.find((props) => "answer" in props && "ok" in props);

		const missingOkKeys = Object.keys(sampleOk).filter(
			(k) => !(k in (okProps ?? {})),
		);

		expect(
			missingOkKeys,
			`QuickAskAnswerPayload (ok) keys missing in backend schema: ${missingOkKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps MicTestAudioLevelPayload shape aligned with backend JSON schema", () => {
		const sample: MicTestAudioLevelPayload = {
			active: true,
			session_id: 1,
			seq: 1,
			rms: 0,
			peak: 0,
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/mic-test-audio-level-payload.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`MicTestAudioLevelPayload keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
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

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/cost-summary.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

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

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/cost-by-provider.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
			definitions?: Record<string, { properties?: Record<string, unknown> }>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		const providerProps =
			schema.definitions?.ProviderCostTotal?.properties ?? {};
		const missingProviderKeys = Object.keys(provider).filter(
			(k) => !(k in providerProps),
		);

		expect(
			missingKeys,
			`CostByProvider keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);

		expect(
			missingProviderKeys,
			`ProviderCostTotal keys missing in backend schema: ${missingProviderKeys.join(", ")}`,
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

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/model-pricing.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
			definitions?: Record<string, { properties?: Record<string, unknown> }>;
		};

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
			`SttModelPricing keys missing in backend schema: ${missingSttKeys.join(", ")}`,
		).toEqual([]);

		expect(
			missingLlmKeys,
			`LlmModelPricing keys missing in backend schema: ${missingLlmKeys.join(", ")}`,
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

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/cache-router-embeddings-response.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`CacheRouterEmbeddingsResponse keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps OpenWindowInfo shape aligned with backend JSON schema", () => {
		const sample: OpenWindowInfo = {
			title: "",
			process_path: "",
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/open-window-info.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`OpenWindowInfo keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	it("keeps ModelOption shape aligned with backend JSON schema", () => {
		const sample: ModelOption = {
			value: "foo",
			label: "Foo",
			disabled: false,
		};

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/model-option.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

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

		const schemaPath = new URL(
			"../../src-tauri/gen/schemas/llm-provider-info.schema.json",
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

		const schemaProps = schema.properties ?? {};
		const missingKeys = Object.keys(sample).filter((k) => !(k in schemaProps));

		expect(
			missingKeys,
			`LlmProviderInfo keys missing in backend schema: ${missingKeys.join(", ")}`,
		).toEqual([]);
	});

	function assertLlmResponseSchema<T extends object>(
		sample: T,
		schemaFile: string,
		label: string,
	) {
		const schemaPath = new URL(
			`../../src-tauri/gen/schemas/${schemaFile}`,
			import.meta.url,
		);
		const rawSchema = fs
			.readFileSync(schemaPath, "utf8")
			.replace(/^\uFEFF/, "");
		const schema = JSON.parse(rawSchema) as {
			properties?: Record<string, unknown>;
		};

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
