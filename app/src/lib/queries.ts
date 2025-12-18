import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import {
	type CleanupPromptSections,
	configAPI,
	type HotkeyConfig,
	logsAPI,
	tauriAPI,
	validateHotkeyNotDuplicate,
} from "./tauri";

export function useTypeText() {
	return useMutation({
		mutationFn: (text: string) => invoke("type_text", { text }),
	});
}

// Settings queries and mutations
export function useSettings() {
	return useQuery({
		queryKey: ["settings"],
		queryFn: () => tauriAPI.getSettings(),
		staleTime: Number.POSITIVE_INFINITY,
	});
}

export function useUpdateToggleHotkey() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: async (hotkey: HotkeyConfig) => {
			// Get current settings for validation
			const settings = await tauriAPI.getSettings();

			// Validate no duplicate
			const error = validateHotkeyNotDuplicate(
				hotkey,
				{
					toggle: settings.toggle_hotkey,
					hold: settings.hold_hotkey,
					paste_last: settings.paste_last_hotkey,
				},
				"toggle",
			);
			if (error) throw new Error(error);

			// Save and re-register
			await tauriAPI.updateToggleHotkey(hotkey);
			await tauriAPI.registerShortcuts();
		},
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["settings"] });
		},
	});
}

export function useUpdateHoldHotkey() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: async (hotkey: HotkeyConfig) => {
			// Get current settings for validation
			const settings = await tauriAPI.getSettings();

			// Validate no duplicate
			const error = validateHotkeyNotDuplicate(
				hotkey,
				{
					toggle: settings.toggle_hotkey,
					hold: settings.hold_hotkey,
					paste_last: settings.paste_last_hotkey,
				},
				"hold",
			);
			if (error) throw new Error(error);

			// Save and re-register
			await tauriAPI.updateHoldHotkey(hotkey);
			await tauriAPI.registerShortcuts();
		},
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["settings"] });
		},
	});
}

export function useUpdatePasteLastHotkey() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: async (hotkey: HotkeyConfig) => {
			// Get current settings for validation
			const settings = await tauriAPI.getSettings();

			// Validate no duplicate
			const error = validateHotkeyNotDuplicate(
				hotkey,
				{
					toggle: settings.toggle_hotkey,
					hold: settings.hold_hotkey,
					paste_last: settings.paste_last_hotkey,
				},
				"paste_last",
			);
			if (error) throw new Error(error);

			// Save and re-register
			await tauriAPI.updatePasteLastHotkey(hotkey);
			await tauriAPI.registerShortcuts();
		},
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["settings"] });
		},
	});
}

export function useUpdateSelectedMic() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: (micId: string | null) => tauriAPI.updateSelectedMic(micId),
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["settings"] });
		},
	});
}

export function useUpdateSoundEnabled() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: (enabled: boolean) => tauriAPI.updateSoundEnabled(enabled),
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["settings"] });
		},
	});
}

export function useUpdateAutoMuteAudio() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: (enabled: boolean) => tauriAPI.updateAutoMuteAudio(enabled),
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["settings"] });
		},
	});
}

export function useUpdateOverlayMode() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: (mode: "always" | "never" | "recording_only") =>
			tauriAPI.updateOverlayMode(mode),
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["settings"] });
		},
	});
}

export function useIsAudioMuteSupported() {
	return useQuery({
		queryKey: ["audioMuteSupported"],
		queryFn: () => tauriAPI.isAudioMuteSupported(),
		staleTime: Number.POSITIVE_INFINITY,
	});
}

export function useUpdateCleanupPromptSections() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: (sections: CleanupPromptSections | null) =>
			tauriAPI.updateCleanupPromptSections(sections),
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["settings"] });
		},
	});
}

export function useResetHotkeysToDefaults() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: async () => {
			await tauriAPI.resetHotkeysToDefaults();
			await tauriAPI.registerShortcuts();
		},
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["settings"] });
		},
		onError: (error) => {
			console.error("Reset hotkeys failed:", error);
		},
	});
}

// History queries and mutations
export function useHistory(limit?: number) {
	return useQuery({
		queryKey: ["history", limit],
		queryFn: () => tauriAPI.getHistory(limit),
	});
}

export function useAddHistoryEntry() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: (text: string) => tauriAPI.addHistoryEntry(text),
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["history"] });
			// Notify other windows about history change
			tauriAPI.emitHistoryChanged();
		},
	});
}

export function useDeleteHistoryEntry() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: (id: string) => tauriAPI.deleteHistoryEntry(id),
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["history"] });
			// Notify other windows about history change
			tauriAPI.emitHistoryChanged();
		},
	});
}

export function useClearHistory() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: () => tauriAPI.clearHistory(),
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["history"] });
			// Notify other windows about history change
			tauriAPI.emitHistoryChanged();
		},
	});
}

// Config API queries and mutations (now using Tauri commands)
export function useDefaultSections() {
	return useQuery({
		queryKey: ["defaultSections"],
		queryFn: () => configAPI.getDefaultSections(),
		staleTime: Number.POSITIVE_INFINITY, // Default prompts never change
	});
}

// Provider queries and mutations

export function useAvailableProviders() {
	return useQuery({
		queryKey: ["availableProviders"],
		queryFn: () => configAPI.getAvailableProviders(),
	});
}

export function useUpdateSTTProvider() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: async (provider: string | null) => {
			await tauriAPI.updateSTTProvider(provider);
			// Sync the pipeline configuration when STT provider changes
			await configAPI.syncPipelineConfig();
		},
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["settings"] });
		},
	});
}

export function useUpdateSTTModel() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: async (model: string | null) => {
			await tauriAPI.updateSTTModel(model);
			// Sync the pipeline configuration when STT model changes
			await configAPI.syncPipelineConfig();
		},
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["settings"] });
		},
	});
}

export function useUpdateLLMProvider() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: (provider: string | null) =>
			tauriAPI.updateLLMProvider(provider),
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["settings"] });
		},
	});
}

export function useUpdateLLMModel() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: (model: string | null) => tauriAPI.updateLLMModel(model),
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["settings"] });
		},
	});
}

// STT Timeout mutation (local settings)
export function useUpdateSTTTimeout() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: (timeoutSeconds: number | null) =>
			tauriAPI.updateSTTTimeout(timeoutSeconds),
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["settings"] });
		},
	});
}

// Request Logs queries and mutations
export function useRequestLogs(limit?: number) {
	return useQuery({
		queryKey: ["requestLogs", limit],
		queryFn: () => logsAPI.getRequestLogs(limit),
		refetchInterval: 2000, // Refresh every 2 seconds to show live updates
	});
}

export function useClearRequestLogs() {
	const queryClient = useQueryClient();
	return useMutation({
		mutationFn: () => logsAPI.clearRequestLogs(),
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: ["requestLogs"] });
		},
	});
}
