import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { RecordingPlayback } from "./history/playback";
import { recordingsAPI } from "./tauri";

export type RecordingPlayerControls = ReturnType<typeof useRecordingPlayer>;

/** One media element for the whole History view, including its reader modal. */
export function useRecordingPlayer(
	options: { onError?: (message: string) => void } = {},
) {
	const onError = useRef(options.onError);
	onError.current = options.onError;
	const [controller] = useState(
		() =>
			new RecordingPlayback({
				createAudio: () => new Audio(),
				getUrl: (requestId) =>
					recordingsAPI.getRecordingAssetUrl({ requestId }),
				getWaveform: (requestId) =>
					recordingsAPI.getRecordingWaveform({ requestId }),
				onError: (message) => onError.current?.(message),
			}),
	);
	const state = useSyncExternalStore(
		controller.subscribe,
		controller.snapshot,
		controller.snapshot,
	);
	useEffect(() => () => controller.dispose(), [controller]);
	return {
		...state,
		media: controller.media,
		prepare: controller.prepare,
		toggle: controller.toggle,
		seek: controller.seek,
		setRate: controller.setRate,
		stop: controller.stop,
		isPlaying: (id: string) => state.id === id && state.playing,
		isLoading: (id: string) => state.id === id && state.loading,
	};
}
