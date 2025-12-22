import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import {
  type AppSettings,
  type CleanupPromptSections,
  configAPI,
  type HotkeyConfig,
  llmAPI,
  logsAPI,
  type OutputMode,
  type PlayingAudioHandling,
  type RewriteProgramPromptProfile,
  sttAPI,
  tauriAPI,
  type TestLlmRewriteResponse,
  validateHotkeyNotDuplicate,
  type WidgetPosition,
} from "./tauri";

export function useTypeText() {
  return useMutation({
    mutationFn: (text: string) => invoke("type_text", { text }),
  });
}

export function useTestLlmRewrite() {
  return useMutation({
    mutationFn: (params: {
      transcript: string;
      profileId?: string | null;
    }): Promise<TestLlmRewriteResponse> => llmAPI.testLlmRewrite(params),
  });
}

export function useTestSttTranscribeLastAudio() {
  return useMutation({
    mutationFn: (params: { profileId?: string | null }): Promise<string> =>
      sttAPI.testTranscribeLastAudio(params),
  });
}

export function useHasLastAudioForSttTest() {
  return useQuery({
    queryKey: ["sttLastAudioAvailable"],
    queryFn: () => sttAPI.hasLastAudio(),
    staleTime: 0,
    refetchOnWindowFocus: true,
    // Very cheap boolean check; polling keeps the UI in sync when the user
    // records audio via hotkey while the settings page is open.
    refetchInterval: 2000,
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
        "toggle"
      );
      if (error) throw new Error(error);

      // Save and re-register
      await tauriAPI.updateToggleHotkey(hotkey);
      await tauriAPI.unregisterShortcuts();
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
        "hold"
      );
      if (error) throw new Error(error);

      // Save and re-register
      await tauriAPI.updateHoldHotkey(hotkey);
      await tauriAPI.unregisterShortcuts();
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
        "paste_last"
      );
      if (error) throw new Error(error);

      // Save and re-register
      await tauriAPI.updatePasteLastHotkey(hotkey);
      await tauriAPI.unregisterShortcuts();
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

export function useUpdateRewriteLlmEnabled() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (enabled: boolean) => {
      await tauriAPI.updateRewriteLlmEnabled(enabled);
      // Gate the pipeline's LLM rewrite step immediately.
      await configAPI.syncPipelineConfig();
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
  });
}

export function useUpdatePlayingAudioHandling() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (handling: PlayingAudioHandling) =>
      tauriAPI.updatePlayingAudioHandling(handling),
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

export function useUpdateWidgetPosition() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (position: WidgetPosition) =>
      tauriAPI.updateWidgetPosition(position),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
  });
}

export function useUpdateOutputMode() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (mode: OutputMode) => tauriAPI.updateOutputMode(mode),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
  });
}

export function useUpdateOutputHitEnter() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (enabled: boolean) => tauriAPI.updateOutputHitEnter(enabled),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
  });
}

export function useUpdateQuietAudioGateEnabled() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (enabled: boolean) => {
      await tauriAPI.updateQuietAudioGateEnabled(enabled);
      await configAPI.syncPipelineConfig();
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
  });
}

export function useUpdateQuietAudioMinDurationSecs() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (seconds: number) => {
      await tauriAPI.updateQuietAudioMinDurationSecs(seconds);
      await configAPI.syncPipelineConfig();
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
  });
}

export function useUpdateQuietAudioRmsDbfsThreshold() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (dbfs: number) => {
      await tauriAPI.updateQuietAudioRmsDbfsThreshold(dbfs);
      await configAPI.syncPipelineConfig();
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
  });
}

export function useUpdateQuietAudioPeakDbfsThreshold() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (dbfs: number) => {
      await tauriAPI.updateQuietAudioPeakDbfsThreshold(dbfs);
      await configAPI.syncPipelineConfig();
    },
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
    mutationFn: async (sections: CleanupPromptSections | null) => {
      await tauriAPI.updateCleanupPromptSections(sections);
      // Apply prompt changes immediately (LLM step reads prompts from pipeline config).
      await configAPI.syncPipelineConfig();
    },
    onMutate: async (sections: CleanupPromptSections | null) => {
      // Optimistically update the settings cache so the UI doesn't snap back
      // if the user navigates away before the mutation settles.
      await queryClient.cancelQueries({ queryKey: ["settings"] });

      const previousSettings = queryClient.getQueryData<AppSettings>([
        "settings",
      ]);

      if (previousSettings) {
        queryClient.setQueryData<AppSettings>(["settings"], {
          ...previousSettings,
          cleanup_prompt_sections: sections,
        });
      }

      return { previousSettings };
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
    onError: (error, _sections, context) => {
      console.error("updateCleanupPromptSections failed:", error);

      // Roll back optimistic update.
      if (context?.previousSettings) {
        queryClient.setQueryData<AppSettings>(
          ["settings"],
          context.previousSettings
        );
      }
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
  });
}

export function useUpdateRewriteProgramPromptProfiles() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (profiles: RewriteProgramPromptProfile[]) => {
      await tauriAPI.updateRewriteProgramPromptProfiles(profiles);
      await configAPI.syncPipelineConfig();
    },
    onMutate: async (nextProfiles: RewriteProgramPromptProfile[]) => {
      // Optimistically update the settings cache so toggles/selects don't
      // "snap back" while the write + pipeline sync is running.
      await queryClient.cancelQueries({ queryKey: ["settings"] });

      const previousSettings = queryClient.getQueryData<AppSettings>([
        "settings",
      ]);

      if (previousSettings) {
        queryClient.setQueryData<AppSettings>(["settings"], {
          ...previousSettings,
          rewrite_program_prompt_profiles: nextProfiles,
        });
      }

      return { previousSettings };
    },
    onError: (_error, _nextProfiles, context) => {
      console.error("updateRewriteProgramPromptProfiles failed:", _error);
      // Roll back optimistic update.
      if (context?.previousSettings) {
        queryClient.setQueryData<AppSettings>(
          ["settings"],
          context.previousSettings
        );
      }
    },
    onSettled: () => {
      // Ensure we reconcile with persisted settings.
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
  });
}

export function useResetHotkeysToDefaults() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      await tauriAPI.resetHotkeysToDefaults();
      await tauriAPI.unregisterShortcuts();
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

export function useUpdateSTTTranscriptionPrompt() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (prompt: string | null) => {
      await tauriAPI.updateSTTTranscriptionPrompt(prompt);
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
    mutationFn: async (provider: string | null) => {
      await tauriAPI.updateLLMProvider(provider);
      // Sync the pipeline configuration when LLM provider changes
      await configAPI.syncPipelineConfig();
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
  });
}

export function useUpdateLLMModel() {
	const queryClient = useQueryClient();
	return useMutation({
    mutationFn: async (model: string | null) => {
      await tauriAPI.updateLLMModel(model);
      // Sync the pipeline configuration when LLM model changes
      await configAPI.syncPipelineConfig();
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["settings"] });
    },
  });
}

// STT Timeout mutation (local settings)
export function useUpdateSTTTimeout() {
	const queryClient = useQueryClient();
	return useMutation({
    mutationFn: async (timeoutSeconds: number | null) => {
      await tauriAPI.updateSTTTimeout(timeoutSeconds);
      await configAPI.syncPipelineConfig();
    },
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

// Retry a previous transcription attempt by request id (loads saved audio in backend).
export function useRetryTranscription() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (requestId: string) => sttAPI.retryTranscription({ requestId }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["history"] });
      queryClient.invalidateQueries({ queryKey: ["requestLogs"] });
    },
  });
}
