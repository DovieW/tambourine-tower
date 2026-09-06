import type { ManagedModel } from "./tauri";

export type ModelOption = {
	value: string;
	label: string;
	pricing?: {
		input_per_million?: number;
		output_per_million?: number;
		cache_read_per_million?: number;
		source: "models.dev";
	};
};

export type ManagedModelByokTarget = { provider: string; model: string };

// A managed-capable account must remain configurable without a successful
// catalog refresh. Keep this launch catalog synchronized with API Edge; the
// live catalog replaces it whenever discovery succeeds.
export const BUNDLED_MANAGED_MODELS: ManagedModel[] = [
	{
		id: "whisper-large-v3-turbo",
		display_name: "Whisper Large V3 Turbo",
		provider: "groq",
		capabilities: ["transcription"],
		default_for_provider: true,
	},
	{
		id: "whisper-large-v3",
		display_name: "Whisper Large V3",
		provider: "groq",
		capabilities: ["transcription"],
		default_for_provider: false,
	},
	{
		id: "gpt-4o-mini",
		display_name: "GPT-4o mini",
		provider: "openai",
		capabilities: ["chat_completions", "responses"],
		default_for_provider: true,
	},
	{
		id: "gpt-5-mini",
		display_name: "GPT-5 mini",
		provider: "openai",
		capabilities: ["chat_completions", "responses"],
		default_for_provider: false,
	},
	{
		id: "gpt-5",
		display_name: "GPT-5",
		provider: "openai",
		capabilities: ["chat_completions", "responses"],
		default_for_provider: false,
	},
	{
		id: "gpt-5.5",
		display_name: "GPT-5.5",
		provider: "openai",
		capabilities: ["chat_completions", "responses"],
		default_for_provider: false,
	},
];

export function managedModelsWithBundledFallback(
	models: ManagedModel[] | null | undefined,
): ManagedModel[] {
	// An empty Edge catalog intentionally disables all managed models.
	return models ?? BUNDLED_MANAGED_MODELS;
}

export function managedModelByokTarget(
	model: ManagedModel,
): ManagedModelByokTarget | null {
	if (model.provider === "cloudflare") return null;
	if (model.provider === "google") {
		return {
			provider: "gemini",
			model:
				model.id === "gemini-3-flash"
					? "models/gemini-3-flash-preview"
					: model.id,
		};
	}
	return { provider: model.provider, model: model.id };
}

export function managedChatModelOptions(models: ManagedModel[]): ModelOption[] {
	return models
		.filter((model) => model.capabilities.includes("chat_completions"))
		.map((model) => ({
			value: model.id,
			label: `${model.display_name} · ${model.provider}`,
		}));
}

export function managedTranscriptionModelOptions(
	models: ManagedModel[],
	provider?: string | null,
): ModelOption[] {
	return models
		.filter(
			(model) =>
				model.capabilities.includes("transcription") &&
				(!provider || model.provider === provider),
		)
		.map((model) => ({
			value: model.id,
			label: model.display_name,
		}));
}

export function isManagedModelSelection(
	models: ManagedModel[],
	capability: "chat_completions" | "transcription",
	provider: string | null | undefined,
	model: string | null | undefined,
): boolean {
	if (!provider || !model) return false;
	return models.some(
		(entry) =>
			entry.provider === provider &&
			entry.id === model &&
			entry.capabilities.includes(capability),
	);
}

// Model options for embedding providers (used by intent router).
export const EMBEDDING_MODELS: Record<string, ModelOption[]> = {
	openai: [
		{ value: "text-embedding-3-small", label: "Text Embedding 3 Small" },
		{ value: "text-embedding-3-large", label: "Text Embedding 3 Large" },
	],
	cohere: [
		{ value: "embed-v4.0", label: "Embed v4" },
		{ value: "embed-multilingual-v3.0", label: "Embed Multilingual v3" },
		{ value: "embed-english-v3.0", label: "Embed English v3" },
	],
	fireworks: [
		{ value: "fireworks/qwen3-embedding-0p6b", label: "Qwen3 Embedding 0.6B" },
		{ value: "fireworks/qwen3-embedding-4b", label: "Qwen3 Embedding 4B" },
		{ value: "fireworks/qwen3-embedding-8b", label: "Qwen3 Embedding 8B" },
	],
};

