import { Alert, Button } from "@mantine/core";
import { AlertCircle, RotateCcw } from "lucide-react";
import { useEffect, useState } from "react";
import {
	DEFAULT_HOLD_HOTKEY,
	DEFAULT_PASTE_LAST_HOTKEY,
	DEFAULT_TOGGLE_HOTKEY,
} from "../../lib/hotkeyDefaults";
import {
	useResetHotkeysToDefaults,
	useSettings,
	useUpdateHoldHotkey,
	useUpdatePasteLastHotkey,
	useUpdateToggleHotkey,
} from "../../lib/queries";
import type { HotkeyConfig } from "../../lib/tauri";
import { HotkeyInput } from "../HotkeyInput";

type RecordingInput = "toggle" | "hold" | "paste_last" | null;

export function HotkeySettings() {
	const { data: settings, isLoading } = useSettings();
	const updateToggleHotkey = useUpdateToggleHotkey();
	const updateHoldHotkey = useUpdateHoldHotkey();
	const updatePasteLastHotkey = useUpdatePasteLastHotkey();
	const resetHotkeys = useResetHotkeysToDefaults();

	// Track which input is currently recording (only one at a time)
	const [recordingInput, setRecordingInput] = useState<RecordingInput>(null);

	// Track dismissed error to allow auto-dismiss
	const [dismissedError, setDismissedError] = useState<string | null>(null);

	// Collect any errors from mutations
	const rawError =
		updateToggleHotkey.error ||
		updateHoldHotkey.error ||
		updatePasteLastHotkey.error ||
		resetHotkeys.error;

	const errorMessage =
		rawError instanceof Error
			? rawError.message
			: rawError
				? String(rawError)
				: null;

	// Only show error if not dismissed
	const showError = errorMessage && errorMessage !== dismissedError;

	// Auto-dismiss error after 5 seconds
	useEffect(() => {
		if (!errorMessage || errorMessage === dismissedError) return;

		const timer = setTimeout(() => {
			setDismissedError(errorMessage);
		}, 5000);

		return () => clearTimeout(timer);
	}, [errorMessage, dismissedError]);

	// Reset dismissed error when a new error appears
	useEffect(() => {
		if (errorMessage && errorMessage !== dismissedError) {
			// New error appeared, clear previous dismissed state
			setDismissedError(null);
		}
	}, [errorMessage, dismissedError]);

	const handleToggleHotkeyChange = (config: HotkeyConfig) => {
		updateToggleHotkey.mutate(config);
	};

	const handleHoldHotkeyChange = (config: HotkeyConfig) => {
		updateHoldHotkey.mutate(config);
	};

	const handlePasteLastHotkeyChange = (config: HotkeyConfig) => {
		updatePasteLastHotkey.mutate(config);
	};

	return (
		<>
			{showError && (
				<Alert
					icon={<AlertCircle size={16} />}
					color="red"
					mb="md"
					title="Error"
					withCloseButton
					onClose={() => setDismissedError(errorMessage)}
				>
					{errorMessage}
				</Alert>
			)}
			<HotkeyInput
				label="Toggle Recording"
				description="Press once to start recording, press again to stop"
				value={settings?.toggle_hotkey ?? DEFAULT_TOGGLE_HOTKEY}
				onChange={handleToggleHotkeyChange}
				disabled={isLoading || updateToggleHotkey.isPending}
				isRecording={recordingInput === "toggle"}
				onStartRecording={() => setRecordingInput("toggle")}
				onStopRecording={() => setRecordingInput(null)}
			/>

			<div style={{ marginTop: 20 }}>
				<HotkeyInput
					label="Hold to Record"
					description="Hold to record, release to stop"
					value={settings?.hold_hotkey ?? DEFAULT_HOLD_HOTKEY}
					onChange={handleHoldHotkeyChange}
					disabled={isLoading || updateHoldHotkey.isPending}
					isRecording={recordingInput === "hold"}
					onStartRecording={() => setRecordingInput("hold")}
					onStopRecording={() => setRecordingInput(null)}
				/>
			</div>

			<div style={{ marginTop: 20 }}>
				<HotkeyInput
					label="Output Last Transcription"
					description="Output last transcription based on output setting"
					value={settings?.paste_last_hotkey ?? DEFAULT_PASTE_LAST_HOTKEY}
					onChange={handlePasteLastHotkeyChange}
					disabled={isLoading || updatePasteLastHotkey.isPending}
					isRecording={recordingInput === "paste_last"}
					onStartRecording={() => setRecordingInput("paste_last")}
					onStopRecording={() => setRecordingInput(null)}
				/>
			</div>

			<div
				style={{
					marginTop: 24,
					paddingTop: 16,
					borderTop: "1px solid var(--mantine-color-dark-4)",
					display: "flex",
					alignItems: "center",
					justifyContent: "flex-end",
				}}
			>
				<Button
					variant="light"
					color="gray"
					size="xs"
					leftSection={<RotateCcw size={14} />}
					onClick={() => resetHotkeys.mutate()}
					loading={resetHotkeys.isPending}
					disabled={isLoading}
				>
					Reset to Defaults
				</Button>
			</div>
		</>
	);
}
