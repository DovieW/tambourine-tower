import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { buildCostInvokeParams } from "../costParams";
import { emitTyped, listenTyped } from "./events";
import { tauriLicenseAPI } from "./license";
import { managedInferenceAPI } from "./managedInference";
import { tauriPolicyAPI } from "./policy";
import { applySettingsRuntimeSyncPolicy } from "./settingsSync";
import type {
	AudioCaptureDiagnostics,
	AudioInputDeviceInfo,
	AudioSettingsTestWavs,
	CacheRouterEmbeddingsResponse,
	ConnectionState,
	CostByProvider,
	CostSummary,
	CostTimeframe,
	DataStorageSummary,
	DefaultSectionsResponse,
	HistoryDeleteMode,
	HistoryDeleteOptions,
	HistoryDeleteResult,
	HistoryEntry,
	HistoryPageQuery,
	HistoryPageResult,
	IterateRewritePromptResponse,
	LicenseAuthContext,
	LicenseState,
	LlmCompleteResponse,
	LlmProviderInfo,
	LocalWhisperBackendStatus,
	ModelOption,
	ModelPricing,
	ModelPricingKind,
	OpenAiReasoningEffort,
	OpenWindowInfo,
	PolicyDiagnosticExport,
	PolicyState,
	RecordingsStats,
	RequestLog,
	SettingsChangedPayload,
	SettingsDoctorReport,
	SystemProxyInfo,
	TestLlmRewriteResponse,
	TestRewriteWithPromptResponse,
	TrustedCaCertificate,
	WhisperModelInfo,
} from "./types";