// Model options for each STT provider.
// This is intentionally shared between Settings pickers and History filters so
// they always list the same models.
export const STT_MODELS: Record<string, ModelOption[]> = {
	aquavoice: [{ value: "avalon-v1-en", label: "Avalon v1" }],
	assemblyai: [
		{
			value: "universal-streaming-english",
			label: "Universal Streaming English (Realtime)",
		},
		{
			value: "universal-streaming-multilingual",
			label: "Universal Streaming Multilingual (Realtime)",
		},
		{ value: "universal", label: "Universal" },
		{ value: "slam-1", label: "Slam-1" },
		{ value: "best", label: "Best (Legacy)" },
	],
	elevenlabs: [
		{ value: "scribe_v2", label: "Scribe v2" },
		{ value: "scribe_v1", label: "Scribe v1" },
	],
	groq: [
		{ value: "whisper-large-v3-turbo", label: "Whisper Large V3 Turbo" },
		{ value: "whisper-large-v3", label: "Whisper Large V3" },
	],
	fireworks: [
		{ value: "fireworks-asr-v2", label: "ASR v2 (Realtime)" },
		{ value: "fireworks-asr-large", label: "ASR Large (Realtime)" },
		{ value: "whisper-v3", label: "Whisper v3" },
		{ value: "whisper-v3-turbo", label: "Whisper v3 Turbo" },
	],
	openai: [
		// { value: "gpt-audio", label: "GPT Audio" },
		// { value: "gpt-audio-mini", label: "GPT Audio Mini" },
		// { value: "gpt-4o-audio-preview", label: "GPT-4o Audio Preview" },
		// { value: "gpt-4o-mini-audio-preview", label: "GPT-4o Mini Audio Preview" },
		{
			value: "gpt-4o-realtime-transcribe",
			label: "GPT-4o Realtime Transcribe",
		},
		{
			value: "gpt-4o-mini-realtime-transcribe",
			label: "GPT-4o Mini Realtime Transcribe",
		},
		{ value: "gpt-4o-transcribe", label: "GPT-4o Transcribe" },
		{
			value: "gpt-4o-transcribe-diarize",
			label: "GPT-4o Transcribe · speaker labels",
		},
		{ value: "gpt-4o-mini-transcribe", label: "GPT-4o Mini Transcribe" },
		{ value: "whisper-1", label: "Whisper-1" },
	],
	deepgram: [
		{ value: "nova-3", label: "Nova 3" },
		{ value: "nova-2", label: "Nova 2" },
		{ value: "nova", label: "Nova" },
		{ value: "enhanced", label: "Enhanced" },
		{ value: "base", label: "Base" },
	],
	speechmatics: [
		{ value: "enhanced", label: "Enhanced" },
		{ value: "standard", label: "Standard" },
	],
	"whisper-server": [
		// Keep this list conservative: most Whisper-compatible servers accept/ignore
		// the model parameter, and supported values vary by implementation.
		{ value: "whisper-1", label: "Whisper-1" },
	],
	whisper: [], // Local whisper has its own model management
};

