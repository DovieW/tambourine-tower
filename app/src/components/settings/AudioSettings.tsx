import {
  ActionIcon,
  Button,
  Group,
  Modal,
  Select,
  Switch,
  Tooltip,
} from "@mantine/core";
import { Info, RotateCcw } from "lucide-react";
import { useState } from "react";
import {
  useIsAudioMuteSupported,
  useSettings,
  useUpdateAutoMuteAudio,
  useUpdateOutputMode,
  useUpdateOverlayMode,
  useUpdateRewriteProgramPromptProfiles,
  useUpdateSoundEnabled,
  useUpdateWidgetPosition,
} from "../../lib/queries";
import type {
  OutputMode,
  OverlayMode,
  RewriteProgramPromptProfile,
  WidgetPosition,
} from "../../lib/tauri";
import { DeviceSelector } from "../DeviceSelector";

const GLOBAL_ONLY_TOOLTIP =
  "This setting can only be changed in the Default profile";
const INHERIT_TOOLTIP = "Inheriting from Default profile";

/** Helper to check if a profile value is inheriting (null/undefined) */
function isInheriting<T>(value: T | null | undefined): boolean {
  return value === null || value === undefined;
}

const OVERLAY_MODE_OPTIONS = [
  { value: "always", label: "Always visible" },
  { value: "recording_only", label: "Only when recording" },
  { value: "never", label: "Hidden" },
];

const WIDGET_POSITION_OPTIONS = [
  { value: "top-left", label: "Top Left" },
  { value: "top-center", label: "Top Center" },
  { value: "top-right", label: "Top Right" },
  { value: "center", label: "Center" },
  { value: "bottom-left", label: "Bottom Left" },
  { value: "bottom-center", label: "Bottom Center" },
  { value: "bottom-right", label: "Bottom Right" },
];

const OUTPUT_MODE_OPTIONS = [
  { value: "paste", label: "Paste" },
  { value: "paste_and_clipboard", label: "Paste and clipboard" },
  { value: "clipboard", label: "Clipboard" },
];

function getProfileValue<T>(
  profileValue: T | null | undefined,
  globalValue: T
): T {
  return profileValue ?? globalValue;
}

