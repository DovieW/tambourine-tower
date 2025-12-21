import { Loader, Select } from "@mantine/core";
import { useEffect, useState } from "react";
import { useSettings, useUpdateSelectedMic } from "../lib/queries";

interface AudioDevice {
  deviceId: string;
  label: string;
}

export function DeviceSelector() {
  const { data: settings, isLoading: settingsLoading } = useSettings();
  const updateSelectedMic = useUpdateSelectedMic();
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    async function loadDevices() {
      try {
        // Request permission first (needed to get device labels)
        await navigator.mediaDevices.getUserMedia({ audio: true });

        const allDevices = await navigator.mediaDevices.enumerateDevices();
        const audioInputs = allDevices
          .filter((device) => device.kind === "audioinput")
          .map((device) => ({
            deviceId: device.deviceId,
            label: device.label || `Microphone ${device.deviceId.slice(0, 8)}`,
          }));

        setDevices(audioInputs);
        setError(null);
      } catch (err) {
        setError("Could not access microphones. Please grant permission.");
        console.error("Failed to enumerate devices:", err);
      } finally {
        setIsLoading(false);
      }
    }

    loadDevices();

    // Listen for device changes
    const handleDeviceChange = () => {
      loadDevices();
    };
    navigator.mediaDevices.addEventListener("devicechange", handleDeviceChange);

    return () => {
      navigator.mediaDevices.removeEventListener(
        "devicechange",
        handleDeviceChange
      );
    };
  }, []);

  const handleChange = (value: string | null) => {
    // null or empty string means "default"
    const micId = value === "" || value === "default" ? null : value;
    updateSelectedMic.mutate(micId);
  };

  const selectData = [
    { value: "default", label: "System Default" },
    ...devices
      .filter((device) => device.deviceId !== "default")
      .map((device) => ({
        value: device.deviceId,
        label: device.label,
      })),
  ];

  const disabled = isLoading || settingsLoading || Boolean(error);
  const description = "Select which microphone to use for dictation";

  // If settings already point to a specific mic id, ensure it exists in the Select
  // options even before enumeration completes, so the control doesn't appear blank.
  const selectedMicId = settings?.selected_mic_id;
  if (
    selectedMicId &&
    selectedMicId !== "default" &&
    !selectData.some((d) => d.value === selectedMicId)
  ) {
    selectData.splice(1, 0, {
      value: selectedMicId,
      label: "Selected microphone",
    });
  }

  return (
    <div className="settings-row">
      <div>
        <p className="settings-label">Microphone</p>
        <p
          className="settings-description"
          style={error ? { color: "#ef4444" } : undefined}
        >
          {error ?? description}
        </p>
      </div>
      <div style={{ minWidth: 240 }}>
        <Select
          data={selectData}
          value={settings?.selected_mic_id ?? "default"}
          onChange={handleChange}
          allowDeselect={false}
          disabled={disabled}
          rightSection={
            isLoading || settingsLoading ? (
              <Loader size={14} color="gray" />
            ) : undefined
          }
          rightSectionPointerEvents="none"
          className="device-selector"
          withCheckIcon={false}
          styles={{
            input: {
              backgroundColor: "var(--bg-elevated)",
              borderColor: "var(--border-default)",
              color: "var(--text-primary)",
            },
          }}
        />
      </div>
    </div>
  );
}
