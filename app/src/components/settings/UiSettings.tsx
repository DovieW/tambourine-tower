import {
  ActionIcon,
  Button,
  Group,
  Modal,
  Select,
  Switch,
  Tooltip,
} from "@mantine/core";
import { Info, RefreshCcw, RotateCcw } from "lucide-react";
import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  useIsAudioMuteSupported,
  useSettings,
  useUpdateOutputMode,
  useUpdateOverlayMode,
  useUpdatePlayingAudioHandling,
  useUpdateRewriteProgramPromptProfiles,
  useUpdateSoundEnabled,
  useUpdateWidgetPosition,
} from "../../lib/queries";
import type {
  OutputMode,
  OverlayMode,
  PlayingAudioHandling,
  RewriteProgramPromptProfile,
  WidgetPosition,
} from "../../lib/tauri";

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

const PLAYING_AUDIO_HANDLING_OPTIONS: Array<{
  value: PlayingAudioHandling;
  label: string;
}> = [
  // Keep a "None" option so existing users who had auto-mute disabled
  // don't suddenly start muting/pausing after this UI change.
  { value: "none", label: "None" },
  { value: "mute", label: "Mute" },
  { value: "pause", label: "Pause" },
  { value: "mute_and_pause", label: "Mute and Pause" },
];

function getProfileValue<T>(
  profileValue: T | null | undefined,
  globalValue: T
): T {
  return profileValue ?? globalValue;
}

export function UiSettings({
  editingProfileId,
}: {
  editingProfileId?: string;
}) {
  const { data: settings, isLoading } = useSettings();
  const { data: isAudioMuteSupported } = useIsAudioMuteSupported();
  const updateSoundEnabled = useUpdateSoundEnabled();
  const updatePlayingAudioHandling = useUpdatePlayingAudioHandling();
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

  const globalPlayingAudioHandling: PlayingAudioHandling =
    settings?.playing_audio_handling ?? "mute";
  const playingAudioHandling = isProfileScope
    ? getProfileValue(
        profile?.playing_audio_handling,
        globalPlayingAudioHandling
      )
    : globalPlayingAudioHandling;
  const playingAudioHandlingInheriting =
    isProfileScope && isInheriting(profile?.playing_audio_handling);

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

  const handlePlayingAudioHandlingChange = (value: string | null) => {
    if (!value) return;
    const next = value as PlayingAudioHandling;
    if (isProfileScope) {
      updateProfile({ playing_audio_handling: next });
      return;
    }
    updatePlayingAudioHandling.mutate(next);
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

  const handleSnapWidgetPosition = async () => {
    // Reposition the overlay window back to the selected preset.
    // This is useful if the user has dragged the overlay away.
    await invoke("set_widget_position", { position: widgetPosition });
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
          <p className="settings-label">Playing audio handling</p>
          <p className="settings-description">
            Mute and/or pause playing audio while recording
          </p>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          {isProfileScope && !playingAudioHandlingInheriting && (
            <Tooltip label="Disable override (inherit from Default)" withArrow>
              <ActionIcon
                variant="subtle"
                color="gray"
                size="sm"
                disabled={isLoading}
                onClick={() =>
                  openDisableOverrideDialog({
                    title: "Disable Playing audio handling override?",
                    onConfirm: () =>
                      updateProfile({ playing_audio_handling: null }),
                  })
                }
              >
                <RotateCcw size={14} style={{ opacity: 0.65 }} />
              </ActionIcon>
            </Tooltip>
          )}
          {playingAudioHandlingInheriting && (
            <Tooltip label={INHERIT_TOOLTIP} withArrow>
              <Info size={14} style={{ opacity: 0.5, flexShrink: 0 }} />
            </Tooltip>
          )}
          <Tooltip
            label="Mute not supported on this platform"
            disabled={isAudioMuteSupported !== false}
            withArrow
          >
            <Select
              data={PLAYING_AUDIO_HANDLING_OPTIONS.map((o) => ({
                ...o,
                disabled:
                  isAudioMuteSupported === false &&
                  (o.value === "mute" || o.value === "mute_and_pause"),
              }))}
              value={playingAudioHandling}
              onChange={handlePlayingAudioHandlingChange}
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
          <Tooltip
            label="Snap overlay back to this position"
            withArrow
            position="top"
          >
            <span>
              <ActionIcon
                variant="default"
                size={36}
                disabled={isLoading || overlayMode === "never"}
                onClick={() => {
                  handleSnapWidgetPosition().catch(console.error);
                }}
                aria-label="Snap overlay position"
                styles={{
                  root: {
                    backgroundColor: "var(--bg-elevated)",
                    borderColor: "var(--border-default)",
                    color: "var(--text-primary)",
                    height: 36,
                    width: 36,
                  },
                }}
              >
                <RefreshCcw size={14} style={{ opacity: 0.75 }} />
              </ActionIcon>
            </span>
          </Tooltip>
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
