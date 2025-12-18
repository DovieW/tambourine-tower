import { Select, Switch, Tooltip } from "@mantine/core";
import {
	useIsAudioMuteSupported,
	useSettings,
	useUpdateAutoMuteAudio,
	useUpdateOverlayMode,
	useUpdateSoundEnabled,
} from "../../lib/queries";
import type { OverlayMode } from "../../lib/tauri";
import { DeviceSelector } from "../DeviceSelector";

const OVERLAY_MODE_OPTIONS = [
	{ value: "always", label: "Always visible" },
	{ value: "recording_only", label: "Only when recording" },
	{ value: "never", label: "Hidden" },
];

export function AudioSettings() {
	const { data: settings, isLoading } = useSettings();
	const { data: isAudioMuteSupported } = useIsAudioMuteSupported();
	const updateSoundEnabled = useUpdateSoundEnabled();
	const updateAutoMuteAudio = useUpdateAutoMuteAudio();
	const updateOverlayMode = useUpdateOverlayMode();

	const handleSoundToggle = (checked: boolean) => {
		updateSoundEnabled.mutate(checked);
	};

	const handleAutoMuteToggle = (checked: boolean) => {
		updateAutoMuteAudio.mutate(checked);
	};

	const handleOverlayModeChange = (value: string | null) => {
		if (value) {
			updateOverlayMode.mutate(value as OverlayMode);
		}
	};

	return (
		<div className="settings-section animate-in animate-in-delay-2">
			<h3 className="settings-section-title">Audio</h3>
			<div className="settings-card">
				<DeviceSelector />
				<div className="settings-row" style={{ marginTop: 16 }}>
					<div>
						<p className="settings-label">Sound feedback</p>
						<p className="settings-description">
							Play sounds when recording starts and stops
						</p>
					</div>
					<Switch
						checked={settings?.sound_enabled ?? true}
						onChange={(event) => handleSoundToggle(event.currentTarget.checked)}
						disabled={isLoading}
						color="gray"
						size="md"
					/>
				</div>
				<div className="settings-row" style={{ marginTop: 16 }}>
					<div>
						<p className="settings-label">Mute audio during recording</p>
						<p className="settings-description">
							Automatically mute system audio while dictating
						</p>
					</div>
					<Tooltip
						label="Not supported on this platform"
						disabled={isAudioMuteSupported !== false}
						withArrow
					>
						<Switch
							checked={settings?.auto_mute_audio ?? false}
							onChange={(event) =>
								handleAutoMuteToggle(event.currentTarget.checked)
							}
							disabled={isLoading || isAudioMuteSupported === false}
							color="gray"
							size="md"
						/>
					</Tooltip>
				</div>
				<div className="settings-row" style={{ marginTop: 16 }}>
					<div>
						<p className="settings-label">Overlay widget</p>
						<p className="settings-description">
							When to show the on-screen recording widget
						</p>
					</div>
					<Select
						data={OVERLAY_MODE_OPTIONS}
						value={settings?.overlay_mode ?? "always"}
						onChange={handleOverlayModeChange}
						disabled={isLoading}
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
		</div>
	);
}
