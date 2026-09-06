import { Button, Group, Modal, Select, Stack, Text } from "@mantine/core";
import { useState } from "react";
import { isRealtimeSttModel, STT_MODELS } from "../lib/modelOptions";
import { useLicenseAuthContext } from "../lib/queries/license";
import {
	useAvailableProviders,
	useManagedModels,
	useWhisperModels,
} from "../lib/queries/providers";
import { hasManagedInferenceAccess } from "../lib/tauri/managedInference";
import type { RecordingPreferences } from "../lib/tauri/types";

export function MeetingModelDialog({
	preferences,
	onSave,
	onClose,
	saving,
}: {
	preferences: RecordingPreferences;
	onSave: (value: RecordingPreferences) => void;
	onClose: () => void;
	saving: boolean;
}) {
	const providers = useAvailableProviders();
	const localModels = useWhisperModels(
		providers.data?.stt.some(
			(provider) =>
				provider.value === "whisper" || provider.value === "local-whisper",
		) ?? false,
	);
	const auth = useLicenseAuthContext();
	const managed = hasManagedInferenceAccess(auth.data);
	const catalog = useManagedModels(managed);
	const [selection, setSelection] = useState(
		preferences.meeting_model
			? `${preferences.meeting_model.provider}::${preferences.meeting_model.model}::${preferences.meeting_model.use_managed ? "managed" : "byok"}`
			: null,
	);
	const options = new Map<
		string,
		{
			value: string;
			label: string;
			provider: string;
			model: string;
			use_managed: boolean;
		}
	>();
	if (managed)
		for (const model of catalog.isSuccess ? (catalog.data ?? []) : []) {
			if (!model.capabilities.includes("transcription")) continue;
			const value = `${model.provider}::${model.id}::managed`;
			options.set(value, {
				value,
				label: `${model.display_name} · ${model.provider} · Managed`,
				provider: model.provider,
				model: model.id,
				use_managed: true,
			});
		}
	for (const provider of providers.data?.stt ?? []) {
		if (provider.value === "whisper" || provider.value === "local-whisper") {
			for (const model of localModels.data ?? []) {
				if (!model.is_downloaded) continue;
				const value = `local-whisper::${model.id}::byok`;
				options.set(value, {
					value,
					label: `${model.name} · Local Whisper`,
					provider: "local-whisper",
					model: model.id,
					use_managed: false,
				});
			}
		}
		for (const model of STT_MODELS[provider.value] ?? []) {
			if (isRealtimeSttModel(provider.value, model.value)) continue;
			const value = `${provider.value}::${model.value}::byok`;
			if (!options.has(value))
				options.set(value, {
					value,
					label: `${model.label} · ${provider.label}${provider.is_local ? "" : " · Your key"}`,
					provider: provider.value,
					model: model.value,
					use_managed: false,
				});
		}
	}
	const chosen = selection ? options.get(selection) : undefined;
	return (
		<Modal opened onClose={onClose} title="Meeting transcription" centered>
			<Stack>
				<Text size="sm" c="dimmed">
					Separate from dictation. Meetings are transcribed after Stop, without
					rewriting.
				</Text>
				<Select
					label="Model"
					placeholder="Choose a model"
					searchable
					data={[...options.values()]}
					value={selection}
					onChange={setSelection}
					disabled={saving}
				/>
				<Text size="xs" c="dimmed">
					For speaker labels, select GPT-4o Transcribe · speaker labels. Managed
					models must be enabled by Kolboo; other cloud models require your key
					in Settings → Providers.
				</Text>
				{!options.size && (
					<Text size="sm">
						No models are available. Configure a provider in Settings, or
						refresh your managed access.
					</Text>
				)}
				<Group justify="flex-end">
					<Button variant="subtle" onClick={onClose}>
						Cancel
					</Button>
					<Button
						disabled={!chosen}
						loading={saving}
						onClick={() => {
							if (chosen)
								onSave({
									...preferences,
									meeting_model: {
										provider: chosen.provider,
										model: chosen.model,
										use_managed: chosen.use_managed,
									},
								});
						}}
					>
						Save
					</Button>
				</Group>
			</Stack>
		</Modal>
	);
}
