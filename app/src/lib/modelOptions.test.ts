import { describe, expect, it } from "vitest";
import {
	BUNDLED_MANAGED_MODELS,
	isManagedModelSelection,
	managedChatModelOptions,
	managedModelByokTarget,
	managedModelsWithBundledFallback,
	managedTranscriptionModelOptions,
} from "./modelOptions";
import type { ManagedModel } from "./tauri";

const models: ManagedModel[] = [
	{
		id: "whisper-large-v3-turbo",
		display_name: "Whisper Large V3 Turbo",
		provider: "groq",
		capabilities: ["transcription"],
		default_for_provider: true,
	},
	{
		id: "gpt-5-mini",
		display_name: "GPT-5 mini",
		provider: "openai",
		capabilities: ["chat_completions", "responses"],
		default_for_provider: false,
	},
];

describe("managed model options", () => {
	it("respects an empty published catalog", () => {
		expect(managedModelsWithBundledFallback([])).toEqual([]);
	});
	it("keeps managed selection usable when live discovery is unavailable", () => {
		expect(managedModelsWithBundledFallback(undefined)).toBe(
			BUNDLED_MANAGED_MODELS,
		);
		expect(
			managedTranscriptionModelOptions(BUNDLED_MANAGED_MODELS, "groq"),
		).toHaveLength(2);
		expect(
			managedChatModelOptions(BUNDLED_MANAGED_MODELS).length,
		).toBeGreaterThan(0);
	});

	it("keeps transcription and chat capabilities separate", () => {
		expect(managedTranscriptionModelOptions(models, "groq")).toEqual([
			{
				value: "whisper-large-v3-turbo",
				label: "Whisper Large V3 Turbo",
			},
		]);
		expect(managedChatModelOptions(models)).toEqual([
			{ value: "gpt-5-mini", label: "GPT-5 mini · openai" },
		]);
	});

	it("matches managed compatibility by provider, model, and capability", () => {
		expect(
			isManagedModelSelection(
				models,
				"transcription",
				"groq",
				"whisper-large-v3-turbo",
			),
		).toBe(true);
		expect(
			isManagedModelSelection(
				models,
				"transcription",
				"openai",
				"whisper-large-v3-turbo",
			),
		).toBe(false);
	});

	it("maps managed models to the provider-native BYOK selection", () => {
		const openAiModel = models.find((model) => model.provider === "openai");
		if (!openAiModel) throw new Error("OpenAI fixture is missing");

		expect(managedModelByokTarget(openAiModel)).toEqual({
			provider: "openai",
			model: "gpt-5-mini",
		});
		expect(
			managedModelByokTarget({
				id: "gemini-3-flash",
				display_name: "Gemini 3 Flash",
				provider: "google",
				capabilities: ["chat_completions"],
				default_for_provider: true,
			}),
		).toEqual({
			provider: "gemini",
			model: "models/gemini-3-flash-preview",
		});
		expect(
			managedModelByokTarget({
				id: "@cf/meta/llama-3.1-8b-instruct",
				display_name: "Llama 3.1 8B",
				provider: "cloudflare",
				capabilities: ["chat_completions"],
				default_for_provider: true,
			}),
		).toBeNull();
	});
});