export const tauriAPI = {
	async typeText(text: string): Promise<{ success: boolean; error?: string }> {
		try {
			await invoke("type_text", { text });
			return { success: true };
		} catch (error) {
			return { success: false, error: String(error) };
		}
	},

	async onStartRecording(callback: () => void): Promise<UnlistenFn> {
		return listenTyped("recording-start", () => {
			callback();
		});
	},

	async onStopRecording(callback: () => void): Promise<UnlistenFn> {
		return listenTyped("recording-stop", () => {
			callback();
		});
	},

	async getCostSummary(params: {
		timeframe: CostTimeframe;
		kind?: "all" | "stt" | "llm";
		sttModelKeys?: string[];
		llmModelKeys?: string[];
		excludeFreeTier?: boolean;
	}): Promise<CostSummary> {
		const costParams = buildCostInvokeParams(params);
		return invoke("get_cost_summary_v2", {
			params: {
				timeframe: costParams.timeframe,
				kind: costParams.kind,
				sttModelKeys: costParams.sttModelKeys,
				llmModelKeys: costParams.llmModelKeys,
				excludeFreeTier: costParams.excludeFreeTier,
			},
		});
	},

	async getCostByProvider(params: {
		timeframe: CostTimeframe;
		kind?: "all" | "stt" | "llm";
		sttModelKeys?: string[];
		llmModelKeys?: string[];
		excludeFreeTier?: boolean;
	}): Promise<CostByProvider> {
		const costParams = buildCostInvokeParams(params);
		return invoke("get_cost_by_provider_v2", {
			params: {
				timeframe: costParams.timeframe,
				kind: costParams.kind,
				sttModelKeys: costParams.sttModelKeys,
				llmModelKeys: costParams.llmModelKeys,
				excludeFreeTier: costParams.excludeFreeTier,
			},
		});
	},

	async getModelPricing(params: {
		provider: string;
		kind: ModelPricingKind;
		model: string;
	}): Promise<ModelPricing | null> {
		return invoke("get_model_pricing", {
			provider: params.provider,
			kind: params.kind,
			model: params.model,
		});
	},

	async getSystemProxyInfo(): Promise<SystemProxyInfo> {
		return invoke<SystemProxyInfo>("get_system_proxy_info");
	},

	async loadTrustedCaCertificateFromFile(
		path: string,
	): Promise<TrustedCaCertificate> {
		return invoke<TrustedCaCertificate>(
			"load_trusted_ca_certificate_from_file",
			{
				path,
			},
		);
	},

	async listOpenWindows(params?: {
		includeTitles?: boolean;
	}): Promise<OpenWindowInfo[]> {
		if (params?.includeTitles) {
			return invoke("list_open_windows", { includeTitles: true });
		}
		return invoke("list_open_windows");
	},

	async getForegroundProcessPath(): Promise<string | null> {
		return invoke("get_foreground_process_path");
	},

	async isLocalWhisperAvailable(): Promise<boolean> {
		return invoke("is_local_whisper_available");
	},

	async getLocalWhisperBackendStatus(): Promise<LocalWhisperBackendStatus> {
		return invoke("get_local_whisper_backend_status");
	},

	async getWhisperModels(): Promise<WhisperModelInfo[]> {
		return invoke("get_whisper_models");
	},

	async getWhisperModelsDir(): Promise<string> {
		return invoke("get_whisper_models_dir");
	},

	async downloadWhisperModel(modelId: string): Promise<void> {
		await invoke("download_whisper_model", { modelId });
	},

	async cancelWhisperModelDownload(modelId: string): Promise<void> {
		await invoke("cancel_whisper_model_download", { modelId });
	},

	async deleteWhisperModel(modelId: string): Promise<void> {
		await invoke("delete_whisper_model", { modelId });
	},

	async validateWhisperModel(modelId: string): Promise<boolean> {
		return invoke("validate_whisper_model", { modelId });
	},

	async isLocalWhisperModelLoaded(): Promise<boolean> {
		return invoke("is_local_whisper_model_loaded");
	},

	async loadLocalWhisperModel(): Promise<void> {
		await invoke("load_local_whisper_model");
	},

	async unloadLocalWhisperModel(): Promise<void> {
		await invoke("unload_local_whisper_model");
	},

	async isAudioMuteSupported(): Promise<boolean> {
		return invoke("is_audio_mute_supported");
	},

	async listAudioInputDevicesV2(): Promise<AudioInputDeviceInfo[]> {
		return invoke<AudioInputDeviceInfo[]>("list_audio_input_devices_v2");
	},

	async getDefaultAudioInputDeviceName(): Promise<string | null> {
		return invoke<string | null>("get_default_audio_input_device_name");
	},

	async startMicTestMeter(inputDeviceId?: string | null): Promise<void> {
		await invoke("mic_test_start_meter", {
			args: {
				inputDeviceId: inputDeviceId ?? null,
			},
		});
	},

	async stopMicTestMeter(): Promise<void> {
		await invoke("mic_test_stop_meter");
	},

	// API Key management
	async hasApiKey(storeKey: string): Promise<boolean> {
		return invoke("secrets_has_api_key", { storeKey });
	},

	async getApiKey(storeKey: string): Promise<string | null> {
		const value = await invoke<string | null>("secrets_get_api_key", {
			storeKey,
		});
		return value ?? null;
	},

	async setApiKey(storeKey: string, apiKey: string): Promise<void> {
		await invoke("secrets_set_api_key", { storeKey, apiKey });
		await applySettingsRuntimeSyncPolicy({
			apiKeysChanged: true,
			backendEventEmitted: false,
			invoke,
			emitSettingsChanged: (payload) => emitTyped("settings-changed", payload),
		});
	},

	async clearApiKey(storeKey: string): Promise<void> {
		await invoke("secrets_clear_api_key", { storeKey });
		await applySettingsRuntimeSyncPolicy({
			apiKeysChanged: true,
			backendEventEmitted: false,
			invoke,
			emitSettingsChanged: (payload) => emitTyped("settings-changed", payload),
		});
	},

	async registerShortcuts(): Promise<void> {
		return invoke("register_shortcuts");
	},

	async unregisterShortcuts(): Promise<void> {
		return invoke("unregister_shortcuts");
	},

	async runSettingsDoctor(): Promise<SettingsDoctorReport> {
		return invoke("settings_doctor");
	},

	// History API
	async addHistoryEntry(text: string): Promise<HistoryEntry> {
		return invoke("add_history_entry", { text });
	},

	async getHistory(limit?: number): Promise<HistoryEntry[]> {
		return invoke("get_history", { limit });
	},

	async getHistoryPage(params: HistoryPageQuery): Promise<HistoryPageResult> {
		return invoke("get_history_page", { params });
	},

	getHistoryDetail: (id: string) =>
		invoke<import("./types").HistoryDetail | null>("get_history_detail", {
			id,
		}),
	saveHistoryEdit: (input: import("./types").HistoryEditInput) =>
		invoke<import("./types").HistoryEdit>("save_history_edit", { input }),

	async deleteHistoryEntry(id: string): Promise<boolean> {
		return invoke("delete_history_entry", { id });
	},

	async getHistoryDeleteOptions(id: string): Promise<HistoryDeleteOptions> {
		return invoke("get_history_delete_options", { id });
	},

	async deleteHistoryEntryEx(
		id: string,
		mode: HistoryDeleteMode,
	): Promise<HistoryDeleteResult> {
		return invoke("delete_history_entry_ex", { id, mode });
	},

	async clearHistory(): Promise<void> {
		return invoke("clear_history");
	},

	// Overlay API
	async setOverlayLayout(expanded: boolean): Promise<void> {
		return invoke("set_overlay_layout", { expanded });
	},

	async showOverlayHover(): Promise<void> {
		return invoke("show_overlay_hover");
	},

	async scheduleHideOverlayHover(delayMs: number): Promise<void> {
		return invoke("schedule_hide_overlay_hover", { delayMs });
	},

	async hideOverlayHover(): Promise<void> {
		return invoke("hide_overlay_hover");
	},

	/**
	 * Enable or disable the Escape-key shortcut while the Quick Ask UI is visible.
	 *
	 * When enabled, pressing Escape will be handled by Quick Ask (e.g. to cancel/close it).
	 * When disabled, Quick Ask will not register the Escape shortcut, allowing other handlers
	 * or windows to receive the key event instead.
	 *
	 * @param enabled Whether the Quick Ask Escape shortcut should be registered.
	 */
	async setQuickAskEscapeEnabled(enabled: boolean): Promise<void> {
		return invoke("set_quick_ask_escape_enabled", { enabled });
	},

	async startDragging(): Promise<void> {
		const window = getCurrentWindow();
		return window.startDragging();
	},

	// Connection state sync between windows
	async emitConnectionState(state: ConnectionState): Promise<void> {
		return emitTyped("connection-state-changed", { state });
	},

	async onConnectionStateChanged(
		callback: (state: ConnectionState) => void,
	): Promise<UnlistenFn> {
		return listenTyped("connection-state-changed", (payload) => {
			callback(payload.state);
		});
	},

	// History sync between windows
	async emitHistoryChanged(): Promise<void> {
		return emitTyped("history-changed", null);
	},

	async onHistoryChanged(callback: () => void): Promise<UnlistenFn> {
		return listenTyped("history-changed", () => {
			callback();
		});
	},

	async onStatsChanged(callback: () => void): Promise<UnlistenFn> {
		return listenTyped("stats-changed", () => {
			callback();
		});
	},

	async onTranscriptCopiedToClipboard(
		callback: () => void,
	): Promise<UnlistenFn> {
		return listenTyped("transcript-copied-to-clipboard", () => {
			callback();
		});
	},

	// Settings sync between windows (main -> overlay)
	async emitSettingsChanged(
		payload: SettingsChangedPayload = {},
	): Promise<void> {
		return emitTyped("settings-changed", payload);
	},

	async onSettingsChanged(
		callback: (payload: SettingsChangedPayload) => void,
	): Promise<UnlistenFn> {
		return listenTyped("settings-changed", (payload) => {
			callback((payload ?? {}) as SettingsChangedPayload);
		});
	},

	async getPolicyState(): Promise<PolicyState> {
		return tauriPolicyAPI.getPolicyState();
	},

	async syncPolicy(request?: { policyPack?: unknown }): Promise<PolicyState> {
		return tauriPolicyAPI.syncPolicy(request);
	},

	async exportPolicyDiagnostics(): Promise<PolicyDiagnosticExport> {
		return tauriPolicyAPI.exportPolicyDiagnostics();
	},

	async getLicenseState(): Promise<LicenseState> {
		return tauriLicenseAPI.getState();
	},

	async getLicenseAuthContext(): Promise<LicenseAuthContext> {
		return tauriLicenseAPI.getAuthContext();
	},

	async startLicenseLogin(request?: {
		provider_hint?: string | null;
		auth_provider?: string | null;
		email?: string | null;
		password?: string | null;
	}): Promise<LicenseState> {
		return tauriLicenseAPI.startLogin(request);
	},

	async signUpLicense(request: {
		email: string;
		password: string;
	}): Promise<import("./license").LicenseSignupResponse> {
		return tauriLicenseAPI.signUp(request);
	},

	async requestLicensePasswordReset(email: string): Promise<void> {
		return tauriLicenseAPI.requestPasswordReset(email);
	},

	async exchangeLicenseSession(
		upstreamAccessToken: string,
	): Promise<import("./types").SessionExchangeResponse> {
		return tauriLicenseAPI.exchangeSession(upstreamAccessToken);
	},

	async logoutLicense(): Promise<LicenseState> {
		return tauriLicenseAPI.logout();
	},

	async refreshLicenseEntitlement(
		simulateFailure?: boolean,
	): Promise<LicenseState> {
		return tauriLicenseAPI.refreshEntitlement(simulateFailure);
	},

	async getLicenseManagementUrl(): Promise<string> {
		return tauriLicenseAPI.getManagementUrl();
	},

	async cacheRouterEmbeddings(params: {
		profileId: string;
		forceRefresh?: boolean;
	}): Promise<CacheRouterEmbeddingsResponse> {
		return invoke("cache_router_embeddings", {
			profileId: params.profileId,
			forceRefresh: params.forceRefresh ?? null,
		});
	},
};

