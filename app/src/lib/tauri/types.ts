import type { HotkeyConfig, HotkeyShortcutCard } from "../hotkeys";

/**
 * Connection state for UI display (maps from pipeline state)
 */
export type ConnectionState =
	| "disconnected"
	| "connecting"
	| "idle"
	| "recording"
	| "processing";

export interface HistoryEntry {
	id: string;
	timestamp: string;
	text: string;
	status?: "in_progress" | "success" | "error";
	error_message?: string | null;
	profile_id?: string | null;
	profile_name?: string | null;
	preset_id?: string | null;
	preset_name?: string | null;
	stt_provider?: string | null;
	stt_model?: string | null;
	llm_provider?: string | null;
	llm_model?: string | null;
	// Request id of the WAV recording to use for playback/rerun.
	recording_request_id?: string | null;
	title?: string | null;
	duration_seconds?: number | null;
	recording_mode?: "dictation" | "meeting" | null;
	speaker_segments?: SpeakerSegment[];
	original_stt_text?: string | null;
}

export interface SpeakerSegment {
	speaker: string;
	text: string;
	start_seconds: number;
	end_seconds: number;
	part: number;
}
export interface RecordingPreferences {
	mode: "dictation" | "meeting";
	meeting_model: { provider: string; model: string; use_managed?: boolean } | null;
}

export interface HistoryEdit {
	revision: number;
	title: string | null;
	text: string | null;
}
export interface HistoryEditInput {
	id: string;
	expected_revision: number;
	title: string | null;
	text: string | null;
}
export interface HistoryDetail {
	entry: HistoryEntry;
	original_text: string;
	revision: number;
	edited: boolean;
	edit_error: string | null;
}

export interface RecordingWaveform {
	duration_seconds: number;
	peaks: number[];
}

export type HistoryDeleteMode =
	| "entry_only"
	| "entry_and_recording"
	| "recording_and_all_entries";

export interface HistoryDeleteOptions {
	recording_id: string | null;
	recording_exists: boolean;
	recording_ref_count: number;
}

export interface HistoryDeleteResult {
	deleted_entries: number;
	deleted_recording: boolean;
}

export interface HistoryPageQuery {
	filterText?: string;
	showFailed?: boolean;
	showEmptyTranscript?: boolean;
	selectedSttModelKeys?: string[];
	selectedLlmModelKeys?: string[];
	page?: number;
	pageSize?: number;
	includeUsageCounts?: boolean;
}

export interface ModelUsageCount {
	key: string;
	count: number;
}

export interface HistoryPageResult {
	items: HistorySummary[];
	totalAll: number;
	totalFiltered: number;
	page: number;
	pageSize: number;
	sttModelUsage: ModelUsageCount[];
	llmModelUsage: ModelUsageCount[];
}

/** text is a preview; fetch HistoryDetail before copying/exporting a document. */
export type HistorySummary = Omit<
	HistoryEntry,
	"speaker_segments" | "original_stt_text"
>;

export interface PromptSection {
	content: string | null;
}

export interface CleanupPromptSections {
	system: PromptSection;
}

// Per-profile prompt overrides: each section can be omitted/null to inherit from Default.
export interface CleanupPromptSectionsOverride {
	system?: PromptSection | null;
}

export type IntentRouterStrategy = "off" | "embeddings" | "llm";

export interface IntentRouterSettings {
	enabled: boolean;
	strategy: IntentRouterStrategy;

	// Embeddings routing knobs (only used when strategy === "embeddings")
	embedding_provider?: "openai" | "cohere" | "fireworks" | null;
	embedding_model?: string | null;
	pick_highest_score?: boolean | null;
	similarity_threshold?: number | null;
	similarity_margin?: number | null;

	// LLM routing knobs (only used when strategy === "llm")
	llm_provider?: string | null;
	llm_model?: string | null;

	// Optional per-router thinking/reasoning knobs (provider/model dependent)
	openai_reasoning_effort?: OpenAiReasoningEffort | null;
	gemini_thinking_budget?: number | null;
	gemini_thinking_level?: "minimal" | "low" | "medium" | "high" | null;
	anthropic_thinking_budget?: number | null;

	// Advanced: optional override for router system prompt
	llm_system_prompt?: string | null;
}

export interface RewritePreset {
	id: string;
	name: string;

	// Optional display hint (used for preset hover/tooltips).
	description?: string | null;

	// Routing hints used by the intent router
	routing_hints?: string[] | null;

	// Same override surface area as RewriteProgramPromptProfile
	cleanup_prompt_sections: CleanupPromptSectionsOverride | null;

	// Explicit per-preset gate for rewrite.
	// Missing/null in legacy settings is treated as true.
	rewrite_llm_enabled: boolean;
	stt_provider?: string | null;
	stt_model?: string | null;
	stt_language?: string | null;
	stt_timeout_seconds?: number | null;
	llm_provider?: string | null;
	llm_model?: string | null;

	openai_reasoning_effort?: OpenAiReasoningEffort | null;
	gemini_thinking_budget?: number | null;
	gemini_thinking_level?: "minimal" | "low" | "medium" | "high" | null;
	anthropic_thinking_budget?: number | null;

