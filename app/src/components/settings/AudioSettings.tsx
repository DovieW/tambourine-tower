import { Group, NumberInput, Switch, Tooltip } from "@mantine/core";
import {
  useSettings,
  useUpdateQuietAudioGateEnabled,
  useUpdateQuietAudioMinDurationSecs,
  useUpdateQuietAudioPeakDbfsThreshold,
  useUpdateQuietAudioRmsDbfsThreshold,
} from "../../lib/queries";
import type { RewriteProgramPromptProfile } from "../../lib/tauri";
import { DeviceSelector } from "../DeviceSelector";

const GLOBAL_ONLY_TOOLTIP =
  "This setting can only be changed in the Default profile";

export function AudioSettings({
  editingProfileId,
}: {
  editingProfileId?: string;
}) {
  const { data: settings } = useSettings();

  const updateQuietAudioGateEnabled = useUpdateQuietAudioGateEnabled();
  const updateQuietAudioMinDurationSecs = useUpdateQuietAudioMinDurationSecs();
  const updateQuietAudioRmsDbfsThreshold =
    useUpdateQuietAudioRmsDbfsThreshold();
  const updateQuietAudioPeakDbfsThreshold =
    useUpdateQuietAudioPeakDbfsThreshold();

  const profiles = settings?.rewrite_program_prompt_profiles ?? [];
  const profile: RewriteProgramPromptProfile | null =
    editingProfileId && editingProfileId !== "default"
      ? profiles.find((p) => p.id === editingProfileId) ?? null
      : null;

  const isProfileScope = profile !== null;

  const quietAudioGateEnabled = settings?.quiet_audio_gate_enabled ?? true;
  const quietAudioMinDurationSecs =
    settings?.quiet_audio_min_duration_secs ?? 0.15;
  const quietAudioRmsDbfsThreshold =
    settings?.quiet_audio_rms_dbfs_threshold ?? -50;
  const quietAudioPeakDbfsThreshold =
    settings?.quiet_audio_peak_dbfs_threshold ?? -40;

  const content = (
    <>
      <DeviceSelector />

      <div className="settings-row">
        <div>
          <p className="settings-label">Hallucination protection</p>
          <p className="settings-description">
            Skip transcription when the recording is basically quiet
          </p>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <Switch
            checked={quietAudioGateEnabled}
            onChange={(event) =>
              updateQuietAudioGateEnabled.mutate(event.currentTarget.checked)
            }
            disabled={isProfileScope}
            color="gray"
            size="md"
          />
        </div>
      </div>

      <div className="settings-row">
        <div>
          <p className="settings-label">Quiet minimum duration</p>
          <p className="settings-description">
            Treat very short recordings as quiet (seconds)
          </p>
        </div>
        <Group gap={8} align="center">
          <NumberInput
            value={quietAudioMinDurationSecs}
            onChange={(value) => {
              const next = typeof value === "number" ? value : 0;
              updateQuietAudioMinDurationSecs.mutate(next);
            }}
            min={0}
            max={5}
            step={0.05}
            precision={2}
            disabled={isProfileScope || !quietAudioGateEnabled}
            styles={{
              input: {
                backgroundColor: "var(--bg-elevated)",
                borderColor: "var(--border-default)",
                color: "var(--text-primary)",
                width: 140,
              },
            }}
          />
        </Group>
      </div>

      <div className="settings-row">
        <div>
          <p className="settings-label">Quiet RMS threshold</p>
          <p className="settings-description">
            Average level below this is considered quiet (dBFS)
          </p>
        </div>
        <Group gap={8} align="center">
          <NumberInput
            value={quietAudioRmsDbfsThreshold}
            onChange={(value) => {
              const next = typeof value === "number" ? value : -50;
              updateQuietAudioRmsDbfsThreshold.mutate(next);
            }}
            min={-120}
            max={0}
            step={1}
            disabled={isProfileScope || !quietAudioGateEnabled}
            styles={{
              input: {
                backgroundColor: "var(--bg-elevated)",
                borderColor: "var(--border-default)",
                color: "var(--text-primary)",
                width: 140,
              },
            }}
          />
        </Group>
      </div>

      <div className="settings-row">
        <div>
          <p className="settings-label">Quiet peak threshold</p>
          <p className="settings-description">
            Peak level below this is considered quiet (dBFS)
          </p>
        </div>
        <Group gap={8} align="center">
          <NumberInput
            value={quietAudioPeakDbfsThreshold}
            onChange={(value) => {
              const next = typeof value === "number" ? value : -40;
              updateQuietAudioPeakDbfsThreshold.mutate(next);
            }}
            min={-120}
            max={0}
            step={1}
            disabled={isProfileScope || !quietAudioGateEnabled}
            styles={{
              input: {
                backgroundColor: "var(--bg-elevated)",
                borderColor: "var(--border-default)",
                color: "var(--text-primary)",
                width: 140,
              },
            }}
          />
        </Group>
      </div>
    </>
  );

  if (isProfileScope) {
    return (
      <Tooltip label={GLOBAL_ONLY_TOOLTIP} withArrow position="top-start">
        <div style={{ opacity: 0.5, cursor: "not-allowed" }}>
          <div style={{ pointerEvents: "none" }}>{content}</div>
        </div>
      </Tooltip>
    );
  }

  return content;
}