export const llmAPI = {
	getLlmProviders: () => invoke<LlmProviderInfo[]>("get_llm_providers"),

	getFireworksModels: () => invoke<ModelOption[]>("fireworks_list_models"),
	getOllamaModels: () => invoke<ModelOption[]>("ollama_list_models"),

	testLlmRewrite: (params: { transcript: string; profileId?: string | null }) =>
		invoke<TestLlmRewriteResponse>("test_llm_rewrite", {
			transcript: params.transcript,
			// IMPORTANT: Tauri command arg mapping uses camelCase in JS.
			// Rust signature uses `profile_id`, which Tauri maps from `profileId`.
			profileId: params.profileId ?? null,
		}),

	complete: (params: {
		provider: string;
		model?: string | null;
		systemPrompt: string;
		userPrompt: string;

		// Optional provider-specific thinking knobs.
		openAiReasoningEffort?: OpenAiReasoningEffort | null;
		geminiThinkingBudget?: number | null;
		geminiThinkingLevel?: "minimal" | "low" | "medium" | "high" | null;
		anthropicThinkingBudget?: number | null;
	}) =>
		invoke<LlmCompleteResponse>("llm_complete", {
			// Rust signature: llm_complete(pipeline, args: LlmCompleteArgs)
			args: {
				provider: params.provider,
				model: params.model ?? null,

				openAiReasoningEffort: params.openAiReasoningEffort ?? null,
				geminiThinkingBudget: params.geminiThinkingBudget ?? null,
				geminiThinkingLevel: params.geminiThinkingLevel ?? null,
				anthropicThinkingBudget: params.anthropicThinkingBudget ?? null,

				// The backend accepts both camelCase and snake_case via serde aliases.
				systemPrompt: params.systemPrompt,
				userPrompt: params.userPrompt,
			},
		}),

	iterateRewritePrompt: (params: {
		profileId?: string | null;
		mode?: "fixed" | "new";
		transcript: string;
		problemOutput: string;
		desiredOutput?: string | null;
		currentPrompt: string;

		// Optional overrides used by Prompt Lab only.
		llmProvider?: string | null;
		llmModel?: string | null;
		openAiReasoningEffort?: OpenAiReasoningEffort | null;
		geminiThinkingLevel?: "minimal" | "low" | "medium" | "high" | null;
		geminiThinkingBudget?: number | null;
		anthropicThinkingBudget?: number | null;
	}) =>
		invoke<IterateRewritePromptResponse>("iterate_rewrite_prompt", {
			transcript: params.transcript,
			problemOutput: params.problemOutput,
			desiredOutput:
				typeof params.desiredOutput === "string" && params.desiredOutput.trim()
					? params.desiredOutput
					: null,
			currentPrompt: params.currentPrompt,
			profileId: params.profileId ?? null,
			mode: params.mode ?? null,

			llmProvider: params.llmProvider ?? null,
			llmModel: params.llmModel ?? null,
			openAiReasoningEffort: params.openAiReasoningEffort ?? null,
			geminiThinkingLevel: params.geminiThinkingLevel ?? null,
			geminiThinkingBudget:
				typeof params.geminiThinkingBudget === "number" &&
				Number.isFinite(params.geminiThinkingBudget)
					? params.geminiThinkingBudget
					: null,
			anthropicThinkingBudget:
				typeof params.anthropicThinkingBudget === "number" &&
				Number.isFinite(params.anthropicThinkingBudget)
					? params.anthropicThinkingBudget
					: null,
		}),

	testRewriteWithPrompt: (params: {
		profileId?: string | null;
		transcript: string;
		prompt: string;
	}) =>
		invoke<TestRewriteWithPromptResponse>("test_rewrite_with_prompt", {
			transcript: params.transcript,
			prompt: params.prompt,
			profileId: params.profileId ?? null,
		}),

	/**
	 * Forward a modifier-only key event (like AltRight) from the frontend to the backend.
	 *
	 * Why this exists:
	 * WebView2 (Chromium) intercepts Alt key events for menu accelerator handling
	 * before they reach the Windows low-level keyboard hook. When the WebView has
	 * focus, the WH_KEYBOARD_LL hook never sees AltRight events. This allows the
	 * frontend to detect AltRight via JavaScript and forward it.
	 */
	forwardModifierKeyEvent: (key: string, isDown: boolean) =>
		invoke<void>("forward_modifier_key_event", { key, isDown }),
};