// Model options for each LLM provider.
export const LLM_MODELS: Record<string, ModelOption[]> = {
	managed: [],
	cerebras: [
		{ value: "llama-3.3-70b", label: "Llama 3.3 70B" },
		{ value: "llama3.1-8b", label: "Llama 3.1 8B" },
		{ value: "gpt-oss-120b", label: "GPT-OSS 120B" },
		{ value: "qwen-3-32b", label: "Qwen 3 32B" },
		{
			value: "qwen-3-235b-a22b-instruct-2507",
			label: "Qwen 3 235B Instruct (Preview)",
		},
		{ value: "zai-glm-4.7", label: "GLM 4.7 (Preview)" },
		{ value: "zai-glm-4.6", label: "GLM 4.6 (Preview)" },
	],
	groq: [
		{ value: "llama-3.3-70b-versatile", label: "Llama 3.3 70B Versatile" },
		{ value: "llama-3.1-8b-instant", label: "Llama 3.1 8B Instant" },
		{ value: "openai/gpt-oss-120b", label: "GPT-OSS 120B" },
		{ value: "openai/gpt-oss-20b", label: "GPT-OSS 20B" },

		// Preview models (Groq docs: https://console.groq.com/docs/models)
		{
			value: "meta-llama/llama-4-scout-17b-16e-instruct",
			label: "Llama 4 Scout 17B 16E Instruct (Preview)",
		},
		{
			value: "meta-llama/llama-4-maverick-17b-128e-instruct",
			label: "Llama 4 Maverick 17B 128E Instruct (Preview)",
		},
		{
			value: "qwen/qwen3-32b",
			label: "Qwen3 32B Instruct (Preview)",
		},
		{
			value: "moonshotai/kimi-k2-instruct-0905",
			label: "Kimi K2 Instruct 0905 (Preview)",
		},
	],
	fireworks: [
		{
			value: "accounts/fireworks/models/llama-v3p1-8b-instruct",
			label: "Llama 3.1 8B Instruct",
		},
		{
			value: "accounts/fireworks/models/llama-v3p1-70b-instruct",
			label: "Llama 3.1 70B Instruct",
		},
	],
	openai: [
		{ value: "gpt-5.2", label: "GPT-5.2" },
		{ value: "gpt-5.1", label: "GPT-5.1" },
		{ value: "gpt-5", label: "GPT-5" },
		{ value: "gpt-5-mini", label: "GPT-5 Mini" },
		{ value: "gpt-5-nano", label: "GPT-5 Nano" },
		{ value: "gpt-4.1", label: "GPT-4.1" },
		{ value: "gpt-4.1-mini", label: "GPT-4.1 Mini" },
		{ value: "gpt-4.1-nano", label: "GPT-4.1 Nano" },
		// { value: "gpt-4o-mini", label: "GPT-4o Mini" },
		// { value: "gpt-4o", label: "GPT-4o" },
		// { value: "gpt-4-turbo", label: "GPT-4 Turbo" },
	],
	gemini: [
		// Gemini 3 (preview) - requested as explicit `models/...` IDs.
		{ value: "models/gemini-3-pro-preview", label: "Gemini 3 Pro (Preview)" },
		{
			value: "models/gemini-3-flash-preview",
			label: "Gemini 3 Flash (Preview)",
		},

		// Gemini 2.5 (stable)
		{ value: "gemini-2.5-pro", label: "Gemini 2.5 Pro" },
		// Dovie requested "2.5 Flash Pro" and "2.5 Flash Mini"; closest stable IDs:
		// - Flash (thinking-capable) and Flash-Lite (smallest)
		{ value: "gemini-2.5-flash", label: "Gemini 2.5 Flash" },
		{ value: "gemini-2.5-flash-lite", label: "Gemini 2.5 Flash-Lite" },
	],
	anthropic: [
		{ value: "claude-sonnet-4-5", label: "Claude Sonnet 4.5" },
		{ value: "claude-haiku-4-5", label: "Claude Haiku 4.5" },
		{ value: "claude-opus-4-5", label: "Claude Opus 4.5" },
		{ value: "claude-3-5-haiku-latest", label: "Claude 3.5 Haiku" },
		{ value: "claude-3-5-sonnet-latest", label: "Claude 3.5 Sonnet" },
		{ value: "claude-3-opus-latest", label: "Claude 3 Opus" },
	],
	cohere: [
		{ value: "command-a-03-2025", label: "Command A" },
		{ value: "command-r-plus-08-2024", label: "Command R+ (08/2024)" },
		{ value: "command-r-08-2024", label: "Command R (08/2024)" },
	],
	ollama: [], // Ollama models are dynamic based on what's installed
};

/**
 * Build a flat list of `{key, label}` entries from a provider→models map.
 * Each key is formatted as `provider::model.value`, label as `provider / model.label`.
 */
function listAllModelKeys(
	modelsByProvider: Record<string, ModelOption[]>,
): Array<{ key: string; label: string }> {
	const options: Array<{ key: string; label: string }> = [];
	for (const [provider, models] of Object.entries(modelsByProvider)) {
		for (const model of models) {
			options.push({
				key: `${provider}::${model.value}`,
				label: `${provider} / ${model.label}`,
			});
		}
	}
	options.sort((a, b) => a.label.localeCompare(b.label));
	return options;
}

/**
 * Whether the given STT provider+model combination supports realtime
 * concurrent streaming (audio is sent during recording for near-instant
 * transcription when recording stops).
 */
export function isRealtimeSttModel(
	provider: string | null | undefined,
	model: string | null | undefined,
): boolean {
	if (!provider || !model) return false;
	const key = `${provider}::${model}`;
	return REALTIME_STT_MODELS.has(key);
}

const REALTIME_STT_MODELS = new Set([
	"assemblyai::universal-streaming-english",
	"assemblyai::universal-streaming-multilingual",
	"deepgram::nova-3",
	"deepgram::nova-2",
	"deepgram::nova",
	"deepgram::enhanced",
	"deepgram::base",
	"elevenlabs::scribe_v2",
	"fireworks::fireworks-asr-large",
	"fireworks::fireworks-asr-v2",
	"openai::gpt-4o-realtime-transcribe",
	"openai::gpt-4o-mini-realtime-transcribe",
	"speechmatics::enhanced",
	"speechmatics::standard",
]);

export function listAllSttModelKeys(): Array<{ key: string; label: string }> {
	return listAllModelKeys(STT_MODELS);
}

export function listAllLlmModelKeys(): Array<{ key: string; label: string }> {
	return listAllModelKeys(LLM_MODELS);
}