	sound_enabled?: boolean | null;
	playing_audio_handling?: PlayingAudioHandling | null;
	overlay_mode?: OverlayMode | null;
	widget_position?: WidgetPosition | null;
	output_mode?: OutputMode | null;
	output_hit_enter?: boolean | null;
}

export interface RewriteProgramPromptProfile {
	id: string;
	name: string;
	program_paths: string[];
	// When true, this profile is temporarily disabled and never activated.
	disabled?: boolean;
	cleanup_prompt_sections: CleanupPromptSectionsOverride | null;

	// Presets/modes within this program profile.
	// Missing/undefined means "no presets" (backward compatible).
	presets?: RewritePreset[] | null;
	// Default preset to use when routing is off or undecided.
	default_preset_id?: string | null;
	// Description for the implicit "Default" (no preset) target, used by the intent router.
	default_preset_description?: string | null;
	// Gate for whether rewrite runs when routed to the implicit "Default" target (no preset).
	// Defaults to true. This does not override the global/per-profile rewrite gate.
	default_target_rewrite_llm_enabled?: boolean | null;
	// Router configuration for auto-selecting a preset based on dictation intent.
	router?: IntentRouterSettings | null;
	// Manually selected active preset for this profile (persisted selection).
	active_preset_id?: string | null;

	// Per-profile gate for the optional LLM rewrite step (falls back to AppSettings.rewrite_llm_enabled)
	rewrite_llm_enabled?: boolean | null;

	// Per-profile overrides for the pipeline
	stt_provider?: string | null;
	stt_model?: string | null;
	stt_language?: string | null;
	stt_timeout_seconds?: number | null;
	llm_provider?: string | null;
	llm_model?: string | null;

	// Per-profile provider-specific thinking/reasoning knobs
	// (null/undefined means inherit from Default/global settings)
	openai_reasoning_effort?: OpenAiReasoningEffort | null;
	gemini_thinking_budget?: number | null;
	gemini_thinking_level?: "minimal" | "low" | "medium" | "high" | null;
	anthropic_thinking_budget?: number | null;

	// Quick Ask (per-profile overrides)
	quick_ask_provider?: string | null;
	quick_ask_model?: string | null;
	quick_ask_system_prompt?: string | null;
	quick_ask_dismiss_mode?: QuickAskDismissMode | null;

	// Context grabbing method for highlighted-text capture.
	context_grab_method?: ContextGrabMethod | null;

	// Clipboard context toggles (per-profile)
	rewrite_include_clipboard_context?: boolean | null;
	quick_replace_include_clipboard_context?: boolean | null;
	quick_ask_include_clipboard_context?: boolean | null;

	// Quick Replace (per-profile overrides)
	quick_replace_enabled?: boolean | null;
	quick_replace_provider?: string | null;
	quick_replace_model?: string | null;
	quick_replace_system_prompt?: string | null;

	quick_ask_openai_reasoning_effort?: OpenAiReasoningEffort | null;
	quick_ask_gemini_thinking_budget?: number | null;
	quick_ask_gemini_thinking_level?:
		| "minimal"
		| "low"
		| "medium"
		| "high"
		| null;
	quick_ask_anthropic_thinking_budget?: number | null;

	// Per-profile overrides for UI (Option 1: override-or-inherit)
	// NOTE: These are persisted in settings.json as part of the profile object.
	// The backend may ignore them until it is updated to apply them at runtime.
	sound_enabled?: boolean | null;
	playing_audio_handling?: PlayingAudioHandling | null;
	overlay_mode?: OverlayMode | null;
	widget_position?: WidgetPosition | null;
	output_mode?: OutputMode | null;

	// After paste, optionally press Enter.
	// (May be ignored by backend until runtime/profile routing supports it.)
	output_hit_enter?: boolean | null;

	// Per-profile OCR context mode overrides (tri-state, null = inherit from global).
	rewrite_active_window_ocr_mode?: ActiveWindowOcrMode | null;
	quick_replace_active_window_ocr_mode?: ActiveWindowOcrMode | null;
	quick_ask_active_window_ocr_mode?: ActiveWindowOcrMode | null;
}

export type PlayingAudioHandling = "none" | "mute" | "pause" | "mute_and_pause";

export type AudioCue = "kolboo" | "maraca" | "clave" | "legacy";

export type OverlayMode = "always" | "never" | "recording_only";

export type ContextGrabMethod =
	| "none"
	| "ctrl_c"
	| "ctrl_shift_c"
	| "ctrl_insert"
	// Legacy (deprecated): previously meant "read clipboard without injecting keys".
	| "clipboard_only";

// Which monitor to place always-on-top overlay windows on.
// - main: primary monitor
// - cursor: monitor that currently contains the mouse cursor
// - active_window: monitor that currently contains the active/foreground window
export type OverlayMonitorTarget = "main" | "cursor" | "active_window";

// ============================================================================
// OCR (Active Window Context) types
// ============================================================================