export const sttAPI = {
	testTranscribeLastAudio: (params: { profileId?: string | null }) =>
		invoke<string>("pipeline_test_transcribe_last_audio", {
			// See note above: Rust uses `profile_id`, JS should pass `profileId`.
			profileId: params.profileId ?? null,
		}),

	hasLastAudio: () => invoke<boolean>("pipeline_has_last_audio"),

	getLastRecordingDiagnostics: () =>
		invoke<AudioCaptureDiagnostics | null>(
			"pipeline_get_last_recording_diagnostics",
		),

	// Retry a previous request using its persisted audio.
	// Returns the final text (STT + optional LLM), same as normal transcription.
	retryTranscription: (params: { requestId: string }) =>
		invoke<string>("pipeline_retry_transcription", {
			requestId: params.requestId,
		}),
};

export const audioSettingsTestAPI = {
	startRecording: () =>
		invoke<void>("pipeline_test_audio_settings_start_recording"),
	stopRecording: () =>
		invoke<AudioSettingsTestWavs>(
			"pipeline_test_audio_settings_stop_recording",
		),
};

interface ProviderInfo {
	value: string;
	label: string;
	is_local: boolean;
}

interface OcrProviderStatus {
	available: boolean;
	reason?: string | null;
}

export interface OverlayPipelineState {
	pipeline_state: string;
	ocr_session_id: string | null;
	ocr_status: "not_started" | "running" | "done" | "failed" | "cancelled";
	ocr_manual_available: boolean;
	ocr_provider: OcrProviderStatus;
	/** True when STT is done (before LLM / output). Use with ocr_status == "running" to show "waiting for OCR". */
	stt_complete: boolean;
}

