import {
  Accordion,
  ActionIcon,
  Button,
  Divider,
  Group,
  Loader,
  Modal,
  NumberInput,
  Select,
  Switch,
  Textarea,
  Text,
  Tooltip,
} from "@mantine/core";
import { Info, RotateCcw } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  useDefaultSections,
  useSettings,
  useAvailableProviders,
  useTestLlmRewrite,
  useTestSttTranscribeLastAudio,
  useHasLastAudioForSttTest,
  useUpdateCleanupPromptSections,
  useUpdateLLMModel,
  useUpdateLLMProvider,
  useUpdateRewriteProgramPromptProfiles,
  useUpdateRewriteLlmEnabled,
  useUpdateSTTModel,
  useUpdateSTTProvider,
  useUpdateSTTTranscriptionPrompt,
  useUpdateSTTTimeout,
} from "../../lib/queries";
import {
  type CleanupPromptSections,
  type CleanupPromptSectionsOverride,
  type RewriteProgramPromptProfile,
  tauriAPI,
} from "../../lib/tauri";
import { PromptSectionEditor } from "./PromptSectionEditor";

const INHERIT_TOOLTIP = "Inheriting from Default profile";

// (debug logging removed)

const DEFAULT_SECTIONS: CleanupPromptSections = {
  main: { enabled: true, content: null },
  advanced: { enabled: true, content: null },
  dictionary: { enabled: false, content: null },
};

// NOTE: This timeout is used by the Rust pipeline as a transcription request timeout.
// Keep this default aligned with backend fallbacks so "unset" settings don't lie.
const DEFAULT_STT_TIMEOUT = 60;

// Model options for each STT provider
const STT_MODELS: Record<string, { value: string; label: string }[]> = {
  groq: [
    { value: "whisper-large-v3", label: "Whisper Large V3" },
    { value: "whisper-large-v3-turbo", label: "Whisper Large V3 Turbo" },
  ],
  openai: [
    // { value: "gpt-audio", label: "GPT Audio" },
    // { value: "gpt-audio-mini", label: "GPT Audio Mini" },
    // { value: "gpt-4o-audio-preview", label: "GPT-4o Audio Preview" },
    // { value: "gpt-4o-mini-audio-preview", label: "GPT-4o Mini Audio Preview" },
    { value: "gpt-4o-transcribe", label: "GPT-4o Transcribe" },
    { value: "gpt-4o-mini-transcribe", label: "GPT-4o Mini Transcribe" },
    { value: "whisper-1", label: "Whisper-1" },
  ],
  deepgram: [
    { value: "nova-2", label: "Nova 2" },
    { value: "nova", label: "Nova" },
    { value: "enhanced", label: "Enhanced" },
    { value: "base", label: "Base" },
  ],
  whisper: [],
};

// Model options for each LLM provider
const LLM_MODELS: Record<string, { value: string; label: string }[]> = {
  groq: [
    { value: "llama-3.3-70b-versatile", label: "Llama 3.3 70B Versatile" },
    { value: "llama-3.1-8b-instant", label: "Llama 3.1 8B Instant" },
    { value: "mixtral-8x7b-32768", label: "Mixtral 8x7B" },
  ],
  openai: [
    { value: "gpt-4.1", label: "GPT-4.1" },
    { value: "gpt-4.1-mini", label: "GPT-4.1 Mini" },
    { value: "gpt-4.1-nano", label: "GPT-4.1 Nano" },
    // { value: "gpt-4o-mini", label: "GPT-4o Mini" },
    // { value: "gpt-4o", label: "GPT-4o" },
    // { value: "gpt-4-turbo", label: "GPT-4 Turbo" },
  ],
  anthropic: [
    { value: "claude-3-5-haiku-latest", label: "Claude 3.5 Haiku" },
    { value: "claude-3-5-sonnet-latest", label: "Claude 3.5 Sonnet" },
    { value: "claude-3-opus-latest", label: "Claude 3 Opus" },
  ],
  ollama: [],
};

type SectionKey = "main" | "advanced" | "dictionary";

function errorToMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  if (err && typeof err === "object") {
    const anyErr = err as any;
    if (typeof anyErr.message === "string") return anyErr.message;
    if (typeof anyErr.error === "string") return anyErr.error;
    try {
      return JSON.stringify(err);
    } catch {
      return String(err);
    }
  }
  return String(err);
}

interface LocalSectionState {
  enabled: boolean;
  content: string;
}

interface LocalSections {
  main: LocalSectionState;
  advanced: LocalSectionState;
  dictionary: LocalSectionState;
}

function createId(): string {
  // `crypto.randomUUID()` is available in modern browsers; keep a fallback for safety.
  // This only needs to be unique enough for local settings.
  return (
    globalThis.crypto?.randomUUID?.() ??
    `id_${Date.now()}_${Math.random().toString(16).slice(2)}`
  );
}