/**
 * Per-tool OCR context mode (tri-state).
 * - "off": never run OCR
 * - "auto": run OCR automatically when the tool is triggered
 * - "manual": show an OCR button in the recording overlay; OCR runs only after the button is clicked
 */
export type ActiveWindowOcrMode = "off" | "auto" | "manual";

/**
 * OCR provider authentication mode.
 * - "none": no authentication
 * - "bearer_api_key": use Authorization: Bearer <key>
 */
export type OcrAuthMode = "none" | "bearer_api_key";

/**
 * When to capture the screenshot in Auto mode.
 * - "on_start": Capture immediately when recording begins (default, opportunistic)
 * - "on_stop": Capture when recording stops
 */
export type OcrAutoCaptureTiming = "on_stop" | "on_start";

/**
 * Resize filter for OCR capture:
 * - "nearest": Fastest, no interpolation
 * - "triangle": Fast bilinear interpolation
 * - "catmullrom": Smooth, medium speed
 * - "lanczos3": Best quality, slowest
 */
export type OcrResizeFilter =
	| "nearest"
	| "triangle"
	| "catmullrom"
	| "lanczos3";

/**
 * Payload emitted when OCR context becomes unavailable (e.g., failure, timeout).
 * Used by the overlay to display a non-blocking "OCR context unavailable" message.
 */
export interface OverlayOcrContextUnavailablePayload {
	/** Session/request id this event relates to (allows the overlay to ignore stale events). */
	request_id?: string | null;
	/** Stable reason code for the unavailability. */
	reason: string;
	/** User-friendly message (no technical stack traces). */
	message: string;
}

// ============================================================================

export interface WhisperModelInfo {
	id: string;
	name: string;
	filename: string;
	size_bytes: number;
	size_display: string;
	download_url: string;
	expected_sha256: string;
	is_english_only: boolean;
	is_downloaded: boolean;
}

export type WhisperModelDownloadStatus =
	| "queued"
	| "downloading"
	| "verifying"
	| "completed"
	| "cancelled"
	| "error";

export interface WhisperModelDownloadProgress {
	model_id: string;
	status: WhisperModelDownloadStatus;
	downloaded_bytes: number;
	total_bytes: number | null;
	percent: number | null;
	message: string | null;
}

export type LocalWhisperModelLoadStatus = "started" | "completed" | "error";

export interface LocalWhisperModelLoadEvent {
	status: LocalWhisperModelLoadStatus;
	message: string | null;
}

export interface SystemEvent {
	timestamp: string;
	event_type: string;
	message: string;
	details: string | null;
}

export interface CommandErrorPayload {
	message: string;
	error_type: string;
	code?: string | null;
	details?: string | null;
	retryable?: boolean | null;
	request_id?: string | null;
}

export interface PipelineErrorPayload {
	message: string;
	request_id: string | null;
}

export type PipelineStateEvent =
	| "idle"
	| "recording"
	| "transcribing"
	| "routing"
	| "rewriting"
	| "error";

export type PipelineTranscriptReadyPayload = string;

export type EmptyEventPayload = null;

export interface SttPartialTranscriptPayload {
	text: string;
}

export interface PolicyConstraintViolation {
	path: string;
	reason?: string | null;
}

export interface LicenseTransitionPayload {
	from: LicenseStatus;
	to: LicenseStatus;
	occurred_at: string;
	reason: string;
}

export type SettingsChangedPayload =
	| ({
			settings_revision?: number;
			policy_normalized?: boolean;
			policy_constraints_applied?: boolean;
			license_state_changed?: boolean;
			license_transition?: LicenseTransitionPayload;
			policy_violations?: PolicyConstraintViolation[];
	  } & Record<string, unknown>)
	| Record<string, unknown>;

export type LicenseTier = "community" | "personal" | "enterprise";

export type LicenseStatus = "signed_out" | "active" | "grace" | "expired";

export interface TierLimits {
	stt_seconds_monthly: number;
	llm_tokens_monthly: number;
	requests_per_day: number;
}

export interface UsageStats {
	stt_seconds_used: number;
	llm_tokens_used: number;
	requests_today: number;
}

export type OrgInferenceMode = "org_byok" | "managed";

export interface OrgContext {
	org_id: string;
	org_name: string;
	inference_mode: OrgInferenceMode | null;
}

export interface LicenseState {
	tier: LicenseTier;
	status: LicenseStatus;
	user_id: string | null;
	email: string | null;
	org: OrgContext | null;
	expires_at: string | null;
	cached_at: string;
	last_validated_at: string | null;
	usage: UsageStats;
	limits: TierLimits;
	portal_available: boolean;
}

export type AuthReasonCode =
	| "reauth_required"
	| "token_invalid"
	| "membership_missing"
	| "insufficient_tier"
	| "policy_denied"
	| "auth_not_configured"
	| "unknown";

export type AuthPolicyStatus = "allow" | "deny";

export interface LicenseAuthContext {
	authenticated: boolean;
	secure_session_present: boolean;
	subject_id: string | null;
	issuer: string | null;
	mode: LicenseTier;
	org_id: string | null;
	entitlements: string[];
	policy_status: AuthPolicyStatus;
	reason_code: AuthReasonCode | null;
}

