import {
  ActionIcon,
  Group,
  NumberInput,
  Slider,
  Switch,
  Tooltip,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { FolderOpen } from "lucide-react";
import { useEffect, useState } from "react";
import {
  useRecordingsStats,
  useSettings,
  useUpdateMaxSavedRecordings,
  useUpdateNoiseGateStrength,
  useUpdateQuietAudioGateEnabled,
  useUpdateQuietAudioMinDurationSecs,
  useUpdateQuietAudioPeakDbfsThreshold,
  useUpdateQuietAudioRmsDbfsThreshold,
} from "../../lib/queries";
import {
  recordingsAPI,
  type RewriteProgramPromptProfile,
} from "../../lib/tauri";
import { DeviceSelector } from "../DeviceSelector";

const GLOBAL_ONLY_TOOLTIP =
  "This setting can only be changed in the Default profile";

export function AudioSettings({
  editingProfileId,
}: {
  editingProfileId?: string;
}) {
  const { data: settings } = useSettings();

  const updateMaxSavedRecordings = useUpdateMaxSavedRecordings();
  const recordingsStats = useRecordingsStats();

  const updateQuietAudioGateEnabled = useUpdateQuietAudioGateEnabled();
  const updateQuietAudioMinDurationSecs = useUpdateQuietAudioMinDurationSecs();
  const updateQuietAudioRmsDbfsThreshold =
    useUpdateQuietAudioRmsDbfsThreshold();
  const updateQuietAudioPeakDbfsThreshold =
    useUpdateQuietAudioPeakDbfsThreshold();
  const updateNoiseGateStrength = useUpdateNoiseGateStrength();

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
    settings?.quiet_audio_rms_dbfs_threshold ?? -60;
  const quietAudioPeakDbfsThreshold =
    settings?.quiet_audio_peak_dbfs_threshold ?? -50;

  const maxSavedRecordings = settings?.max_saved_recordings ?? 1000;

  const handleOpenRecordingsFolder = async () => {
    try {
      await recordingsAPI.openRecordingsFolder();
    } catch (e) {
      notifications.show({
        title: "Recordings",
        message: String(e),
        color: "red",
      });
    }
  };

  const recordingsSummary = (() => {
    const stats = recordingsStats.data;
    if (!stats) return null;
    if (typeof stats.count !== "number" || !Number.isFinite(stats.count))
      return null;
    if (typeof stats.bytes !== "number" || !Number.isFinite(stats.bytes))
      return null;

    const gb = stats.bytes / 1024 ** 3;
    return {
      count: Math.max(0, Math.round(stats.count)),
      gb,
    };
  })();

  const noiseGateStrengthFromSettings = settings?.noise_gate_strength ?? 0;
  const [noiseGateStrengthDraft, setNoiseGateStrengthDraft] = useState<
    number | null
  >(null);

  useEffect(() => {
    // If settings update elsewhere (or after save), drop any draft value.
    setNoiseGateStrengthDraft(null);
  }, [noiseGateStrengthFromSettings]);

  const noiseGateStrength =
    noiseGateStrengthDraft ?? noiseGateStrengthFromSettings;

  const content = (
    <>
      <DeviceSelector />

      <div className="settings-row">
        <div>
          <p className="settings-label">Max recordings to save</p>
          <p className="settings-description">
            Keep at most this many recordings on disk.
            {recordingsStats.isLoading
              ? " (Calculating storage…)"
              : recordingsSummary === null
              ? ""
              : ` (Currently saved ${
                  recordingsSummary.count
                } recordings at ${recordingsSummary.gb.toFixed(2)} GB)`}
          </p>
        </div>
        <Group gap={8} align="center">
          <Tooltip label="Open recordings folder" withArrow position="top">
            <span>
              <ActionIcon
                variant="default"
                size={36}
                onClick={() => {
                  handleOpenRecordingsFolder().catch(console.error);
                }}
                aria-label="Open recordings folder"
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
                <FolderOpen size={14} style={{ opacity: 0.75 }} />
              </ActionIcon>
            </span>
          </Tooltip>
          <NumberInput
            value={maxSavedRecordings}
            onChange={(value) => {
              const next = typeof value === "number" ? value : 1000;
              updateMaxSavedRecordings.mutate(next);
            }}
            min={1}
            max={100000}
            step={100}
            clampBehavior="strict"
            disabled={isProfileScope}
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
            decimalScale={2}
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

      <div className="settings-row">
        <div>
          <p className="settings-label">Noise gate (experimental)</p>
          <p className="settings-description">
            Reduce background noise in recordings. 0 = Off.
          </p>
        </div>
        <div style={{ width: 220 }}>
          <Slider
            value={noiseGateStrength}
            onChange={setNoiseGateStrengthDraft}
            onChangeEnd={(value) => updateNoiseGateStrength.mutate(value)}
            min={0}
            max={100}
            step={1}
            label={(value) => (value === 0 ? "Off" : String(value))}
            color="gray"
            styles={{
              track: { backgroundColor: "var(--bg-elevated)" },
            }}
          />
        </div>
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