interface AvailableProvidersResponse {
	stt: ProviderInfo[];
	llm: ProviderInfo[];
	ocr: OcrProviderStatus;
}

export const configAPI = {
	// Default prompt sections (from Tauri)
	getDefaultSections: () =>
		invoke<DefaultSectionsResponse>("get_default_sections"),

	// Available providers (from Tauri, based on configured API keys)
	getAvailableProviders: () =>
		invoke<AvailableProvidersResponse>("get_available_providers"),

	// Sync pipeline config when settings change
	syncPipelineConfig: () => invoke<void>("sync_pipeline_config"),

	// Managed inference usage-state adapter
	getManagedUsageState: () => managedInferenceAPI.getUsageState(),
};

export const ocrAPI = {
	triggerActiveWindowOcr: () =>
		invoke<boolean>("pipeline_trigger_active_window_ocr"),
	cancelActiveWindowOcr: () =>
		invoke<void>("pipeline_cancel_active_window_ocr"),
	getOverlayState: () =>
		invoke<OverlayPipelineState>("pipeline_get_overlay_state"),
};

export const recordingControlsAPI = {
	getPreferences: () =>
		invoke<import("./types").RecordingPreferences>("recording_get_preferences"),
	setPreferences: (preferences: import("./types").RecordingPreferences) =>
		invoke<void>("recording_set_preferences", { preferences }),
	getSeconds: () => invoke<number>("pipeline_get_recording_seconds"),
	canPause: () => invoke<boolean>("pipeline_can_pause_recording"),
	computerAudioAvailable: () =>
		invoke<boolean>("recording_computer_audio_available"),
	listRecovery: () => invoke<string[]>("recording_list_recovery"),
	recover: (id: string) => invoke<void>("recording_recover", { id }),
	discardRecovery: (id: string) =>
		invoke<void>("recording_discard_recovery", { id }),
	getPaused: () => invoke<boolean>("pipeline_get_recording_paused"),
	setPaused: (paused: boolean) =>
		invoke<void>("pipeline_set_recording_paused", { paused }),
	getState: () => invoke<string>("pipeline_get_state"),
	start: (computerAudio = false) =>
		invoke<void>("pipeline_start_recording", {
			historyOnly: true,
			computerAudio,
		}),
	stop: () => invoke<string>("pipeline_stop_and_transcribe"),
	cancel: () => invoke<void>("pipeline_cancel"),
};