export type TokenExchangeDecision = "direct_idp_token" | "adopt_token_exchange";

export interface TokenExchangeTriggerSet {
	multi_idp_required: boolean;
	kill_switch_required: boolean;
	embedded_claims_required: boolean;
	desktop_idp_agnostic_required: boolean;
	reviewed_at: string | null;
	decision: TokenExchangeDecision;
}

export interface SessionExchangeResponse {
	enabled: boolean;
	decision: TokenExchangeDecision;
	trigger_set: TokenExchangeTriggerSet;
	session_token: string | null;
	refresh_token: string | null;
	expires_at: string | null;
	claims: Record<string, unknown>;
	reason: string;
}

export type EnterprisePersonaType = "byok" | "managed" | "mixed-policy";

export type EnterprisePersonaEnvironment =
	| "local"
	| "preview"
	| "staging"
	| "production";

export interface EnterprisePersonaState {
	context_key: string | null;
	persona_type: EnterprisePersonaType | null;
	test_access_active: boolean;
	test_access_expires_at: string | null;
	environment: EnterprisePersonaEnvironment;
	source: "storage" | "event" | "none";
	updated_at: string | null;
}

export type ManagedInferenceMode = "managed" | "byok";

export type ManagedErrorCategory =
	| "unauthorized"
	| "ineligible"
	| "over_quota"
	| "temporarily_unavailable";

export interface ManagedError {
	category: ManagedErrorCategory;
	code: string;
	message: string;
	reason_code?: AuthReasonCode | null;
	request_id?: string | null;
	retry_after_seconds?: number | null;
}

export interface ManagedUsageCounter {
	metric: "stt_seconds" | "llm_tokens" | "managed_requests";
	used: number;
	limit: number;
	warning_thresholds?: number[];
	window: "daily" | "monthly";
}

export interface ManagedUsageState {
	tier: LicenseTier;
	mode: ManagedInferenceMode;
	counters: ManagedUsageCounter[];
}

export interface ConnectionStateChangedPayload {
	state: ConnectionState;
}

export interface OverlayAudioLevelPayload {
	seq: number;
	rms: number;
	peak: number;
	wave_seq?: number;
	mins?: number[];
	maxes?: number[];
}

export interface MicTestAudioLevelPayload {
	active: boolean;
	session_id: number;
	seq: number;
	rms: number;
	peak: number;
}

export interface AudioInputDeviceInfo {
	id: string;
	name: string;
}

export type QuickAskStartedPayload = {
	question?: string;
	provider?: string;
	model?: string | null;
};

export type QuickAskAnswerPayload =
	| {
			ok: true;
			answer: string;
			provider_used?: string;
			model_used?: string;
			duration_ms?: number;
	  }
	| {
			ok: false;
			error: string;
	  };

export type LocalWhisperComputeBackend = "cpu" | "cuda";

export interface LocalWhisperBackendStatus {
	build_has_local_whisper: boolean;
	build_has_cuda: boolean;
	compute: LocalWhisperComputeBackend;
	reason: string | null;
	missing_dlls: string[];
	observed?: {
		nvidia_smi_available: boolean;
		pid: number;
		cuda_process_present: boolean | null;
		used_gpu_memory_mb: number | null;
		error: string | null;
	};
}

export type WidgetPosition =
	| "center"
	| "top-left"
	| "top-center"
	| "top-right"
	| "bottom-left"
	| "bottom-center"
	| "bottom-right";

export type OutputMode = "paste" | "paste_and_clipboard" | "clipboard";

export type QuickAskDismissMode = "manual" | "auto";

export type TranscriptionRetentionUnit = "days" | "hours";

export type RequestLogsRetentionMode = "amount" | "time";

export type SettingsGuideState = "pending" | "skipped" | "completed";

export type PolicySource =
	| "none"
	| "file"
	| "cloud"
	| "cached"
	| "degraded_expired";

export interface PolicyEnforcedField {
	path: string;
	reason?: string | null;
	effective_value?: unknown;
}

export interface PolicyState {
	source: PolicySource;
	eligible?: boolean;
	is_valid: boolean;
	active_policy_id?: string | null;
	active_version?: number | null;
	last_sync_at?: string | null;
	last_success_at?: string | null;
	last_updated: string | null;
	expires_at: string | null;
	failure_reason?: string | null;
	enforced_count?: number;
	version: string | null;
	enforced_fields: PolicyEnforcedField[];
}

export interface PolicyDiagnosticField {
	path: string;
	effective_value?: unknown;
	reason?: string | null;
}

export interface PolicyDiagnosticExport {
	generated_at: string;
	policy_state: PolicyState;
	enforced_fields: PolicyDiagnosticField[];
	redaction_applied: boolean;
}

// ============================================================================
// Network / proxy settings
// ============================================================================

export type ProxyMode = "no_proxy" | "system" | "manual";

export type TrustedCaCertFormat = "pem" | "der";

export interface TrustedCaCertificate {
	id: string;
	file_name: string;
	format: TrustedCaCertFormat;
	data_base64: string;
}