export function AudioSettings({
  editingProfileId,
}: {
  editingProfileId?: string;
}) {
  const { data: settings, isLoading } = useSettings();
  const { data: isAudioMuteSupported } = useIsAudioMuteSupported();
  const updateSoundEnabled = useUpdateSoundEnabled();
  const updateAutoMuteAudio = useUpdateAutoMuteAudio();
  const updateOverlayMode = useUpdateOverlayMode();
  const updateWidgetPosition = useUpdateWidgetPosition();
  const updateOutputMode = useUpdateOutputMode();
  const updateRewriteProgramPromptProfiles =
    useUpdateRewriteProgramPromptProfiles();

  const profiles = settings?.rewrite_program_prompt_profiles ?? [];
  const profile: RewriteProgramPromptProfile | null =
    editingProfileId && editingProfileId !== "default"
      ? profiles.find((p) => p.id === editingProfileId) ?? null
      : null;

  const isProfileScope = profile !== null;

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

  const updateProfile = (partial: Partial<RewriteProgramPromptProfile>) => {
    if (!profile) return;
    const next = profiles.map((p) =>
      p.id === profile.id ? { ...p, ...partial } : p
    );
    updateRewriteProgramPromptProfiles.mutate(next);
  };

  // Get effective values (profile value or fall back to global)
  const globalSoundEnabled = settings?.sound_enabled ?? true;
  const soundEnabled = isProfileScope
    ? getProfileValue(profile?.sound_enabled, globalSoundEnabled)
    : globalSoundEnabled;
  const soundInheriting =
    isProfileScope && isInheriting(profile?.sound_enabled);

  const globalAutoMuteAudio = settings?.auto_mute_audio ?? false;
  const autoMuteAudio = isProfileScope
    ? getProfileValue(profile?.auto_mute_audio, globalAutoMuteAudio)
    : globalAutoMuteAudio;
  const autoMuteInheriting =
    isProfileScope && isInheriting(profile?.auto_mute_audio);

  const globalOverlayMode: OverlayMode = settings?.overlay_mode ?? "always";
  const overlayMode = isProfileScope
    ? getProfileValue(profile?.overlay_mode, globalOverlayMode)
    : globalOverlayMode;
  const overlayModeInheriting =
    isProfileScope && isInheriting(profile?.overlay_mode);

  const globalWidgetPosition: WidgetPosition =
    settings?.widget_position ?? "bottom-right";
  const widgetPosition = isProfileScope
    ? getProfileValue(profile?.widget_position, globalWidgetPosition)
    : globalWidgetPosition;
  const widgetPositionInheriting =
    isProfileScope && isInheriting(profile?.widget_position);

  const globalOutputMode: OutputMode = settings?.output_mode ?? "paste";
  const outputMode = isProfileScope
    ? getProfileValue(profile?.output_mode, globalOutputMode)
    : globalOutputMode;
  const outputModeInheriting =
    isProfileScope && isInheriting(profile?.output_mode);

  // Handlers - update profile or global depending on scope
  const handleSoundToggle = (checked: boolean) => {
    if (isProfileScope) {
      updateProfile({ sound_enabled: checked });
      return;
    }
    updateSoundEnabled.mutate(checked);
  };

  const handleAutoMuteToggle = (checked: boolean) => {
    if (isProfileScope) {
      updateProfile({ auto_mute_audio: checked });
      return;
    }
    updateAutoMuteAudio.mutate(checked);
  };

  const handleOverlayModeChange = (value: string | null) => {
    if (!value) return;
    if (isProfileScope) {
      updateProfile({ overlay_mode: value as OverlayMode });
      return;
    }
    updateOverlayMode.mutate(value as OverlayMode);
  };

  const handleWidgetPositionChange = (value: string | null) => {
    if (!value) return;
    if (isProfileScope) {
      updateProfile({ widget_position: value as WidgetPosition });
      return;
    }
    updateWidgetPosition.mutate(value as WidgetPosition);
  };

  const handleOutputModeChange = (value: string | null) => {
    if (!value) return;
    if (isProfileScope) {
      updateProfile({ output_mode: value as OutputMode });
      return;
    }
    updateOutputMode.mutate(value as OutputMode);
  };

  return (
    <>
      <Modal
        opened={resetDialog !== null}
        onClose={() => setResetDialog(null)}
        title={resetDialog?.title ?? ""}
        centered
      >
        <div style={{ fontSize: 13, opacity: 0.85, lineHeight: 1.4 }}>
          This setting is currently overriding the Default profile. Disable the
          override to inherit from Default.
        </div>
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

      {isProfileScope ? (
        <Tooltip label={GLOBAL_ONLY_TOOLTIP} withArrow position="top-start">
          <div
            style={{
              opacity: 0.5,
              cursor: "not-allowed",
            }}
          >
            <div style={{ pointerEvents: "none" }}>
              <DeviceSelector />
            </div>
          </div>
        </Tooltip>
      ) : (
        <DeviceSelector />
      )}

      <div className="settings-row">
        <div>
          <p className="settings-label">Sound feedback</p>
          <p className="settings-description">
            Play sounds when recording starts and stops
          </p>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          {isProfileScope && !soundInheriting && (
            <Tooltip label="Disable override (inherit from Default)" withArrow>
              <ActionIcon
                variant="subtle"
                color="gray"
                size="sm"
                disabled={isLoading}
                onClick={() =>
                  openDisableOverrideDialog({
                    title: "Disable Sound feedback override?",
                    onConfirm: () => updateProfile({ sound_enabled: null }),
                  })
                }
              >
                <RotateCcw size={14} style={{ opacity: 0.65 }} />
              </ActionIcon>
            </Tooltip>
          )}
          {soundInheriting && (
            <Tooltip label={INHERIT_TOOLTIP} withArrow>
              <Info size={14} style={{ opacity: 0.5, flexShrink: 0 }} />
            </Tooltip>
          )}
          <Switch
            checked={soundEnabled}
            onChange={(event) => handleSoundToggle(event.currentTarget.checked)}
            disabled={isLoading}
            color="gray"
            size="md"
          />
        </div>
      </div>
      <div className="settings-row">
        <div>
          <p className="settings-label">Mute audio during recording</p>
          <p className="settings-description">
            Automatically mute system audio while dictating
          </p>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          {isProfileScope && !autoMuteInheriting && (
            <Tooltip label="Disable override (inherit from Default)" withArrow>
              <ActionIcon
                variant="subtle"
                color="gray"
                size="sm"
                disabled={isLoading}
                onClick={() =>
                  openDisableOverrideDialog({
                    title: "Disable Mute audio override?",
                    onConfirm: () => updateProfile({ auto_mute_audio: null }),
                  })
                }
              >
                <RotateCcw size={14} style={{ opacity: 0.65 }} />
              </ActionIcon>
            </Tooltip>
          )}
          {autoMuteInheriting && (
            <Tooltip label={INHERIT_TOOLTIP} withArrow>
              <Info size={14} style={{ opacity: 0.5, flexShrink: 0 }} />
            </Tooltip>
          )}
          <Tooltip
            label="Not supported on this platform"
            disabled={isAudioMuteSupported !== false}
            withArrow
          >
            <Switch
              checked={autoMuteAudio}
              onChange={(event) =>
                handleAutoMuteToggle(event.currentTarget.checked)
              }
              disabled={isLoading || isAudioMuteSupported === false || false}
              color="gray"
              size="md"
            />
          </Tooltip>
        </div>
      </div>
      <div className="settings-row">
        <div>
          <p className="settings-label">Overlay widget</p>
          <p className="settings-description">
            When to show the on-screen recording widget
          </p>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          {isProfileScope && !overlayModeInheriting && (
            <Tooltip label="Disable override (inherit from Default)" withArrow>
              <ActionIcon
                variant="subtle"
                color="gray"
                size="sm"
                disabled={isLoading}
                onClick={() =>
                  openDisableOverrideDialog({
                    title: "Disable Overlay widget override?",
                    onConfirm: () => updateProfile({ overlay_mode: null }),
                  })
                }
              >
                <RotateCcw size={14} style={{ opacity: 0.65 }} />
              </ActionIcon>
            </Tooltip>
          )}
          {overlayModeInheriting && (
            <Tooltip label={INHERIT_TOOLTIP} withArrow>
              <Info size={14} style={{ opacity: 0.5, flexShrink: 0 }} />
            </Tooltip>
          )}
          <Select
            data={OVERLAY_MODE_OPTIONS}
            value={overlayMode}
            onChange={handleOverlayModeChange}
            disabled={isLoading}
            withCheckIcon={false}
            styles={{
              input: {
                backgroundColor: "var(--bg-elevated)",
                borderColor: "var(--border-default)",
                color: "var(--text-primary)",
                minWidth: 180,
              },
            }}
          />
        </div>
      </div>
      <div className="settings-row">
        <div>
          <p className="settings-label">Widget position</p>
          <p className="settings-description">
            Default position of the overlay widget on screen
          </p>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          {isProfileScope && !widgetPositionInheriting && (
            <Tooltip label="Disable override (inherit from Default)" withArrow>
              <ActionIcon
                variant="subtle"
                color="gray"
                size="sm"
                disabled={isLoading}
                onClick={() =>
                  openDisableOverrideDialog({
                    title: "Disable Widget position override?",
                    onConfirm: () => updateProfile({ widget_position: null }),
                  })
                }
              >
                <RotateCcw size={14} style={{ opacity: 0.65 }} />
              </ActionIcon>
            </Tooltip>
          )}
          {widgetPositionInheriting && (
            <Tooltip label={INHERIT_TOOLTIP} withArrow>
              <Info size={14} style={{ opacity: 0.5, flexShrink: 0 }} />
            </Tooltip>
          )}
          <Select
            data={WIDGET_POSITION_OPTIONS}
            value={widgetPosition}
            onChange={handleWidgetPositionChange}
            disabled={isLoading || overlayMode === "never"}
            withCheckIcon={false}
            styles={{
              input: {
                backgroundColor: "var(--bg-elevated)",
                borderColor: "var(--border-default)",
                color: "var(--text-primary)",
                minWidth: 180,
              },
            }}
          />
        </div>
      </div>
      <div className="settings-row">
        <div>
          <p className="settings-label">Output mode</p>
          <p className="settings-description">How to output transcribed text</p>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          {isProfileScope && !outputModeInheriting && (
            <Tooltip label="Disable override (inherit from Default)" withArrow>
              <ActionIcon
                variant="subtle"
                color="gray"
                size="sm"
                disabled={isLoading}
                onClick={() =>
                  openDisableOverrideDialog({
                    title: "Disable Output mode override?",
                    onConfirm: () => updateProfile({ output_mode: null }),
                  })
                }
              >
                <RotateCcw size={14} style={{ opacity: 0.65 }} />
              </ActionIcon>
            </Tooltip>
          )}
          {outputModeInheriting && (
            <Tooltip label={INHERIT_TOOLTIP} withArrow>
              <Info size={14} style={{ opacity: 0.5, flexShrink: 0 }} />
            </Tooltip>
          )}
          <Select
            data={OUTPUT_MODE_OPTIONS}
            value={outputMode}
            onChange={handleOutputModeChange}
            disabled={isLoading}
            withCheckIcon={false}
            styles={{
              input: {
                backgroundColor: "var(--bg-elevated)",
                borderColor: "var(--border-default)",
                color: "var(--text-primary)",
                minWidth: 220,
              },
            }}
          />
        </div>
      </div>
    </>
  );
}
