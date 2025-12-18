import { Loader, Select, Slider, Text } from "@mantine/core";
import { useEffect, useState } from "react";
import {
	useAvailableProviders,
	useSettings,
	useUpdateLLMModel,
	useUpdateLLMProvider,
	useUpdateSTTModel,
	useUpdateSTTProvider,
	useUpdateSTTTimeout,
} from "../../lib/queries";
import { tauriAPI } from "../../lib/tauri";

const DEFAULT_STT_TIMEOUT = 0.8;

// Model options for each STT provider
const STT_MODELS: Record<string, { value: string; label: string }[]> = {
	groq: [
		{ value: "whisper-large-v3", label: "Whisper Large V3" },
		{ value: "whisper-large-v3-turbo", label: "Whisper Large V3 Turbo" },
	],
	openai: [
		{ value: "gpt-4o-audio-preview", label: "GPT-4o Audio Preview" },
		{ value: "gpt-4o-mini-audio-preview", label: "GPT-4o Mini Audio Preview" },
		{ value: "whisper-1", label: "Whisper 1 (Legacy)" },
	],
	deepgram: [
		{ value: "nova-2", label: "Nova 2" },
		{ value: "nova", label: "Nova" },
		{ value: "enhanced", label: "Enhanced" },
		{ value: "base", label: "Base" },
	],
	whisper: [], // Local whisper has its own model management
};

// Model options for each LLM provider
const LLM_MODELS: Record<string, { value: string; label: string }[]> = {
	groq: [
		{ value: "llama-3.3-70b-versatile", label: "Llama 3.3 70B Versatile" },
		{ value: "llama-3.1-8b-instant", label: "Llama 3.1 8B Instant" },
		{ value: "mixtral-8x7b-32768", label: "Mixtral 8x7B" },
	],
	openai: [
		{ value: "gpt-4o-mini", label: "GPT-4o Mini" },
		{ value: "gpt-4o", label: "GPT-4o" },
		{ value: "gpt-4-turbo", label: "GPT-4 Turbo" },
	],
	anthropic: [
		{ value: "claude-3-5-haiku-latest", label: "Claude 3.5 Haiku" },
		{ value: "claude-3-5-sonnet-latest", label: "Claude 3.5 Sonnet" },
		{ value: "claude-3-opus-latest", label: "Claude 3 Opus" },
	],
	ollama: [], // Ollama models are dynamic based on what's installed
};