export interface ManualProxySettings {
	/** Proxy URL applied to both http + https. Example: "http://127.0.0.1:8080" */
	proxy_url: string;
	/** Comma- or whitespace-separated bypass list (NO_PROXY semantics). */
	no_proxy: string;
	/** Optional basic auth username. */
	username: string;
	/** Optional basic auth password. */
	password: string;
}

export interface ProxySettings {
	mode: ProxyMode;
	manual: ManualProxySettings;
	trusted_ca_certificates: TrustedCaCertificate[];
	/** When true, accept invalid TLS certs (including self-signed). */
	danger_accept_invalid_certs: boolean;
}

export interface WindowsInternetProxySettings {
	proxy_enable: boolean | null;
	proxy_server: string | null;
	proxy_override: string | null;
	auto_config_url: string | null;
}

export interface SystemProxyInfo {
	env_http_proxy: string | null;
	env_https_proxy: string | null;
	env_no_proxy: string | null;

	// Windows-only, best-effort.
	windows_internet_settings: WindowsInternetProxySettings | null;
}

export type OpenAiReasoningEffort =
	| "none"
	| "minimal"
	| "low"
	| "medium"
	| "high"
	| "xhigh";

export type CostTimeframe = "24h" | "7d" | "30d" | "90d" | "all";

export interface CostSummary {
	timeframe: CostTimeframe | string;
	total_usd_micros: number;
	events_total: number;
	events_with_cost: number;
	earliest_included_at: string | null;
	latest_included_at: string | null;
}

export interface ProviderCostTotal {
	provider: string;
	total_usd_micros: number;
	events_total: number;
	events_with_cost: number;
}

export interface CostByProvider {
	timeframe: CostTimeframe | string;
	providers: ProviderCostTotal[];
}

export type ModelPricingKind = "stt" | "llm";

export interface SttModelPricing {
	usd_micros_per_minute?: number | null;
	usd_micros_per_hour?: number | null;
	min_billed_secs?: number | null;
}

export interface LlmModelPricing {
	input_usd_micros_per_1m: number;
	cached_input_usd_micros_per_1m?: number | null;
	output_usd_micros_per_1m: number;
}

export interface ModelPricing {
	kind: ModelPricingKind;
	provider: string;
	model: string;
	stt?: SttModelPricing | null;
	llm?: LlmModelPricing | null;
}

export type LocalWhisperLoadMode = "manual" | "on_transcribe" | "on_launch";

// What the window close (X) button does for the main/settings window.
//
// NOTE: We previously used "close_window" (destroy the window but keep the tray app running).
// That option is now treated as legacy and maps to "exit_program".
export type MainWindowCloseBehavior = "exit_program" | "minimize_to_tray";

export interface AppSettings {
	// Settings schema version (used for migrations).
	settings_version: number;
	policy_state: PolicyState;
	license_state: LicenseState;
	token_exchange_trigger_set: TokenExchangeTriggerSet;
	toggle_hotkey: HotkeyConfig | null;
	hold_hotkey: HotkeyConfig | null;
	paste_last_hotkey: HotkeyConfig | null;
	retry_hotkey: HotkeyConfig | null;
	quick_ask_hold_hotkey: HotkeyConfig | null;
	quick_ask_toggle_hotkey: HotkeyConfig | null;
	/** Card-based shortcuts (supports multiple per action). */
	hotkey_shortcuts: HotkeyShortcutCard[];

	/** When true, backend emits extra hotkey diagnostics to the System Events panel. */
	hotkey_debug_enabled: boolean;

	selected_mic_id: string | null;
	sound_enabled: boolean;
	audio_cue: AudioCue;
	/** User-selected accent color (hex). */
	accent_color: string | null;
	// Global gate for the optional LLM rewrite step
	rewrite_llm_enabled: boolean;

	// Quick Replace (global defaults)
	// NOTE: This is used by the backend; profiles may inherit from it.
	quick_replace_enabled: boolean;
	cleanup_prompt_sections: CleanupPromptSections | null;
	rewrite_program_prompt_profiles: RewriteProgramPromptProfile[];
	stt_provider: string | null;
	stt_model: string | null;
	stt_language: string;
	// Global STT prompt (applies to all transcriptions when supported by the selected provider/model)
	stt_transcription_prompt: string | null;
	// Prefer the managed STT transport for catalog-supported models when the
	// signed-in account is entitled to managed inference.
	stt_use_managed_inference: boolean;
	// When true and a realtime STT model is selected, committed chunks are pasted
	// live during recording instead of waiting until the end.
	stt_live_output: boolean;
	// When true, simulate realtime streaming for batch-only models by periodically
	// sending audio chunks to the batch API during recording.
	stt_simulated_streaming: boolean;
	// AquaVoice server override (optional)
	aquavoice_base_url: string | null;
	// Whisper server base URL (OpenAI-compatible API; optional)
	whisper_server_base_url: string | null;

	// Ollama server base URL (optional)
	ollama_url: string | null;