export function PromptSettings({
  editingProfileId,
}: {
  editingProfileId?: string;
}) {
  const { data: settings, isLoading: isLoadingSettings } = useSettings();
  const { data: defaultSections, isLoading: isLoadingDefaultSections } =
    useDefaultSections();
  const { data: availableProviders, isLoading: isLoadingProviders } =
    useAvailableProviders();
  const updateCleanupPromptSections = useUpdateCleanupPromptSections();
  const updateRewriteLlmEnabled = useUpdateRewriteLlmEnabled();
  const updateRewriteProgramPromptProfiles =
    useUpdateRewriteProgramPromptProfiles();
  const testLlmRewrite = useTestLlmRewrite();
  const testSttLastAudio = useTestSttTranscribeLastAudio();
  const { data: hasLastAudioForSttTest } = useHasLastAudioForSttTest();

  // Default profile (global) provider settings
  const updateSTTProvider = useUpdateSTTProvider();
  const updateSTTModel = useUpdateSTTModel();
  const updateSTTTranscriptionPrompt = useUpdateSTTTranscriptionPrompt();
  const updateLLMProvider = useUpdateLLMProvider();
  const updateLLMModel = useUpdateLLMModel();
  const updateSTTTimeout = useUpdateSTTTimeout();

  const activeProfileId = editingProfileId ?? "default";

  const profiles: RewriteProgramPromptProfile[] =
    settings?.rewrite_program_prompt_profiles ?? [];

  const activeProfile =
    activeProfileId === "default"
      ? null
      : profiles.find((p) => p.id === activeProfileId) ?? null;

  const defaultRewriteEnabled = settings?.rewrite_llm_enabled ?? false;

  const [localProfileSttProvider, setLocalProfileSttProvider] = useState<
    string | null
  >(null);
  const [localProfileSttModel, setLocalProfileSttModel] = useState<
    string | null
  >(null);
  const [localProfileLlmProvider, setLocalProfileLlmProvider] = useState<
    string | null
  >(null);
  const [localProfileLlmModel, setLocalProfileLlmModel] = useState<
    string | null
  >(null);
  const [localProfileRewriteEnabled, setLocalProfileRewriteEnabled] =
    useState<boolean>(false);
  const [localProfileSttTimeout, setLocalProfileSttTimeout] = useState<
    string | number
  >(DEFAULT_STT_TIMEOUT);

  const [rewriteTestInput, setRewriteTestInput] = useState<string>(
    "um hello there uh how are you"
  );
  const [rewriteTestOutput, setRewriteTestOutput] = useState<string>("");
  const [rewriteTestError, setRewriteTestError] = useState<string>("");
  const [rewriteTestDurationMs, setRewriteTestDurationMs] = useState<
    number | null
  >(null);
  const rewriteTestStartRef = useRef<number | null>(null);

  const [sttTestOutput, setSttTestOutput] = useState<string>("");
  const [sttTestError, setSttTestError] = useState<string>("");
  const [sttTestDurationMs, setSttTestDurationMs] = useState<number | null>(
    null
  );
  const [localSttTranscriptionPrompt, setLocalSttTranscriptionPrompt] =
    useState<string>("");
  const sttTestStartRef = useRef<number | null>(null);

  const [resetDialog, setResetDialog] = useState<null | {
    title: string;
    onConfirm: () => void;
  }>(null);

  const openDisableOverrideDialog = (args: {
    title: string;
    onConfirm: () => void;
  }) => {
    setResetDialog(args);
  };

  // Track whether profile settings are inheriting (original value was null)
  const [sttProviderInheriting, setSttProviderInheriting] = useState(false);
  const [sttModelInheriting, setSttModelInheriting] = useState(false);
  const [sttTimeoutInheriting, setSttTimeoutInheriting] = useState(false);
  const [llmProviderInheriting, setLlmProviderInheriting] = useState(false);
  const [llmModelInheriting, setLlmModelInheriting] = useState(false);
  const [rewriteEnabledInheriting, setRewriteEnabledInheriting] =
    useState(false);

  // NOTE: Settings tabs unmount when switching (keepMounted=false). If we render
  // switches immediately, they first render with placeholder values then “jump”
  // once settings load. We avoid that by showing a loader until we have settings
  // + defaults and local state is initialized.
  const [localSections, setLocalSections] = useState<LocalSections | null>(
    null
  );

  // When updating per-section prompt overrides quickly (e.g., toggling Advanced then
  // Dictionary immediately), relying on `activeProfile` can drop earlier changes
  // because the component may not have re-rendered with the optimistic update yet.
  // Keep a ref of the latest per-profile prompt overrides to merge safely.
  const profilePromptOverridesRef =
    useRef<CleanupPromptSectionsOverride | null>(null);

  useEffect(() => {
    profilePromptOverridesRef.current =
      activeProfile?.cleanup_prompt_sections ?? null;
  }, [activeProfileId, activeProfile?.cleanup_prompt_sections]);

  useEffect(() => {
    if (settings !== undefined && defaultSections !== undefined) {
      const base = settings.cleanup_prompt_sections ?? DEFAULT_SECTIONS;

      const profileOverrides: CleanupPromptSectionsOverride | null | undefined =
        activeProfileId === "default"
          ? null
          : profiles.find((p) => p.id === activeProfileId)
              ?.cleanup_prompt_sections;

      const resolved: CleanupPromptSections =
        activeProfileId === "default"
          ? base
          : {
              main: profileOverrides?.main ?? base.main,
              advanced: profileOverrides?.advanced ?? base.advanced,
              dictionary: profileOverrides?.dictionary ?? base.dictionary,
            };

      setLocalSections({
        main: {
          enabled: resolved.main.enabled,
          content: resolved.main.content ?? defaultSections.main,
        },
        advanced: {
          enabled: resolved.advanced.enabled,
          content: resolved.advanced.content ?? defaultSections.advanced,
        },
        dictionary: {
          enabled: resolved.dictionary.enabled,
          content: resolved.dictionary.content ?? defaultSections.dictionary,
        },
      });
    }
  }, [settings, defaultSections, activeProfileId, profiles]);

  useEffect(() => {
    if (activeProfile) {
      // Track whether each setting is inheriting (null in the profile)
      const sttProviderIsNull =
        activeProfile.stt_provider === null ||
        activeProfile.stt_provider === undefined;
      const sttModelIsNull =
        activeProfile.stt_model === null ||
        activeProfile.stt_model === undefined;
      const sttTimeoutIsNull =
        activeProfile.stt_timeout_seconds === null ||
        activeProfile.stt_timeout_seconds === undefined;
      const llmProviderIsNull =
        activeProfile.llm_provider === null ||
        activeProfile.llm_provider === undefined;
      const llmModelIsNull =
        activeProfile.llm_model === null ||
        activeProfile.llm_model === undefined;
      const rewriteEnabledIsNull =
        activeProfile.rewrite_llm_enabled === null ||
        activeProfile.rewrite_llm_enabled === undefined;

      setSttProviderInheriting(sttProviderIsNull);
      setSttModelInheriting(sttModelIsNull);
      setSttTimeoutInheriting(sttTimeoutIsNull);
      setLlmProviderInheriting(llmProviderIsNull);
      setLlmModelInheriting(llmModelIsNull);
      setRewriteEnabledInheriting(rewriteEnabledIsNull);

      // Set local state (falling back to global defaults for display)
      setLocalProfileSttProvider(
        activeProfile.stt_provider ?? settings?.stt_provider ?? null
      );
      setLocalProfileSttModel(
        activeProfile.stt_model ?? settings?.stt_model ?? null
      );
      setLocalProfileLlmProvider(
        activeProfile.llm_provider ?? settings?.llm_provider ?? null
      );
      setLocalProfileLlmModel(
        activeProfile.llm_model ?? settings?.llm_model ?? null
      );
      setLocalProfileRewriteEnabled(
        activeProfile.rewrite_llm_enabled ?? defaultRewriteEnabled
      );
      setLocalProfileSttTimeout(
        activeProfile.stt_timeout_seconds ??
          settings?.stt_timeout_seconds ??
          DEFAULT_STT_TIMEOUT
      );
    } else {
      // Default scope - not inheriting
      setSttProviderInheriting(false);
      setSttModelInheriting(false);
      setSttTimeoutInheriting(false);
      setLlmProviderInheriting(false);
      setLlmModelInheriting(false);
      setRewriteEnabledInheriting(false);

      setLocalProfileSttProvider(null);
      setLocalProfileSttModel(null);
      setLocalProfileLlmProvider(null);
      setLocalProfileLlmModel(null);
      setLocalProfileRewriteEnabled(defaultRewriteEnabled);
      setLocalProfileSttTimeout(
        settings?.stt_timeout_seconds ?? DEFAULT_STT_TIMEOUT
      );
    }
  }, [
    activeProfileId,
    activeProfile,
    settings?.stt_timeout_seconds,
    settings?.stt_provider,
    settings?.stt_model,
    settings?.llm_provider,
    settings?.llm_model,
    defaultRewriteEnabled,
  ]);

  const isLoading =
    isLoadingSettings ||
    isLoadingDefaultSections ||
    isLoadingProviders ||
    settings === undefined ||
    defaultSections === undefined ||
    localSections === null;

  // One-time migration: ensure every profile has its own rewrite enable flag.
  // This prevents the Default toggle from affecting other profiles.
  const didMigrateProfileRewriteEnabled = useRef(false);
  useEffect(() => {
    if (didMigrateProfileRewriteEnabled.current) return;
    if (!settings) return;
    if (profiles.length === 0) return;

    const needsMigration = profiles.some(
      (p) => typeof p.rewrite_llm_enabled !== "boolean"
    );
    if (!needsMigration) {
      didMigrateProfileRewriteEnabled.current = true;
      return;
    }

    didMigrateProfileRewriteEnabled.current = true;

    const migrated = profiles.map((p) => {
      const current = p.rewrite_llm_enabled;
      if (typeof current === "boolean") return p;
      return { ...p, rewrite_llm_enabled: defaultRewriteEnabled };
    });

    updateRewriteProgramPromptProfiles.mutate(migrated, {
      onSuccess: () => {
        tauriAPI.emitSettingsChanged();
      },
    });
  }, [
    settings,
    profiles,
    defaultRewriteEnabled,
    updateRewriteProgramPromptProfiles,
  ]);

  // Provider dropdown options
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

  const effectiveSttProvider =
    activeProfileId === "default"
      ? settings?.stt_provider ?? null
      : localProfileSttProvider ?? settings?.stt_provider ?? null;
  const effectiveSttModel =
    activeProfileId === "default"
      ? settings?.stt_model ?? null
      : localProfileSttModel ?? settings?.stt_model ?? null;
  const effectiveLlmProvider =
    activeProfileId === "default"
      ? settings?.llm_provider ?? null
      : localProfileLlmProvider ?? settings?.llm_provider ?? null;

  const isOpenAiStt = effectiveSttProvider === "openai";
  const isGroqStt = effectiveSttProvider === "groq";
  const isWhisper1Selected = isOpenAiStt && effectiveSttModel === "whisper-1";
  const isGroqWhisperModel =
    isGroqStt &&
    (effectiveSttModel === null ||
      Boolean(effectiveSttModel?.includes("whisper")));

  const promptMaxChars = 224;
  const isPrompt224CharLimited = isWhisper1Selected || isGroqWhisperModel;

  const sttPromptSupported =
    (isOpenAiStt &&
      (effectiveSttModel === "whisper-1" ||
        (Boolean(effectiveSttModel?.includes("transcribe")) &&
          !effectiveSttModel?.includes("diarize")))) ||
    isGroqWhisperModel;

  const sttPromptDisabledReason = useMemo(() => {
    if (!effectiveSttProvider) {
      return "Select an STT provider to enable transcription prompting.";
    }

    if (effectiveSttProvider === "openai") {
      const modelLabel = effectiveSttModel ?? "default";
      return `The selected OpenAI model (${modelLabel}) does not support transcription prompting.`;
    }

    if (effectiveSttProvider === "groq") {
      const modelLabel = effectiveSttModel ?? "default";
      return `The selected Groq model (${modelLabel}) does not support transcription prompting.`;
    }

    return "Transcription prompt is supported for OpenAI transcription models and Groq Whisper models.";
  }, [effectiveSttProvider, effectiveSttModel]);

  // Keep the local UI state in sync with persisted settings.
  useEffect(() => {
    setLocalSttTranscriptionPrompt(settings?.stt_transcription_prompt ?? "");
  }, [settings?.stt_transcription_prompt]);

  // Debounced save (global setting). We only allow editing/saving when supported.
  useEffect(() => {
    if (!sttPromptSupported) return;

    const normalized = localSttTranscriptionPrompt.trim();
    const toStore: string | null = normalized.length > 0 ? normalized : null;
    const storedNormalized: string | null =
      settings?.stt_transcription_prompt?.trim() || null;

    if (toStore === storedNormalized) return;

    const handle = window.setTimeout(() => {
      updateSTTTranscriptionPrompt.mutate(toStore, {
        onSuccess: () => {
          tauriAPI.emitSettingsChanged();
        },
      });
    }, 500);

    return () => {
      window.clearTimeout(handle);
    };
  }, [
    localSttTranscriptionPrompt,
    settings?.stt_transcription_prompt,
    sttPromptSupported,
    updateSTTTranscriptionPrompt,
  ]);

  const sttModelOptions = effectiveSttProvider
    ? STT_MODELS[effectiveSttProvider] ?? []
    : [];
  const llmModelOptions = effectiveLlmProvider
    ? LLM_MODELS[effectiveLlmProvider] ?? []
    : [];

  const baseStoredSections: CleanupPromptSections =
    settings?.cleanup_prompt_sections ?? DEFAULT_SECTIONS;

  const storedSectionsResolved: CleanupPromptSections =
    activeProfileId === "default" || !activeProfile
      ? baseStoredSections
      : {
          main:
            activeProfile.cleanup_prompt_sections?.main ??
            baseStoredSections.main,
          advanced:
            activeProfile.cleanup_prompt_sections?.advanced ??
            baseStoredSections.advanced,
          dictionary:
            activeProfile.cleanup_prompt_sections?.dictionary ??
            baseStoredSections.dictionary,
        };

  const hasCustomContent = {
    main: Boolean(storedSectionsResolved.main.content),
    advanced: Boolean(storedSectionsResolved.advanced.content),
    dictionary: Boolean(storedSectionsResolved.dictionary.content),
  };

  const buildSections = (overrides?: {
    key: SectionKey;
    enabled?: boolean;
    content?: string | null;
  }): CleanupPromptSections => {
    if (localSections === null) {
      return DEFAULT_SECTIONS;
    }

    const getContent = (key: SectionKey): string | null => {
      const content =
        overrides?.key === key && overrides.content !== undefined
          ? overrides.content
          : localSections[key].content;

      // Return null if content matches default (to use server default)
      if (content === defaultSections?.[key]) {
        return null;
      }
      return content || null;
    };

    const getEnabled = (key: SectionKey): boolean => {
      return overrides?.key === key && overrides.enabled !== undefined
        ? overrides.enabled
        : localSections[key].enabled;
    };

    return {
      main: { enabled: getEnabled("main"), content: getContent("main") },
      advanced: {
        enabled: getEnabled("advanced"),
        content: getContent("advanced"),
      },
      dictionary: {
        enabled: getEnabled("dictionary"),
        content: getContent("dictionary"),
      },
    };
  };

  const saveAllSections = (sections: CleanupPromptSections) => {
    // Only used for Default scope. Per-profile prompt changes are stored as per-section overrides.
    updateCleanupPromptSections.mutate(sections, {
      onSuccess: () => {
        tauriAPI.emitSettingsChanged();
      },
    });
  };

  const normalizePromptOverrides = (
    overrides: CleanupPromptSectionsOverride
  ): CleanupPromptSectionsOverride | null => {
    const hasAny =
      overrides.main != null ||
      overrides.advanced != null ||
      overrides.dictionary != null;
    return hasAny ? overrides : null;
  };

  const saveProfileSectionOverride = (
    key: SectionKey,
    section: CleanupPromptSections[SectionKey] | null
  ) => {
    const current: CleanupPromptSectionsOverride =
      profilePromptOverridesRef.current ?? {};
    const next: CleanupPromptSectionsOverride = { ...current, [key]: section };
    const normalized = normalizePromptOverrides(next);
    profilePromptOverridesRef.current = normalized;

    saveProfileMetadata({ cleanup_prompt_sections: normalized });
  };

  const buildSectionOverride = (
    key: SectionKey,
    overrides?: {
      enabled?: boolean;
      content?: string | null;
    }
  ): CleanupPromptSections[SectionKey] => {
    if (localSections === null) {
      return DEFAULT_SECTIONS[key];
    }

    const contentRaw =
      overrides?.content !== undefined
        ? overrides.content
        : localSections[key].content;

    const contentToStore =
      contentRaw === defaultSections?.[key] ? null : contentRaw || null;

    const enabledToStore =
      overrides?.enabled !== undefined
        ? overrides.enabled
        : localSections[key].enabled;

    return {
      enabled: enabledToStore,
      content: contentToStore,
    };
  };

  const saveProfileMetadata = (next: Partial<RewriteProgramPromptProfile>) => {
    if (activeProfileId === "default") return;

    const exists = profiles.some((p) => p.id === activeProfileId);
    if (!exists) {
      return;
    }

    const updated = profiles.map((p) =>
      p.id === activeProfileId ? { ...p, ...next } : p
    );

    updateRewriteProgramPromptProfiles.mutate(updated, {
      onSuccess: () => {
        tauriAPI.emitSettingsChanged();
      },
    });
  };

  const handleToggle = (key: SectionKey, checked: boolean) => {
    setLocalSections((prev) => {
      if (prev === null) return prev;
      return { ...prev, [key]: { ...prev[key], enabled: checked } };
    });

    if (activeProfileId === "default") {
      saveAllSections(buildSections({ key, enabled: checked }));
      return;
    }

    saveProfileSectionOverride(
      key,
      buildSectionOverride(key, { enabled: checked })
    );
  };

  const handleSave = (key: SectionKey, content: string) => {
    setLocalSections((prev) => {
      if (prev === null) return prev;
      return { ...prev, [key]: { ...prev[key], content } };
    });

    if (activeProfileId === "default") {
      saveAllSections(buildSections({ key, content }));
      return;
    }

    saveProfileSectionOverride(key, buildSectionOverride(key, { content }));
  };

  const handleReset = (key: SectionKey) => {
    const defaultContent = defaultSections?.[key] ?? "";
    setLocalSections((prev) => {
      if (prev === null) return prev;
      return { ...prev, [key]: { ...prev[key], content: defaultContent } };
    });

    if (activeProfileId === "default") {
      saveAllSections(buildSections({ key, content: null }));
      return;
    }

    saveProfileSectionOverride(
      key,
      buildSectionOverride(key, { content: null })
    );
  };

  const handleDefaultSTTProviderChange = (value: string | null) => {
    if (!value) return;
    updateSTTProvider.mutate(value, {
      onSuccess: () => {
        const models = STT_MODELS[value];
        const firstModel = models?.[0];
        if (firstModel) {
          updateSTTModel.mutate(firstModel.value);
        }
        tauriAPI.emitSettingsChanged();
      },
    });
  };

  const handleDefaultSTTModelChange = (value: string | null) => {
    if (!value) return;
    updateSTTModel.mutate(value, {
      onSuccess: () => {
        tauriAPI.emitSettingsChanged();
      },
    });
  };

  const handleDefaultLLMProviderChange = (value: string | null) => {
    if (!value) return;
    updateLLMProvider.mutate(value, {
      onSuccess: () => {
        const models = LLM_MODELS[value];
        const firstModel = models?.[0];
        if (firstModel) {
          updateLLMModel.mutate(firstModel.value);
        }
        tauriAPI.emitSettingsChanged();
      },
    });
  };

  const handleDefaultLLMModelChange = (value: string | null) => {
    if (!value) return;
    updateLLMModel.mutate(value, {
      onSuccess: () => {
        tauriAPI.emitSettingsChanged();
      },
    });
  };

  const handleDefaultSTTTimeoutChange = (value: number) => {
    updateSTTTimeout.mutate(value, {
      onSuccess: () => {
        tauriAPI.emitSettingsChanged();
      },
    });
  };

  if (isLoading) {
    return (
      <div
        style={{
          display: "flex",
          justifyContent: "center",
          padding: "20px",
        }}
      >
        <Loader size="sm" color="gray" />
      </div>
    );
  }

  // If user selected a profile that no longer exists, fall back to Default.
  if (activeProfileId !== "default" && !activeProfile) {
    return (
      <div style={{ fontSize: 12, opacity: 0.75 }}>
        That profile no longer exists. Select another profile in the Editing
        dropdown.
      </div>
    );
  }

  const isDefaultScope = activeProfileId === "default";

  return (
    <>
      <Modal
        opened={resetDialog !== null}
        onClose={() => setResetDialog(null)}
        title={resetDialog?.title ?? ""}
        centered
      >
        <Text size="sm" c="dimmed" style={{ lineHeight: 1.4 }}>
          This setting is currently overriding the Default profile. Disable the
          override to inherit from Default.
        </Text>
        <Group justify="flex-end" mt="md" gap="sm">
          <Button variant="default" onClick={() => setResetDialog(null)}>
            Keep override
          </Button>
          <Button
            color="gray"
            onClick={() => {
              const confirm = resetDialog?.onConfirm;
              setResetDialog(null);
              confirm?.();
            }}
          >
            Disable override
          </Button>
        </Group>
      </Modal>

      <Divider
        my="md"
        label="Speech-to-text"
        labelPosition="left"
        styles={{
          root: {
            borderTopWidth: 2,
            borderColor: "var(--border-default)",
          },
          label: {
            color: "var(--text-primary)",
            fontSize: 11,
            fontWeight: 600,
            letterSpacing: "0.08em",
            textTransform: "uppercase",
          },
        }}
      />

      <div className="settings-row">
        <div>
          <p className="settings-label">Speech-to-Text Provider</p>
          <p className="settings-description">Service for transcribing audio</p>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          {!isDefaultScope && sttProviderInheriting && (
            <Tooltip label={INHERIT_TOOLTIP} withArrow>
              <Info size={14} style={{ opacity: 0.5, flexShrink: 0 }} />
            </Tooltip>
          )}
          {!isDefaultScope && !sttProviderInheriting && (
            <Tooltip label="Disable override (inherit from Default)" withArrow>
              <ActionIcon
                variant="subtle"
                color="gray"
                size="sm"
                onClick={() =>
                  openDisableOverrideDialog({
                    title: "Disable Speech-to-Text Provider override?",
                    onConfirm: () => {
                      setSttProviderInheriting(true);
                      setSttModelInheriting(true);
                      setLocalProfileSttProvider(
                        settings?.stt_provider ?? null
                      );
                      setLocalProfileSttModel(settings?.stt_model ?? null);
                      saveProfileMetadata({
                        stt_provider: null,
                        stt_model: null,
                      });
                    },
                  })
                }
              >
                <RotateCcw size={14} style={{ opacity: 0.65 }} />
              </ActionIcon>
            </Tooltip>
          )}
          <Select
            data={sttProviderOptions}
            value={
              isDefaultScope
                ? settings?.stt_provider ?? null
                : localProfileSttProvider
            }
            onChange={(value) => {
              if (!value) return;
              if (isDefaultScope) {
                handleDefaultSTTProviderChange(value);
                return;
              }

              setSttProviderInheriting(false);
              setSttModelInheriting(false);
              setLocalProfileSttProvider(value);
              const models = STT_MODELS[value] ?? [];
              const firstModel = models[0]?.value ?? null;
              setLocalProfileSttModel(firstModel);
              saveProfileMetadata({
                stt_provider: value,
                stt_model: firstModel,
              });
            }}
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
        </div>
      </div>

      {sttModelOptions.length > 0 ? (
        <div className="settings-row">
          <div>
            <p className="settings-label">STT Model</p>
            <p className="settings-description">
              Model to use for transcription
            </p>
          </div>
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            {!isDefaultScope && sttModelInheriting && (
              <Tooltip label={INHERIT_TOOLTIP} withArrow>
                <Info size={14} style={{ opacity: 0.5, flexShrink: 0 }} />
              </Tooltip>
            )}
            {!isDefaultScope && !sttModelInheriting && (
              <Tooltip
                label="Disable override (inherit from Default)"
                withArrow
              >
                <ActionIcon
                  variant="subtle"
                  color="gray"
                  size="sm"
                  onClick={() =>
                    openDisableOverrideDialog({
                      title: "Disable STT Model override?",
                      onConfirm: () => {
                        setSttModelInheriting(true);
                        setLocalProfileSttModel(settings?.stt_model ?? null);
                        saveProfileMetadata({ stt_model: null });
                      },
                    })
                  }
                >
                  <RotateCcw size={14} style={{ opacity: 0.65 }} />
                </ActionIcon>
              </Tooltip>
            )}
            <Select
              data={sttModelOptions}
              value={
                isDefaultScope
                  ? settings?.stt_model ?? sttModelOptions[0]?.value ?? null
                  : localProfileSttModel
              }
              onChange={(value) => {
                if (!value) return;
                if (isDefaultScope) {
                  handleDefaultSTTModelChange(value);
                  return;
                }
                setSttModelInheriting(false);
                setLocalProfileSttModel(value);
                saveProfileMetadata({ stt_model: value });
              }}
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
        </div>
      ) : null}

      <div className="settings-row no-divider">
        <div>
          <p className="settings-label">STT Timeout</p>
          <p className="settings-description">
            Increase if nothing is getting transcribed
          </p>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          {!isDefaultScope && sttTimeoutInheriting && (
            <Tooltip label={INHERIT_TOOLTIP} withArrow>
              <Info size={14} style={{ opacity: 0.5, flexShrink: 0 }} />
            </Tooltip>
          )}
          {!isDefaultScope && !sttTimeoutInheriting && (
            <Tooltip label="Disable override (inherit from Default)" withArrow>
              <ActionIcon
                variant="subtle"
                color="gray"
                size="sm"
                onClick={() =>
                  openDisableOverrideDialog({
                    title: "Disable STT Timeout override?",
                    onConfirm: () => {
                      setSttTimeoutInheriting(true);
                      setLocalProfileSttTimeout(
                        settings?.stt_timeout_seconds ?? DEFAULT_STT_TIMEOUT
                      );
                      saveProfileMetadata({ stt_timeout_seconds: null });
                    },
                  })
                }
              >
                <RotateCcw size={14} style={{ opacity: 0.65 }} />
              </ActionIcon>
            </Tooltip>
          )}

          <NumberInput
            value={localProfileSttTimeout}
            onChange={(value) => {
              setLocalProfileSttTimeout(value);

              if (typeof value !== "number" || Number.isNaN(value)) {
                return;
              }

              const clamped = Math.max(5, Math.min(120, value));
              if (clamped !== value) {
                setLocalProfileSttTimeout(clamped);
              }

              if (isDefaultScope) {
                handleDefaultSTTTimeoutChange(clamped);
                return;
              }

              setSttTimeoutInheriting(false);
              saveProfileMetadata({ stt_timeout_seconds: clamped });
            }}
            min={5}
            max={120}
            step={5}
            clampBehavior="strict"
            styles={{
              input: {
                backgroundColor: "var(--bg-elevated)",
                borderColor: "var(--border-default)",
                color: "var(--text-primary)",
                width: 120,
              },
            }}
            rightSection={
              <Text size="xs" c="dimmed">
                s
              </Text>
            }
            rightSectionPointerEvents="none"
          />
        </div>
      </div>

      <div style={{ marginTop: 16, marginBottom: 16 }}>
        <Accordion variant="separated" radius="md">
          <Accordion.Item value={`${activeProfileId}-stt-prompt`}>
            <Accordion.Control>
              <Tooltip
                label={sttPromptDisabledReason}
                withArrow
                disabled={sttPromptSupported}
              >
                <div style={{ opacity: sttPromptSupported ? 1 : 0.5 }}>
                  <p className="settings-label">Transcription prompt</p>
                  <p className="settings-description">
                    Optional context used during transcription.
                  </p>
                </div>
              </Tooltip>
            </Accordion.Control>
            <Accordion.Panel>
              <div
                style={{ display: "flex", flexDirection: "column", gap: 10 }}
              >
                <div style={{ width: "100%" }}>
                  <div
                    style={{
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "space-between",
                      gap: 12,
                      marginBottom: 6,
                    }}
                  >
                    <Text size="sm" c="dimmed">
                      Prompt (optional)
                    </Text>
                    {isPrompt224CharLimited ? (
                      <Text size="xs" c="dimmed">
                        {localSttTranscriptionPrompt.length}/{promptMaxChars}{" "}
                        chars
                      </Text>
                    ) : null}
                  </div>

                  <Textarea
                    value={localSttTranscriptionPrompt}
                    onChange={(e) => {
                      const next = e.currentTarget.value;

                      if (
                        isPrompt224CharLimited &&
                        next.length > promptMaxChars
                      ) {
                        setLocalSttTranscriptionPrompt(
                          next.slice(0, promptMaxChars)
                        );
                        return;
                      }

                      setLocalSttTranscriptionPrompt(next);
                    }}
                    disabled={!sttPromptSupported}
                    placeholder={"Prompt"}
                    autosize
                    minRows={2}
                    maxLength={
                      isPrompt224CharLimited ? promptMaxChars : undefined
                    }
                    styles={{
                      input: {
                        backgroundColor: "var(--bg-elevated)",
                        borderColor: "var(--border-default)",
                        color: "var(--text-primary)",
                        fontFamily: "monospace",
                        fontSize: "13px",
                      },
                    }}
                  />

                  <Text size="xs" c="dimmed" style={{ marginTop: 6 }}>
                    OpenAI transcription models and Groq Whisper models accept
                    an optional <code>prompt</code>. For Whisper-style models,
                    docs commonly note only the first 224 tokens are considered;
                    this UI conservatively limits it to 224 characters.
                  </Text>
                </div>
              </div>
            </Accordion.Panel>
          </Accordion.Item>

          <Accordion.Item value={`${activeProfileId}-stt-test`}>
            <Accordion.Control>
              <div>
                <p className="settings-label">Test transcription</p>
                <p className="settings-description">
                  Run STT on the last recorded audio to validate provider/model
                  settings.
                </p>
              </div>
            </Accordion.Control>
            <Accordion.Panel>
              <div
                style={{ display: "flex", flexDirection: "column", gap: 10 }}
              >
                <div
                  style={{
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "space-between",
                    gap: 12,
                  }}
                >
                  <Text size="sm" c="dimmed">
                    {testSttLastAudio.isPending
                      ? "Duration: running…"
                      : sttTestDurationMs === null
                      ? "Duration: —"
                      : `Duration: ${(sttTestDurationMs / 1000).toFixed(2)}s`}
                  </Text>

                  <div
                    style={{
                      display: "flex",
                      alignItems: "center",
                      gap: 12,
                      marginLeft: "auto",
                    }}
                  >
                    <Text size="sm" c="dimmed">
                      Test with last created audio
                    </Text>
                    <Tooltip
                      label={
                        hasLastAudioForSttTest
                          ? undefined
                          : "No previous audio found. Record once (toggle/hold hotkey), then come back and click Test."
                      }
                      withArrow
                      disabled={Boolean(hasLastAudioForSttTest)}
                    >
                      <span>
                        <Button
                          color="gray"
                          loading={testSttLastAudio.isPending}
                          disabled={!hasLastAudioForSttTest}
                          onClick={() => {
                            setSttTestError("");
                            setSttTestOutput("");
                            setSttTestDurationMs(null);
                            sttTestStartRef.current = performance.now();

                            testSttLastAudio.mutate(
                              {
                                profileId: activeProfileId,
                              },
                              {
                                onSuccess: (res) => {
                                  const startedAt = sttTestStartRef.current;
                                  sttTestStartRef.current = null;
                                  if (typeof startedAt === "number") {
                                    setSttTestDurationMs(
                                      performance.now() - startedAt
                                    );
                                  }

                                  setSttTestOutput(res);
                                },
                                onError: (err) => {
                                  const startedAt = sttTestStartRef.current;
                                  sttTestStartRef.current = null;
                                  if (typeof startedAt === "number") {
                                    setSttTestDurationMs(
                                      performance.now() - startedAt
                                    );
                                  }

                                  setSttTestError(errorToMessage(err));
                                },
                              }
                            );
                          }}
                        >
                          Test
                        </Button>
                      </span>
                    </Tooltip>
                  </div>
                </div>

                <div style={{ width: "100%" }}>
                  {sttTestError ? (
                    <Text size="sm" c="red" style={{ marginBottom: 8 }}>
                      {sttTestError}
                    </Text>
                  ) : null}

                  {sttPromptSupported &&
                  (settings?.stt_transcription_prompt ?? "").trim().length >
                    0 ? (
                    <Text size="xs" c="dimmed" style={{ marginBottom: 8 }}>
                      Test transcription will include your global transcription
                      prompt.
                    </Text>
                  ) : null}

                  <Textarea
                    value={sttTestOutput}
                    readOnly
                    placeholder="Transcript will appear here"
                    autosize
                    minRows={3}
                    styles={{
                      input: {
                        backgroundColor: "var(--bg-elevated)",
                        borderColor: "var(--border-default)",
                        color: "var(--text-primary)",
                        fontFamily: "monospace",
                        fontSize: "13px",
                      },
                    }}
                  />
                </div>
              </div>
            </Accordion.Panel>
          </Accordion.Item>
        </Accordion>
      </div>

      <Divider
        my="md"
        label="Language model"
        labelPosition="left"
        styles={{
          root: {
            borderTopWidth: 2,
            borderColor: "var(--border-default)",
          },
          label: {
            color: "var(--text-primary)",
            fontSize: 11,
            fontWeight: 600,
            letterSpacing: "0.08em",
            textTransform: "uppercase",
          },
        }}
      />

      <div className="settings-row">
        <div>
          <p className="settings-label">Rewrite Transcription</p>
          <p className="settings-description">
            Enable or disable rewriting the transcription with an LLM
          </p>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          {!isDefaultScope && rewriteEnabledInheriting && (
            <Tooltip label={INHERIT_TOOLTIP} withArrow>
              <Info size={14} style={{ opacity: 0.5, flexShrink: 0 }} />
            </Tooltip>
          )}
          {!isDefaultScope && !rewriteEnabledInheriting && (
            <Tooltip label="Disable override (inherit from Default)" withArrow>
              <ActionIcon
                variant="subtle"
                color="gray"
                size="sm"
                onClick={() =>
                  openDisableOverrideDialog({
                    title: "Disable Rewrite Transcription override?",
                    onConfirm: () => {
                      setRewriteEnabledInheriting(true);
                      setLocalProfileRewriteEnabled(defaultRewriteEnabled);
                      saveProfileMetadata({ rewrite_llm_enabled: null });
                    },
                  })
                }
              >
                <RotateCcw size={14} style={{ opacity: 0.65 }} />
              </ActionIcon>
            </Tooltip>
          )}
          <Switch
            checked={
              isDefaultScope
                ? defaultRewriteEnabled
                : localProfileRewriteEnabled
            }
            onChange={(e) => {
              const enabled = e.currentTarget.checked;
              if (isDefaultScope) {
                updateRewriteLlmEnabled.mutate(enabled, {
                  onSuccess: () => {
                    tauriAPI.emitSettingsChanged();
                  },
                });
                return;
              }

              setRewriteEnabledInheriting(false);
              setLocalProfileRewriteEnabled(enabled);
              saveProfileMetadata({ rewrite_llm_enabled: enabled });
            }}
            color="gray"
            size="md"
          />
        </div>
      </div>

      <div className="settings-row">
        <div>
          <p className="settings-label">Language Model Provider</p>
          <p className="settings-description">AI service for text formatting</p>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          {!isDefaultScope && llmProviderInheriting && (
            <Tooltip label={INHERIT_TOOLTIP} withArrow>
              <Info size={14} style={{ opacity: 0.5, flexShrink: 0 }} />
            </Tooltip>
          )}
          {!isDefaultScope && !llmProviderInheriting && (
            <Tooltip label="Disable override (inherit from Default)" withArrow>
              <ActionIcon
                variant="subtle"
                color="gray"
                size="sm"
                onClick={() =>
                  openDisableOverrideDialog({
                    title: "Disable Language Model Provider override?",
                    onConfirm: () => {
                      setLlmProviderInheriting(true);
                      setLlmModelInheriting(true);
                      setLocalProfileLlmProvider(
                        settings?.llm_provider ?? null
                      );
                      setLocalProfileLlmModel(settings?.llm_model ?? null);
                      saveProfileMetadata({
                        llm_provider: null,
                        llm_model: null,
                      });
                    },
                  })
                }
              >
                <RotateCcw size={14} style={{ opacity: 0.65 }} />
              </ActionIcon>
            </Tooltip>
          )}
          <Select
            data={llmProviderOptions}
            value={
              isDefaultScope
                ? settings?.llm_provider ?? null
                : localProfileLlmProvider
            }
            onChange={(value) => {
              if (!value) return;
              if (isDefaultScope) {
                handleDefaultLLMProviderChange(value);
                return;
              }

              setLlmProviderInheriting(false);
              setLlmModelInheriting(false);
              setLocalProfileLlmProvider(value);
              const models = LLM_MODELS[value] ?? [];
              const firstModel = models[0]?.value ?? null;
              setLocalProfileLlmModel(firstModel);
              saveProfileMetadata({
                llm_provider: value,
                llm_model: firstModel,
              });
            }}
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
        </div>
      </div>

      {llmModelOptions.length > 0 ? (
        <div className="settings-row">
          <div>
            <p className="settings-label">Rewrite LLM Model</p>
            <p className="settings-description">
              LLM Model used to rewrite the transcription.
            </p>
          </div>
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            {!isDefaultScope && llmModelInheriting && (
              <Tooltip label={INHERIT_TOOLTIP} withArrow>
                <Info size={14} style={{ opacity: 0.5, flexShrink: 0 }} />
              </Tooltip>
            )}
            {!isDefaultScope && !llmModelInheriting && (
              <Tooltip
                label="Disable override (inherit from Default)"
                withArrow
              >
                <ActionIcon
                  variant="subtle"
                  color="gray"
                  size="sm"
                  onClick={() =>
                    openDisableOverrideDialog({
                      title: "Disable Rewrite LLM Model override?",
                      onConfirm: () => {
                        setLlmModelInheriting(true);
                        setLocalProfileLlmModel(settings?.llm_model ?? null);
                        saveProfileMetadata({ llm_model: null });
                      },
                    })
                  }
                >
                  <RotateCcw size={14} style={{ opacity: 0.65 }} />
                </ActionIcon>
              </Tooltip>
            )}
            <Select
              data={llmModelOptions}
              value={
                isDefaultScope
                  ? settings?.llm_model ?? llmModelOptions[0]?.value ?? null
                  : localProfileLlmModel
              }
              onChange={(value) => {
                if (!value) return;
                if (isDefaultScope) {
                  handleDefaultLLMModelChange(value);
                  return;
                }

                setLlmModelInheriting(false);
                setLocalProfileLlmModel(value);
                saveProfileMetadata({ llm_model: value });
              }}
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
        </div>
      ) : null}

      <div style={{ marginTop: 16 }}>
        <Accordion variant="separated" radius="md">
          <Accordion.Item value={`${activeProfileId}-test-rewrite`}>
            <Accordion.Control>
              <div>
                <p className="settings-label">Test rewrite</p>
                <p className="settings-description">
                  Paste a raw transcript and run it through the rewrite step.
                </p>
              </div>
            </Accordion.Control>
            <Accordion.Panel>
              <div
                style={{ display: "flex", flexDirection: "column", gap: 10 }}
              >
                <Textarea
                  value={rewriteTestInput}
                  onChange={(e) => {
                    setRewriteTestInput(e.currentTarget.value);
                  }}
                  placeholder="um like okay so today we're going to talk about..."
                  autosize
                  minRows={3}
                  styles={{
                    input: {
                      backgroundColor: "var(--bg-elevated)",
                      borderColor: "var(--border-default)",
                      color: "var(--text-primary)",
                      fontFamily: "monospace",
                      fontSize: "13px",
                    },
                  }}
                />

                <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
                  <Button
                    color="gray"
                    loading={testLlmRewrite.isPending}
                    disabled={rewriteTestInput.trim().length === 0}
                    onClick={() => {
                      setRewriteTestError("");
                      setRewriteTestOutput("");
                      setRewriteTestDurationMs(null);
                      rewriteTestStartRef.current = performance.now();

                      testLlmRewrite.mutate(
                        {
                          transcript: rewriteTestInput,
                          profileId: activeProfileId,
                        },
                        {
                          onSuccess: (res) => {
                            const startedAt = rewriteTestStartRef.current;
                            rewriteTestStartRef.current = null;
                            if (typeof startedAt === "number") {
                              setRewriteTestDurationMs(
                                performance.now() - startedAt
                              );
                            }
                            setRewriteTestOutput(res.output);
                          },
                          onError: (err) => {
                            const startedAt = rewriteTestStartRef.current;
                            rewriteTestStartRef.current = null;
                            if (typeof startedAt === "number") {
                              setRewriteTestDurationMs(
                                performance.now() - startedAt
                              );
                            }
                            setRewriteTestError(errorToMessage(err));
                          },
                        }
                      );
                    }}
                  >
                    Test
                  </Button>

                  <Text size="sm" c="dimmed">
                    {testLlmRewrite.isPending
                      ? "Duration: running…"
                      : rewriteTestDurationMs === null
                      ? "Duration: —"
                      : `Duration: ${(rewriteTestDurationMs / 1000).toFixed(2)}s`}
                  </Text>
                </div>

                {rewriteTestError ? (
                  <Text size="sm" c="red">
                    {rewriteTestError}
                  </Text>
                ) : null}

                {rewriteTestOutput ? (
                  <Textarea
                    value={rewriteTestOutput}
                    readOnly
                    autosize
                    minRows={3}
                    styles={{
                      input: {
                        backgroundColor: "var(--bg-elevated)",
                        borderColor: "var(--border-default)",
                        color: "var(--text-primary)",
                        fontFamily: "monospace",
                        fontSize: "13px",
                      },
                    }}
                  />
                ) : null}
              </div>
            </Accordion.Panel>
          </Accordion.Item>

          <PromptSectionEditor
            sectionKey={`${activeProfileId}-main-prompt`}
            title="Core Formatting Rules"
            description="Filler word removal, punctuation, capitalization"
            enabled={true}
            hideToggle={true}
            initialContent={localSections!.main.content}
            defaultContent={defaultSections?.main ?? ""}
            hasCustom={hasCustomContent.main}
            inheritMode={
              isDefaultScope
                ? null
                : activeProfile?.cleanup_prompt_sections?.main == null
                ? "inheriting"
                : "overriding"
            }
            onDisableOverride={
              isDefaultScope
                ? undefined
                : () =>
                    openDisableOverrideDialog({
                      title: "Disable Core Formatting Rules override?",
                      onConfirm: () => {
                        const base =
                          settings?.cleanup_prompt_sections ?? DEFAULT_SECTIONS;

                        const current: CleanupPromptSectionsOverride =
                          activeProfile?.cleanup_prompt_sections ?? {};
                        const next = normalizePromptOverrides({
                          ...current,
                          main: null,
                        });
                        profilePromptOverridesRef.current = next;

                        const resolved: CleanupPromptSections = {
                          main: next?.main ?? base.main,
                          advanced: next?.advanced ?? base.advanced,
                          dictionary: next?.dictionary ?? base.dictionary,
                        };

                        setLocalSections({
                          main: {
                            enabled: resolved.main.enabled,
                            content:
                              resolved.main.content ?? defaultSections!.main,
                          },
                          advanced: {
                            enabled: resolved.advanced.enabled,
                            content:
                              resolved.advanced.content ??
                              defaultSections!.advanced,
                          },
                          dictionary: {
                            enabled: resolved.dictionary.enabled,
                            content:
                              resolved.dictionary.content ??
                              defaultSections!.dictionary,
                          },
                        });

                        saveProfileMetadata({ cleanup_prompt_sections: next });
                      },
                    })
            }
            onToggle={() => {}}
            onSave={(content) => handleSave("main", content)}
            onReset={() => handleReset("main")}
            isSaving={
              updateCleanupPromptSections.isPending ||
              updateRewriteProgramPromptProfiles.isPending
            }
          />

          <PromptSectionEditor
            sectionKey={`${activeProfileId}-advanced-prompt`}
            title="Advanced Features"
            description='Backtrack corrections ("scratch that") and list formatting'
            enabled={localSections!.advanced.enabled}
            initialContent={localSections!.advanced.content}
            defaultContent={defaultSections?.advanced ?? ""}
            hasCustom={hasCustomContent.advanced}
            inheritMode={
              isDefaultScope
                ? null
                : activeProfile?.cleanup_prompt_sections?.advanced == null
                ? "inheriting"
                : "overriding"
            }
            onDisableOverride={
              isDefaultScope
                ? undefined
                : () =>
                    openDisableOverrideDialog({
                      title: "Disable Advanced Features override?",
                      onConfirm: () => {
                        const base =
                          settings?.cleanup_prompt_sections ?? DEFAULT_SECTIONS;

                        const current: CleanupPromptSectionsOverride =
                          activeProfile?.cleanup_prompt_sections ?? {};
                        const next = normalizePromptOverrides({
                          ...current,
                          advanced: null,
                        });
                        profilePromptOverridesRef.current = next;

                        const resolved: CleanupPromptSections = {
                          main: next?.main ?? base.main,
                          advanced: next?.advanced ?? base.advanced,
                          dictionary: next?.dictionary ?? base.dictionary,
                        };

                        setLocalSections({
                          main: {
                            enabled: resolved.main.enabled,
                            content:
                              resolved.main.content ?? defaultSections!.main,
                          },
                          advanced: {
                            enabled: resolved.advanced.enabled,
                            content:
                              resolved.advanced.content ??
                              defaultSections!.advanced,
                          },
                          dictionary: {
                            enabled: resolved.dictionary.enabled,
                            content:
                              resolved.dictionary.content ??
                              defaultSections!.dictionary,
                          },
                        });

                        saveProfileMetadata({ cleanup_prompt_sections: next });
                      },
                    })
            }
            onToggle={(checked) => handleToggle("advanced", checked)}
            onSave={(content) => handleSave("advanced", content)}
            onReset={() => handleReset("advanced")}
            isSaving={
              updateCleanupPromptSections.isPending ||
              updateRewriteProgramPromptProfiles.isPending
            }
          />

          <PromptSectionEditor
            sectionKey={`${activeProfileId}-dictionary-prompt`}
            title="Personal Dictionary"
            description="Custom word mappings for technical terms"
            enabled={localSections!.dictionary.enabled}
            initialContent={localSections!.dictionary.content}
            defaultContent={defaultSections?.dictionary ?? ""}
            hasCustom={hasCustomContent.dictionary}
            inheritMode={
              isDefaultScope
                ? null
                : activeProfile?.cleanup_prompt_sections?.dictionary == null
                ? "inheriting"
                : "overriding"
            }
            onDisableOverride={
              isDefaultScope
                ? undefined
                : () =>
                    openDisableOverrideDialog({
                      title: "Disable Personal Dictionary override?",
                      onConfirm: () => {
                        const base =
                          settings?.cleanup_prompt_sections ?? DEFAULT_SECTIONS;

                        const current: CleanupPromptSectionsOverride =
                          activeProfile?.cleanup_prompt_sections ?? {};
                        const next = normalizePromptOverrides({
                          ...current,
                          dictionary: null,
                        });
                        profilePromptOverridesRef.current = next;

                        const resolved: CleanupPromptSections = {
                          main: next?.main ?? base.main,
                          advanced: next?.advanced ?? base.advanced,
                          dictionary: next?.dictionary ?? base.dictionary,
                        };

                        setLocalSections({
                          main: {
                            enabled: resolved.main.enabled,
                            content:
                              resolved.main.content ?? defaultSections!.main,
                          },
                          advanced: {
                            enabled: resolved.advanced.enabled,
                            content:
                              resolved.advanced.content ??
                              defaultSections!.advanced,
                          },
                          dictionary: {
                            enabled: resolved.dictionary.enabled,
                            content:
                              resolved.dictionary.content ??
                              defaultSections!.dictionary,
                          },
                        });

                        saveProfileMetadata({ cleanup_prompt_sections: next });
                      },
                    })
            }
            onToggle={(checked) => handleToggle("dictionary", checked)}
            onSave={(content) => handleSave("dictionary", content)}
            onReset={() => handleReset("dictionary")}
            isSaving={
              updateCleanupPromptSections.isPending ||
              updateRewriteProgramPromptProfiles.isPending
            }
          />
        </Accordion>
      </div>
    </>
  );
}