export function ProvidersSettings() {
	const { data: settings, isLoading: isLoadingSettings } = useSettings();
	const { data: availableProviders, isLoading: isLoadingProviders } =
		useAvailableProviders();

	// Wait for settings (source of truth) and provider list (for options)
	const isLoadingProviderData = isLoadingSettings || isLoadingProviders;
	const updateSTTProvider = useUpdateSTTProvider();
	const updateSTTModel = useUpdateSTTModel();
	const updateLLMProvider = useUpdateLLMProvider();
	const updateLLMModel = useUpdateLLMModel();
	const updateSTTTimeout = useUpdateSTTTimeout();

	const handleSTTProviderChange = (value: string | null) => {
		if (!value) return;
		// Save to local settings (Tauri) then notify overlay window to sync to server
		updateSTTProvider.mutate(value, {
			onSuccess: () => {
				// Reset model to first available when provider changes
				const models = STT_MODELS[value];
				const firstModel = models?.[0];
				if (firstModel) {
					updateSTTModel.mutate(firstModel.value);
				}
				tauriAPI.emitSettingsChanged();
			},
		});
	};

	const handleSTTModelChange = (value: string | null) => {
		if (!value) return;
		updateSTTModel.mutate(value, {
			onSuccess: () => {
				tauriAPI.emitSettingsChanged();
			},
		});
	};

	const handleLLMProviderChange = (value: string | null) => {
		if (!value) return;
		// Save to local settings (Tauri) then notify overlay window to sync to server
		updateLLMProvider.mutate(value, {
			onSuccess: () => {
				// Reset model to first available when provider changes
				const models = LLM_MODELS[value];
				const firstModel = models?.[0];
				if (firstModel) {
					updateLLMModel.mutate(firstModel.value);
				}
				tauriAPI.emitSettingsChanged();
			},
		});
	};

	const handleLLMModelChange = (value: string | null) => {
		if (!value) return;
		updateLLMModel.mutate(value, {
			onSuccess: () => {
				tauriAPI.emitSettingsChanged();
			},
		});
	};

	const handleSTTTimeoutChange = (value: number) => {
		// Save to local settings (Tauri) then notify overlay window to sync to server
		updateSTTTimeout.mutate(value, {
			onSuccess: () => {
				tauriAPI.emitSettingsChanged();
			},
		});
	};

	// Get the current timeout value from settings, falling back to default
	const currentTimeout = settings?.stt_timeout_seconds ?? DEFAULT_STT_TIMEOUT;

	// Local state for smooth slider dragging
	const [sliderValue, setSliderValue] = useState(currentTimeout);

	// Sync local state when server value changes
	useEffect(() => {
		setSliderValue(currentTimeout);
	}, [currentTimeout]);

	// Group providers by cloud/local for dropdown display
	const sttCloudProviders =
		availableProviders?.stt
			.filter((p) => !p.is_local)
			.map((p) => ({ value: p.value, label: p.label })) ?? [];
	const sttLocalProviders =
		availableProviders?.stt
			.filter((p) => p.is_local)
			.map((p) => ({ value: p.value, label: p.label })) ?? [];
	const sttProviderOptions = [
		{ group: "Cloud", items: sttCloudProviders },
		{ group: "Local", items: sttLocalProviders },
	];

	const llmCloudProviders =
		availableProviders?.llm
			.filter((p) => !p.is_local)
			.map((p) => ({ value: p.value, label: p.label })) ?? [];
	const llmLocalProviders =
		availableProviders?.llm
			.filter((p) => p.is_local)
			.map((p) => ({ value: p.value, label: p.label })) ?? [];
	const llmProviderOptions = [
		{ group: "Cloud", items: llmCloudProviders },
		{ group: "Local", items: llmLocalProviders },
	];

	// Get available models for the selected providers
	const sttModelOptions = settings?.stt_provider
		? (STT_MODELS[settings.stt_provider] ?? [])
		: [];
	const llmModelOptions = settings?.llm_provider
		? (LLM_MODELS[settings.llm_provider] ?? [])
		: [];

	return (
		<>
			{/* STT Provider */}
			<div className="settings-row">
				<div>
					<p className="settings-label">Speech-to-Text Provider</p>
					<p className="settings-description">Service for transcribing audio</p>
				</div>
				<div style={{ display: "flex", alignItems: "center", gap: 8 }}>
					{isLoadingProviderData ? (
						<Loader size="sm" color="gray" />
					) : (
						<Select
							data={sttProviderOptions}
							value={settings?.stt_provider ?? null}
							onChange={handleSTTProviderChange}
							placeholder="Select provider"
							withCheckIcon={false}
							disabled={
								sttCloudProviders.length === 0 && sttLocalProviders.length === 0
							}
							styles={{
								input: {
									backgroundColor: "var(--bg-elevated)",
									borderColor: "var(--border-default)",
									color: "var(--text-primary)",
									minWidth: 200,
								},
							}}
						/>
					)}
				</div>
			</div>

			{/* STT Model - only show if provider has models */}
			{sttModelOptions.length > 0 && (
				<div className="settings-row">
					<div>
						<p className="settings-label">STT Model</p>
						<p className="settings-description">
							Model to use for transcription
						</p>
					</div>
					<Select
						data={sttModelOptions}
						value={settings?.stt_model ?? sttModelOptions[0]?.value ?? null}
						onChange={handleSTTModelChange}
						placeholder="Select model"
						withCheckIcon={false}
						styles={{
							input: {
								backgroundColor: "var(--bg-elevated)",
								borderColor: "var(--border-default)",
								color: "var(--text-primary)",
								minWidth: 200,
							},
						}}
					/>
				</div>
			)}

			{/* LLM Provider */}
			<div className="settings-row">
				<div>
					<p className="settings-label">Language Model Provider</p>
					<p className="settings-description">AI service for text formatting</p>
				</div>
				<div style={{ display: "flex", alignItems: "center", gap: 8 }}>
					{isLoadingProviderData ? (
						<Loader size="sm" color="gray" />
					) : (
						<Select
							data={llmProviderOptions}
							value={settings?.llm_provider ?? null}
							onChange={handleLLMProviderChange}
							placeholder="Select provider"
							withCheckIcon={false}
							disabled={
								llmCloudProviders.length === 0 && llmLocalProviders.length === 0
							}
							styles={{
								input: {
									backgroundColor: "var(--bg-elevated)",
									borderColor: "var(--border-default)",
									color: "var(--text-primary)",
									minWidth: 200,
								},
							}}
						/>
					)}
				</div>
			</div>

			{/* LLM Model - only show if provider has models */}
			{llmModelOptions.length > 0 && (
				<div className="settings-row">
					<div>
						<p className="settings-label">LLM Model</p>
						<p className="settings-description">
							Model to use for text formatting
						</p>
					</div>
					<Select
						data={llmModelOptions}
						value={settings?.llm_model ?? llmModelOptions[0]?.value ?? null}
						onChange={handleLLMModelChange}
						placeholder="Select model"
						withCheckIcon={false}
						styles={{
							input: {
								backgroundColor: "var(--bg-elevated)",
								borderColor: "var(--border-default)",
								color: "var(--text-primary)",
								minWidth: 200,
							},
						}}
					/>
				</div>
			)}

			{/* STT Timeout */}
			<div className="settings-row">
				<div style={{ flex: 1 }}>
					<p className="settings-label">STT Timeout</p>
					<p className="settings-description">
						Increase if nothing is getting transcribed
					</p>
					<div
						style={{
							marginTop: 12,
							display: "flex",
							alignItems: "center",
							gap: 12,
						}}
					>
						<Slider
							value={sliderValue}
							onChange={setSliderValue}
							onChangeEnd={handleSTTTimeoutChange}
							min={0.5}
							max={3.0}
							step={0.1}
							marks={[
								{ value: 0.5, label: "0.5s" },
								{ value: 3.0, label: "3.0s" },
							]}
							styles={{
								root: { flex: 1 },
								track: { backgroundColor: "var(--bg-elevated)" },
								bar: { backgroundColor: "var(--accent-primary)" },
								thumb: { borderColor: "var(--accent-primary)" },
								markLabel: { color: "var(--text-secondary)", fontSize: 10 },
							}}
						/>
						<Text size="xs" c="dimmed" style={{ minWidth: 32 }}>
							{sliderValue.toFixed(1)}s
						</Text>
					</div>
				</div>
			</div>
		</>
	);
}