	// Local Whisper model id (e.g. "base", "tinyen"). Only meaningful when the
	// Local Whisper feature is compiled in.
	local_whisper_model_id: string | null;

	// When to load the local whisper.cpp model file.
	local_whisper_load_mode: LocalWhisperLoadMode;

	// Global proxy configuration for outgoing HTTP requests
	proxy_settings: ProxySettings;
	llm_provider: string | null;
	llm_model: string | null;

	// Quick Ask (global defaults)
	quick_ask_provider: string | null;
	quick_ask_model: string | null;
	quick_ask_system_prompt: string | null;
	quick_ask_dismiss_mode: QuickAskDismissMode;

	// When enabled, Quick Ask will attempt to capture the currently highlighted text
	// (via a copy probe) and include it as additional context.
	// NOTE: This is a global setting (not per-profile) and is disabled by default.
	quick_ask_include_selected_text: boolean;

	// Windows-only: allow clipboard-based fallback for context capture (default off).
	windows_clipboard_fallback_for_context_capture: boolean;

	// Quick Ask conversation history (ephemeral; in-memory only)
	quick_ask_conversation_history_enabled: boolean;
	// How many previous Q/A turns to include when enabled.
	quick_ask_conversation_history_count: number;

	quick_ask_openai_reasoning_effort: OpenAiReasoningEffort | null;
	quick_ask_anthropic_thinking_budget: number | null;
	quick_ask_gemini_thinking_budget: number | null;
	quick_ask_gemini_thinking_level: "minimal" | "low" | "medium" | "high" | null;

	// Provider-specific knobs
	// When true, treat Cerebras usage as free-tier for stats filtering.
	cerebras_free_tier: boolean;

	// When true, treat Groq usage as free-tier (UI-only for now; kept in settings for future backend usage).
	groq_free_tier: boolean;

	// When true, treat Cohere usage as free-tier for stats filtering.
	cohere_free_tier: boolean;

	// When true, treat AssemblyAI usage as free-tier for stats filtering.
	assemblyai_free_tier: boolean;

	// When true, treat Speechmatics usage as free-tier for stats filtering.
	speechmatics_free_tier: boolean;

	// Optional per-provider reasoning/thinking knobs.
	// These are ignored unless the selected provider/model supports them.
	openai_reasoning_effort: OpenAiReasoningEffort | null;
	anthropic_thinking_budget: number | null;
	gemini_thinking_budget: number | null;
	gemini_thinking_level: "minimal" | "low" | "medium" | "high" | null;

	playing_audio_handling: PlayingAudioHandling;
	stt_timeout_seconds: number | null;
	overlay_mode: OverlayMode;
	/** When true, show detailed phase text (routing/transcribing/rewriting) in the overlay. */
	overlay_show_detailed_loading: boolean;
	/** Which monitor the overlay windows should appear on. */
	overlay_monitor_target: OverlayMonitorTarget;
	widget_position: WidgetPosition;
	output_mode: OutputMode;
	output_hit_enter: boolean;
	// When true, output injection will not read/restore the clipboard.
	output_clipboard_privacy_mode: boolean;
	// When true, avoid pasting into sensitive targets (e.g., password fields).
	output_smart_paste_protection: boolean;

	/** What the window close button does for the main/settings window. */
	main_window_close_behavior: MainWindowCloseBehavior;

	// Hallucination protection (quiet-audio gate)
	quiet_audio_gate_enabled: boolean;
	quiet_audio_min_duration_secs: number;
	quiet_audio_rms_dbfs_threshold: number;
	quiet_audio_peak_dbfs_threshold: number;
	// Extra protection: if enabled, also require that VAD detects speech.
	quiet_audio_require_speech: boolean;

	// Capture behavior (Hot Mic + recovery)
	// When enabled, keep the microphone stream open while idle and maintain a rolling pre-roll.
	hot_mic_enabled: boolean;
	// How much audio to keep before record start (ms). Only used when hot_mic_enabled is true.
	hot_mic_pre_roll_ms: number;
	// When enabled, watchdog the mic stream and attempt auto-recovery on hangs/disconnects.
	mic_auto_recover_enabled: boolean;

	// Experimental: noise gate threshold (dBFS). null means off.
	noise_gate_threshold_dbfs: number | null;

	// Voice pickup (stop-time preprocessing)
	audio_downmix_to_mono: boolean;
	audio_resample_to_16khz: boolean;
	audio_highpass_enabled: boolean;
	audio_agc_enabled: boolean;
	audio_noise_suppression_enabled: boolean;

	// How many recordings/history entries to retain
	max_saved_recordings: number;

	// Time-based retention for transcriptions/history.
	// 0 means keep forever.
	transcription_retention_mode: RequestLogsRetentionMode;
	transcription_retention_amount: number;
	transcription_retention_unit: TranscriptionRetentionUnit;
	transcription_retention_value: number;
	// If enabled, deleting old transcriptions also deletes their recordings (best-effort).
	transcription_retention_delete_recordings: boolean;

	// Recordings retention (amount or time-based).
	recordings_retention_mode: RequestLogsRetentionMode;
	recordings_retention_amount: number;
	recordings_retention_unit: TranscriptionRetentionUnit;
	recordings_retention_value: number;