export const policyAPI = {
	syncPolicy: (request?: { policyPack?: unknown }) =>
		tauriPolicyAPI.syncPolicy(request),
	getPolicyState: () => tauriPolicyAPI.getPolicyState(),
	exportPolicyDiagnostics: () => tauriPolicyAPI.exportPolicyDiagnostics(),
};

export const licenseAPI = {
	getState: () => tauriLicenseAPI.getState(),
	getAuthContext: () => tauriLicenseAPI.getAuthContext(),
	startLogin: (request?: {
		provider_hint?: string | null;
		auth_provider?: string | null;
		email?: string | null;
		password?: string | null;
	}) => tauriLicenseAPI.startLogin(request),
	signUp: (request: { email: string; password: string }) =>
		tauriLicenseAPI.signUp(request),
	requestPasswordReset: (email: string) =>
		tauriLicenseAPI.requestPasswordReset(email),
	exchangeSession: (upstreamAccessToken: string) =>
		tauriLicenseAPI.exchangeSession(upstreamAccessToken),
	logout: () => tauriLicenseAPI.logout(),
	refreshEntitlement: (simulateFailure?: boolean) =>
		tauriLicenseAPI.refreshEntitlement(simulateFailure),
	getManagementUrl: () => tauriLicenseAPI.getManagementUrl(),
	onTransition: tauriLicenseAPI.onTransition,
};

