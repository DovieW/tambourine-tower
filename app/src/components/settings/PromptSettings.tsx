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
  Text,
  Tooltip,
} from "@mantine/core";
import { Info, RotateCcw } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  useDefaultSections,
  useSettings,
  useAvailableProviders,
  useUpdateCleanupPromptSections,
  useUpdateLLMModel,
  useUpdateLLMProvider,
  useUpdateRewriteProgramPromptProfiles,
  useUpdateRewriteLlmEnabled,
  useUpdateSTTModel,
  useUpdateSTTProvider,
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
    { value: "gpt-4o-mini", label: "GPT-4o Mini" },
    { value: "gpt-4o", label: "GPT-4o" },
    { value: "gpt-4-turbo", label: "GPT-4 Turbo" },
  ],
  anthropic: [
    { value: "claude-3-5-haiku-latest", label: "Claude 3.5 Haiku" },
    { value: "claude-3-5-sonnet-latest", label: "Claude 3.5 Sonnet" },
    { value: "claude-3-opus-latest", label: "Claude 3 Opus" },
  ],
  ollama: [],
};

type SectionKey = "main" | "advanced" | "dictionary";

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

  // Default profile (global) provider settings
  const updateSTTProvider = useUpdateSTTProvider();
  const updateSTTModel = useUpdateSTTModel();
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
  const effectiveLlmProvider =
    activeProfileId === "default"
      ? settings?.llm_provider ?? null
      : localProfileLlmProvider ?? settings?.llm_provider ?? null;

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

      <Divider my="md" label="Speech-to-text" labelPosition="left" />

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

              const clamped = Math.max(0.5, Math.min(3.0, value));
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
            min={0.5}
            max={3.0}
            step={0.1}
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

      <Divider my="md" label="Language model" labelPosition="left" />

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