	// Persisted stats retention (usage/cost events).
	// 0 means keep forever.
	stats_retention_unit: TranscriptionRetentionUnit;
	stats_retention_value: number;
	// Defensive cap for on-disk stats storage.
	stats_retention_max_bytes: number;

	// Request logs retention (in-memory request log history)
	request_logs_retention_mode: RequestLogsRetentionMode;
	// Only used when mode === "amount"
	request_logs_retention_amount: number;
	// Only used when mode === "time" (0 = forever)
	request_logs_retention_days: number;
	// When true, hide full request payloads in the UI (privacy mode).
	request_logs_privacy_mode: boolean;

	// Product analytics / telemetry disclosure.
	// Analytics stay opt-out-by-default, but disclosure must still be resolved
	// before the frontend may emit any product analytics events.
	posthog_analytics_enabled: boolean;
	telemetry_disclosure_acknowledged_at: string | null;
	telemetry_disclosure_version: string | null;

	// Backups
	// Optional: GitHub Gist id used for "push/pull" backups.
	github_backup_gist_id: string | null;

	// ============================================================================
	// OCR (Active Window Context) provider configuration
	// ============================================================================

	/** Base URL for the OCR service (e.g., "http://localhost:8000" for vLLM, "https://api.openai.com" for OpenAI). */
	ocr_base_url: string | null;
	/** OCR model identifier (e.g., "lightonai/LightOnOCR-1B-1025"). */
	ocr_model: string | null;
	/** How OCR requests are authenticated. */
	ocr_auth_mode: OcrAuthMode;
	/** OCR system prompt (optional override). */
	ocr_prompt: string | null;
	/** Max tokens for OCR response. */
	ocr_max_tokens: number | null;
	/** Temperature for OCR LLM inference. */
	ocr_temperature: number | null;
	/** Top-p for OCR LLM inference. */
	ocr_top_p: number | null;
	/** Request timeout for OCR requests (ms). Keep small so OCR never blocks the tool. */
	ocr_request_timeout_ms: number | null;
	/** Maximum characters of OCR output included in downstream LLM prompts. */
	ocr_context_max_chars: number | null;

	// Per-tool OCR context mode (tri-state)
	/** OCR mode for Rewrite tool. */
	rewrite_active_window_ocr_mode: ActiveWindowOcrMode;
	/** OCR mode for Quick Replace tool. */
	quick_replace_active_window_ocr_mode: ActiveWindowOcrMode;
	/** OCR mode for Quick Ask tool. */
	quick_ask_active_window_ocr_mode: ActiveWindowOcrMode;

	/** When to capture screenshot in Auto mode: on_stop or on_start (opportunistic). */
	ocr_auto_capture_timing: OcrAutoCaptureTiming;

	/** Enable robust image validation to prevent OCR hallucinations on blank/uniform images. */
	ocr_hallucination_protection: boolean;

	/** Variance threshold for image validation (higher = more permissive). Default 2000. */
	ocr_hallucination_threshold: number;

	/** Max dimension (width or height) for resizing captured images. 0 = no resize. */
	ocr_resize_max_dimension: number;

	/** Resize filter: "nearest", "triangle", "catmullrom", "lanczos3". */
	ocr_resize_filter: OcrResizeFilter;
}

export interface SettingsDoctorIssue {
	key: string;
	message: string;
}

export interface SettingsDoctorReport {
	issues: SettingsDoctorIssue[];
}

export interface OpenWindowInfo {
	title: string;
	process_path: string;
}

export interface CacheRouterEmbeddingsResponse {
	provider: string;
	model: string;
	total_hints: number;
	cached_now: number;
	skipped_existing: number;
	stored_inserted: number;
	stored_updated: number;
}

// ============================================================================
// LLM API
// ============================================================================

export interface TestLlmRewriteResponse {
	output: string;
	provider_used: string;
	model_used: string;
}

export interface LlmProviderInfo {
	id: string;
	name: string;
	requires_api_key: boolean;
	default_model: string;
	models: string[];
}

export interface ModelOption {
	value: string;
	label: string;
	disabled?: boolean;
}

export interface LlmCompleteResponse {
	output: string;
	provider_used: string;
	model_used: string;
}

export interface IterateRewritePromptResponse {
	improved_prompt: string;
	provider_used: string;
	model_used: string;
}

export interface TestRewriteWithPromptResponse {
	output: string;
	provider_used: string;
	model_used: string;
}

export interface AudioLevelStats {
	duration_secs: number;
	rms: number;
	peak: number;
}

export interface AudioCaptureDiagnostics {
	stats: AudioLevelStats;
	// null when speech detection wasn't computed for the last recording.
	speech_detected: boolean | null;
}

export interface AudioSettingsTestWavs {
	raw_wav_base64: string;
	processed_wav_base64: string;
}

// ============================================================================
// Config API - Using Tauri commands
// ============================================================================

export interface DefaultSectionsResponse {
	system: string;
}

// ============================================================================
// Request Logs API
// ============================================================================

