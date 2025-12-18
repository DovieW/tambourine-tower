import { Select, Switch, Tooltip } from "@mantine/core";
import {
	useIsAudioMuteSupported,
	useSettings,
	useUpdateAutoMuteAudio,
	useUpdateOutputMode,
	useUpdateOverlayMode,
	useUpdateSoundEnabled,
	useUpdateWidgetPosition,
} from "../../lib/queries";
import type { OutputMode, OverlayMode, WidgetPosition } from "../../lib/tauri";
import { DeviceSelector } from "../DeviceSelector";

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
	{ value: "auto_paste", label: "Auto-paste (Ctrl+V / Cmd+V)" },
	{ value: "clipboard", label: "Copy to clipboard only" },
	{ value: "keystrokes", label: "Type as keystrokes" },
];

export function AudioSettings() {
	const { data: settings, isLoading } = useSettings();
	const { data: isAudioMuteSupported } = useIsAudioMuteSupported();
	const updateSoundEnabled = useUpdateSoundEnabled();
	const updateAutoMuteAudio = useUpdateAutoMuteAudio();
	const updateOverlayMode = useUpdateOverlayMode();
	const updateWidgetPosition = useUpdateWidgetPosition();
	const updateOutputMode = useUpdateOutputMode();

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

	const handleWidgetPositionChange = (value: string | null) => {
		if (value) {
			updateWidgetPosition.mutate(value as WidgetPosition);
		}
	};

	const handleOutputModeChange = (value: string | null) => {
		if (value) {
			updateOutputMode.mutate(value as OutputMode);
		}
	};

	return (
		<>
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
			<div className="settings-row" style={{ marginTop: 16 }}>
				<div>
					<p className="settings-label">Widget position</p>
					<p className="settings-description">
						Default position of the overlay widget on screen
					</p>
				</div>
				<Select
					data={WIDGET_POSITION_OPTIONS}
					value={settings?.widget_position ?? "bottom-right"}
					onChange={handleWidgetPositionChange}
					disabled={isLoading || settings?.overlay_mode === "never"}
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
			<div className="settings-row" style={{ marginTop: 16 }}>
				<div>
					<p className="settings-label">Output mode</p>
					<p className="settings-description">How to output transcribed text</p>
				</div>
				<Select
					data={OUTPUT_MODE_OPTIONS}
					value={settings?.output_mode ?? "auto_paste"}
					onChange={handleOutputModeChange}
					disabled={isLoading}
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
		</>
	);
}