export const logsAPI = {
	getRequestLogs: (limit?: number) =>
		invoke<RequestLog[]>("get_request_logs", { limit: limit ?? 50 }),

	clearRequestLogs: () => invoke<void>("clear_request_logs"),

	exportRequestLogsToFile: (params: {
		path: string;
		limit?: number;
		stripTextAndPayloads?: boolean;
	}) =>
		invoke<void>("export_request_logs_to_file", {
			path: params.path,
			limit: params.limit,
			stripTextAndPayloads: params.stripTextAndPayloads ?? true,
		}),

	/** Directory containing daily-rotated app trace logs, or `null` if unavailable. */
	getAppLogsDir: () => invoke<string | null>("get_app_logs_dir"),

	/** Open the app trace logs directory in the system file explorer. */
	openAppLogsFolder: () => invoke<void>("open_app_logs_folder"),

	/** Trigger a deterministic backend Sentry smoke event. */
	sentryBackendSmokeTest: (surface?: string) =>
		invoke<boolean>("sentry_backend_smoke_test", {
			surface: surface ?? "ui-manual",
		}),
};

export const dataAPI = {
	getStorageSummary: () =>
		invoke<DataStorageSummary>("get_data_storage_summary"),

	deleteAllRecordings: () => invoke<number>("recordings_delete_all"),

	// Deletes transcript text for all history entries but keeps any linked recordings.
	// Returns the number of entries updated.
	deleteAllTranscriptsKeepRecordings: () =>
		invoke<number>("delete_all_transcripts_keep_recordings"),

	deleteAllApiKeys: () => invoke<void>("delete_all_api_keys"),

	deleteAllSettings: () => invoke<void>("delete_all_settings"),

	deleteAllStats: () => invoke<void>("delete_all_stats"),

	deleteAllData: () => invoke<void>("delete_all_data"),
};

export const backupAPI = {
	exportSettingsBackupJson: () => invoke<string>("export_settings_backup_json"),

	exportSettingsBackupToFile: (params: { path: string }) =>
		invoke<void>("export_settings_backup_to_file", { path: params.path }),

	importSettingsBackupJson: (params: { json: string }) =>
		invoke<void>("import_settings_backup_json", { json: params.json }),

	importSettingsBackupFromFile: (params: { path: string }) =>
		invoke<void>("import_settings_backup_from_file", { path: params.path }),

	githubBackupHasToken: () => invoke<boolean>("github_backup_has_token"),

	githubBackupSetToken: (params: { token: string }) =>
		invoke<void>("github_backup_set_token", { token: params.token }),

	githubBackupClearToken: () => invoke<void>("github_backup_clear_token"),

	githubBackupPushToGist: (params: { gistId?: string | null }) =>
		invoke<string>("github_backup_push_to_gist", {
			gistId: params.gistId ?? null,
		}),

	githubBackupPullFromGist: (params: { gistId: string }) =>
		invoke<string>("github_backup_pull_from_gist", { gistId: params.gistId }),
};

export const recordingsAPI = {
	// Returns a URL usable as an <audio src>, or null if no recording exists.
	getRecordingAssetUrl: async (params: { requestId: string }) => {
		const path = await invoke<string | null>("recording_get_wav_path", {
			requestId: params.requestId,
		});
		return path ? convertFileSrc(path) : null;
	},

	getRecordingWaveform: (params: { requestId: string }) =>
		invoke<import("./types").RecordingWaveform | null>(
			"recording_get_waveform",
			{
				requestId: params.requestId,
			},
		),

	// Open recordings directory in file explorer.
	openRecordingsFolder: () => invoke<void>("recordings_open_folder"),

	// Total size (bytes) used by saved recordings.
	getRecordingsStorageBytes: () =>
		invoke<number>("recordings_get_storage_bytes"),

	// Stats for UI display (count + bytes).
	getRecordingsStats: () => invoke<RecordingsStats>("recordings_get_stats"),
};