export type LogLevel = "debug" | "info" | "warn" | "error";
export type RequestStatus = "in_progress" | "success" | "error" | "cancelled";
export type RequestKind = "transcription" | "quick_ask" | "quick_replace";

export interface LogEntry {
	timestamp: string;
	level: LogLevel;
	message: string;
	details: string | null;
}

export interface RequestLog {
	id: string;

	// High-level request kind for UI grouping/filtering.
	// Optional for backward compatibility with older logs.
	kind?: RequestKind;
	started_at: string;
	ended_at: string | null;
	stt_provider: string;
	stt_model: string | null;
	llm_provider: string | null;
	llm_model: string | null;
	managed_inference?: boolean;

	profile_id?: string | null;
	profile_name?: string | null;

	preset_id?: string | null;
	preset_name?: string | null;

	raw_transcript: string | null;
	final_text: string | null;

	// LLM rewrite clipboard context (when enabled)
	rewrite_clipboard_context?: string | null;

	// Quick Ask fields (when kind === "quick_ask")
	quick_ask_question?: string | null;
	quick_ask_context_text?: string | null;
	quick_ask_clipboard_context?: string | null;
	quick_ask_answer?: string | null;
	quick_ask_provider?: string | null;
	quick_ask_model?: string | null;
	quick_ask_duration_ms?: number | null;

	// Quick Replace fields (when kind === "quick_replace")
	quick_replace_instructions?: string | null;
	quick_replace_selected_text?: string | null;
	quick_replace_output_text?: string | null;
	quick_replace_clipboard_context?: string | null;
	quick_replace_provider?: string | null;
	quick_replace_model?: string | null;
	quick_replace_duration_ms?: number | null;

	// Total request processing duration (ms). Excludes recording time when available.
	total_duration_ms: number | null;
	stt_duration_ms: number | null;
	llm_duration_ms: number | null;

	// LLM rewrite outcome (optional; added for clearer debugging when rewrite is skipped).
	llm_outcome?: "not_attempted" | "succeeded" | "timed_out" | "failed" | null;
	llm_not_attempted_reason?:
		| "quiet_audio_gate"
		| "no_speech_detected_by_vad"
		| "disabled_default_profile"
		| "disabled_profile"
		| "disabled_preset"
		| "provider_unavailable"
		| "unknown"
		| null;
	llm_error_message?: string | null;

	// Intent router (preset selection) diagnostics
	router_duration_ms?: number | null;
	router_strategy?: string | null;
	router_scores?: Array<{
		preset_id: string;
		preset_name: string;
		score: number | null;
		selected: boolean;
	}> | null;
	status: RequestStatus;
	error_message: string | null;
	entries: LogEntry[];

	stt_is_free_tier: boolean;
	llm_is_free_tier: boolean;
	stt_estimated_cost_usd_micros: number | null;
	llm_estimated_cost_usd_micros: number | null;

	// Optional provider payloads for debugging.
	// Binary audio is redacted and represented with placeholders.
	stt_request_json?: unknown;
	stt_response_json?: unknown;
	llm_request_json?: unknown;
	llm_response_json?: unknown;

	// Quick Ask payloads (optional)
	quick_ask_request_json?: unknown;
	quick_ask_response_json?: unknown;

	// Quick Replace payloads (optional)
	quick_replace_request_json?: unknown;
	quick_replace_response_json?: unknown;

	// Optional router payloads for debugging.
	// For embeddings this may be an array of calls/responses.
	router_request_json?: unknown;
	router_response_json?: unknown;

	// ============================================================================
	// OCR (Active Window Context) request log fields
	// ============================================================================

	/** Effective OCR mode used for the current flow (e.g. "off" | "auto" | "manual"). */
	ocr_effective_mode?: string | null;
	/** OCR task lifecycle status for this request. */
	ocr_status?: string | null;
	/** Whether OCR context was included in the prompt. */
	ocr_context_present?: boolean;
	/** Number of OCR characters included (if any). */
	ocr_context_chars?: number | null;
	/** OCR context text that was attached to the prompt (if any). */
	ocr_context_text?: string | null;
	/** OCR duration in milliseconds (if it completed or failed). */
	ocr_duration_ms?: number | null;
	/** Best-effort, user-friendly OCR failure reason (if OCR was enabled but failed). */
	ocr_failed_reason?: string | null;
	/** If OCR was not started (or not used), a stable reason code. */
	ocr_not_attempted_reason?: string | null;
	/** When OCR started for this request (if it started). */
	ocr_started_at?: string | null;
	/** OCR request payload preview (redacted; does NOT include image bytes). */
	ocr_request_json?: unknown;
	/** OCR response payload (redacted). */
	ocr_response_json?: unknown;
}

export interface RecordingsStats {
	count: number;
	bytes: number;
}

export interface DataStorageSummary {
	recordings_count: number;
	recordings_bytes: number;
	history_count: number;
	history_bytes: number;
	request_logs_count: number;
	stats_files_count: number;
	stats_bytes: number;
	settings_bytes: number;
	api_keys_set_count: number;
}
