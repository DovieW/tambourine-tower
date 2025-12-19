import {
  ActionIcon,
  Button,
  Group,
  Loader,
  Modal,
  Select,
  Text,
  TextInput,
  Tooltip,
} from "@mantine/core";
import { Crosshair, Plus, RotateCcw, Trash2 } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  useSettings,
  useUpdateRewriteProgramPromptProfiles,
} from "../../lib/queries";
import type { OpenWindowInfo } from "../../lib/tauri";
import { tauriAPI, type RewriteProgramPromptProfile } from "../../lib/tauri";

function createId(): string {
  // `crypto.randomUUID()` is available in modern browsers; keep a fallback for safety.
  // This only needs to be unique enough for local settings.
  return (
    globalThis.crypto?.randomUUID?.() ??
    `id_${Date.now()}_${Math.random().toString(16).slice(2)}`
  );
}

export function ProfileConfigModal({
  opened,
  onClose,
  editingProfileId,
  onEditingProfileChange,
  autoCreateProfile,
}: {
  opened: boolean;
  onClose: () => void;
  editingProfileId: string;
  onEditingProfileChange: (id: string) => void;
  autoCreateProfile?: boolean;
}) {
  const { data: settings, isLoading } = useSettings();
  const updateRewriteProgramPromptProfiles =
    useUpdateRewriteProgramPromptProfiles();

  const profiles: RewriteProgramPromptProfile[] =
    settings?.rewrite_program_prompt_profiles ?? [];

  const didAutoCreate = useRef(false);

  useEffect(() => {
    if (!opened) {
      didAutoCreate.current = false;
      return;
    }
    if (!autoCreateProfile) return;
    if (didAutoCreate.current) return;
    if (!settings) return;
    didAutoCreate.current = true;
    addProfile();
  }, [opened, autoCreateProfile, settings, addProfile]);

  const selectedProfile: RewriteProgramPromptProfile | null =
    editingProfileId !== "default"
      ? profiles.find((p) => p.id === editingProfileId) ?? null
      : null;

  const [localName, setLocalName] = useState<string>("");
  const [localPaths, setLocalPaths] = useState<string[]>([]);

  useEffect(() => {
    if (!selectedProfile) {
      setLocalName("");
      setLocalPaths([]);
      return;
    }
    setLocalName(selectedProfile.name);
    setLocalPaths(selectedProfile.program_paths ?? []);
  }, [selectedProfile]);

  const saveProfileMetadata = (
    partial: Partial<RewriteProgramPromptProfile>
  ) => {
    if (!selectedProfile) return;

    const next = profiles.map((p) =>
      p.id === selectedProfile.id ? { ...p, ...partial } : p
    );

    updateRewriteProgramPromptProfiles.mutate(next, {
      onSuccess: () => {
        tauriAPI.emitSettingsChanged();
      },
    });
  };

  function addProfile() {
    // Create new profile with all settings in inherit mode (null)
    // Settings will inherit from default until user explicitly changes them
    const newProfile: RewriteProgramPromptProfile = {
      id: createId(),
      name: "New Profile",
      program_paths: [],
      cleanup_prompt_sections: null, // inherit
      // AI settings - all inherit
      stt_provider: null,
      stt_model: null,
      stt_timeout_seconds: null,
      llm_provider: null,
      llm_model: null,
      rewrite_llm_enabled: null,
      // Audio & Overlay settings - all inherit
      sound_enabled: null,
      auto_mute_audio: null,
      overlay_mode: null,
      widget_position: null,
      output_mode: null,
    };

    const next = [...profiles, newProfile];
    updateRewriteProgramPromptProfiles.mutate(next, {
      onSuccess: () => {
        tauriAPI.emitSettingsChanged();
        onEditingProfileChange(newProfile.id);
      },
    });
  }

  const deleteSelectedProfile = () => {
    if (!selectedProfile) return;

    const name = selectedProfile.name.trim() || "this profile";
    if (!window.confirm(`Delete ${name}? This cannot be undone.`)) return;

    const next = profiles.filter((p) => p.id !== selectedProfile.id);
    updateRewriteProgramPromptProfiles.mutate(next, {
      onSuccess: () => {
        tauriAPI.emitSettingsChanged();
        // If we just deleted the currently selected profile, close the modal
        // and return to Default scope.
        if (editingProfileId === selectedProfile.id) {
          onEditingProfileChange("default");
          onClose();
        }
      },
    });
  };

  const disableAllOverridesForSelectedProfile = () => {
    if (!selectedProfile) return;

    const name = selectedProfile.name.trim() || "this profile";
    const ok = window.confirm(
      `Disable all overrides for ${name}?\n\nThis will reset the profile to inherit all settings from Default. Program matching will remain unchanged.`
    );
    if (!ok) return;

    saveProfileMetadata({
      // Prompts
      cleanup_prompt_sections: null,

      // AI settings
      stt_provider: null,
      stt_model: null,
      stt_timeout_seconds: null,
      llm_provider: null,
      llm_model: null,
      rewrite_llm_enabled: null,

      // Audio & Overlay settings
      sound_enabled: null,
      auto_mute_audio: null,
      overlay_mode: null,
      widget_position: null,
      output_mode: null,
    });
  };

  const updateProgramPathAtIndex = (idx: number, nextValue: string) => {
    const next = [...localPaths];
    next[idx] = nextValue;
    setLocalPaths(next);
  };

  const persistProgramPaths = (paths: string[]) => {
    if (!selectedProfile) return;
    saveProfileMetadata({ program_paths: paths });
  };

  const removeProgramPathAtIndex = (idx: number) => {
    const next = localPaths.filter((_, i) => i !== idx);
    setLocalPaths(next);
    persistProgramPaths(next);
  };

  const addEmptyProgramPath = () => {
    if (!selectedProfile) return;
    setLocalPaths((prev) => [...prev, ""]);
  };

  const hasAnyConfiguredProgram = useMemo(
    () => localPaths.some((p) => p.trim().length > 0),
    [localPaths]
  );

  // Window picker (open app list)
  const [windowPickerOpen, setWindowPickerOpen] = useState(false);
  const [windowPickerDropdownOpened, setWindowPickerDropdownOpened] =
    useState(false);
  const [openWindows, setOpenWindows] = useState<OpenWindowInfo[]>([]);
  const [isLoadingWindows, setIsLoadingWindows] = useState(false);

  const windowPickerOptions = useMemo(() => {
    const byPath = new Map<string, OpenWindowInfo>();
    for (const w of openWindows) {
      if (!byPath.has(w.process_path)) {
        byPath.set(w.process_path, w);
      }
    }

    return Array.from(byPath.values()).map((w) => ({
      value: w.process_path,
      label: `${w.title} — ${w.process_path}`,
    }));
  }, [openWindows]);

  const openWindowPicker = async () => {
    setWindowPickerOpen(true);
    setWindowPickerDropdownOpened(true);
    setIsLoadingWindows(true);
    try {
      const windows = await tauriAPI.listOpenWindows();
      setOpenWindows(windows);
    } finally {
      setIsLoadingWindows(false);
    }
  };

  const addProgramPathFromPicker = (value: string) => {
    if (!selectedProfile) return;
    const trimmed = value.trim();
    if (!trimmed) return;

    const existing = new Set(localPaths.map((p) => p.trim()).filter(Boolean));
    if (existing.has(trimmed)) {
      setWindowPickerOpen(false);
      return;
    }

    const firstEmptyIndex = localPaths.findIndex((p) => p.trim().length === 0);

    const nextLocal = [...localPaths];
    if (firstEmptyIndex >= 0) {
      nextLocal[firstEmptyIndex] = trimmed;
    } else {
      nextLocal.push(trimmed);
    }

    // Persist a clean list (trim, remove empties, de-dupe while preserving order).
    const deduped: string[] = [];
    const seen = new Set<string>();
    for (const p of nextLocal) {
      const v = p.trim();
      if (!v) continue;
      if (seen.has(v)) continue;
      seen.add(v);
      deduped.push(v);
    }

    setLocalPaths(deduped);
    persistProgramPaths(deduped);
    setWindowPickerOpen(false);
  };

  const isBusy = isLoading || updateRewriteProgramPromptProfiles.isPending;

  return (
    <>
      <Modal opened={opened} onClose={onClose} title="Profile config" centered>
        {editingProfileId === "default" ? (
          <div style={{ fontSize: 12, opacity: 0.75, padding: "8px 0" }}>
            Select a profile in the Profile dropdown above, then click the gear
            icon to configure it.
          </div>
        ) : null}

        {editingProfileId !== "default" && !selectedProfile ? (
          <div style={{ fontSize: 12, opacity: 0.75, padding: "8px 0" }}>
            That profile no longer exists.
          </div>
        ) : null}

        {selectedProfile ? (
          <>
            <TextInput
              label="Profile name"
              placeholder="e.g. VS Code"
              value={localName}
              onChange={(e) => setLocalName(e.currentTarget.value)}
              onBlur={() => {
                if (!selectedProfile) return;
                if (localName !== selectedProfile.name) {
                  saveProfileMetadata({ name: localName });
                }
              }}
              styles={{ label: { fontSize: 12 } }}
            />

            <Group justify="space-between" align="center" mt="sm" mb={6}>
              <div style={{ fontSize: 12, fontWeight: 500 }}>Programs</div>
              <Group gap={8}>
                <Tooltip
                  label="Pick from currently open apps"
                  withArrow
                  position="bottom"
                >
                  <ActionIcon
                    variant="subtle"
                    color="gray"
                    onClick={openWindowPicker}
                    aria-label="Pick from open programs"
                    disabled={isBusy}
                  >
                    <Crosshair size={16} />
                  </ActionIcon>
                </Tooltip>

                <Tooltip label="Add a program path" withArrow position="bottom">
                  <ActionIcon
                    variant="light"
                    color="gray"
                    onClick={addEmptyProgramPath}
                    aria-label="Add program"
                    disabled={isBusy}
                  >
                    <Plus size={16} />
                  </ActionIcon>
                </Tooltip>
              </Group>
            </Group>

            {localPaths.length === 0 ? (
              <div style={{ fontSize: 12, opacity: 0.7, paddingBottom: 8 }}>
                No programs yet. Add one to apply this profile automatically.
              </div>
            ) : null}

            {localPaths.map((path, idx) => (
              <Group
                key={`${selectedProfile.id}-path-${idx}`}
                mt={6}
                wrap="nowrap"
              >
                <TextInput
                  placeholder="C:\\Program Files\\...\\app.exe"
                  value={path}
                  onChange={(e) =>
                    updateProgramPathAtIndex(idx, e.currentTarget.value)
                  }
                  onBlur={() => {
                    const trimmed = localPaths.map((p) => p.trim());
                    const filtered = trimmed.filter((p) => p.length > 0);

                    const deduped: string[] = [];
                    const seen = new Set<string>();
                    for (const p of filtered) {
                      if (!seen.has(p)) {
                        seen.add(p);
                        deduped.push(p);
                      }
                    }

                    setLocalPaths(deduped);
                    if (
                      JSON.stringify(deduped) !==
                      JSON.stringify(selectedProfile.program_paths ?? [])
                    ) {
                      persistProgramPaths(deduped);
                    }
                  }}
                  styles={{ input: { fontSize: 12 } }}
                  style={{ flex: 1 }}
                />
                <ActionIcon
                  variant="subtle"
                  color="red"
                  aria-label="Remove program"
                  onClick={() => removeProgramPathAtIndex(idx)}
                  disabled={isBusy}
                >
                  <Trash2 size={16} />
                </ActionIcon>
              </Group>
            ))}

            <Group justify="space-between" mt="md">
              {!hasAnyConfiguredProgram ? (
                <div style={{ fontSize: 12, opacity: 0.7 }}>
                  Tip: you can use the crosshair to pick from open apps.
                </div>
              ) : (
                <div />
              )}
              <Group gap={8}>
                <Button
                  size="xs"
                  variant="light"
                  color="gray"
                  leftSection={<RotateCcw size={14} />}
                  onClick={disableAllOverridesForSelectedProfile}
                  disabled={isBusy}
                >
                  Disable all overrides
                </Button>

                <Button
                  size="xs"
                  variant="light"
                  color="red"
                  leftSection={<Trash2 size={14} />}
                  onClick={deleteSelectedProfile}
                  disabled={isBusy}
                >
                  Delete profile
                </Button>
              </Group>
            </Group>
          </>
        ) : null}

        {updateRewriteProgramPromptProfiles.isPending ? (
          <Group justify="center" mt="md">
            <Loader size="sm" color="gray" />
          </Group>
        ) : null}
      </Modal>

      <Modal
        opened={windowPickerOpen}
        onClose={() => {
          setWindowPickerOpen(false);
          setWindowPickerDropdownOpened(false);
        }}
        title="Pick an open program"
        centered
      >
        {isLoadingWindows ? (
          <Group justify="center" p="md">
            <Loader size="sm" color="gray" />
          </Group>
        ) : (
          <Select
            searchable
            nothingFoundMessage="No windows found"
            placeholder="Select a window"
            data={windowPickerOptions}
            dropdownOpened={windowPickerDropdownOpened}
            onDropdownOpen={() => setWindowPickerDropdownOpened(true)}
            onDropdownClose={() => setWindowPickerDropdownOpened(false)}
            onChange={(value) => {
              if (!value) return;
              addProgramPathFromPicker(value);
            }}
          />
        )}
      </Modal>
    </>
  );
}

// Backward compatible alias (some code still imports ProgramsModal)
export const ProgramsModal = ProfileConfigModal;
