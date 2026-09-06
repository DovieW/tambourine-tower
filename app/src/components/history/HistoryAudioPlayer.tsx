import {
	ActionIcon,
	Button,
	Group,
	Loader,
	Select,
	Slider,
	Stack,
	Text,
} from "@mantine/core";
import { Pause, Play, RotateCcw, RotateCw } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import WaveSurfer from "wavesurfer.js";
import type { RecordingPlayerControls } from "../../lib/useRecordingPlayer";

export function audioTime(seconds: number): string {
	const total = Math.max(0, Math.floor(seconds || 0));
	const hours = Math.floor(total / 3600);
	return `${hours ? `${hours}:` : ""}${hours ? String(Math.floor(total / 60) % 60).padStart(2, "0") : Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

export function HistoryAudioPlayer({
	player,
	recordingId,
}: {
	player: RecordingPlayerControls;
	recordingId: string;
}) {
	const container = useRef<HTMLDivElement>(null);
	const [waveformError, setWaveformError] = useState(false);
	const active = player.id === recordingId;
	const waveform = active ? player.waveform : null;
	const media = player.media;
	useEffect(() => {
		if (!container.current || !media || !waveform) return;
		setWaveformError(false);
		let wave: WaveSurfer | undefined;
		try {
			wave = WaveSurfer.create({
				container: container.current,
				media,
				peaks: [waveform.peaks],
				duration: waveform.duration_seconds,
				height: 64,
				waveColor: "#61756b",
				progressColor: "#19bf65",
				cursorColor: "#19bf65",
				barWidth: 2,
				barGap: 2,
				barRadius: 2,
				normalize: false,
				interact: true,
				dragToSeek: true,
			});
			wave.on("error", () => setWaveformError(true));
		} catch {
			setWaveformError(true);
		}
		return () => wave?.destroy();
	}, [media, waveform]);
	if (!active || player.loading)
		return (
			<Group p="md" gap="xs">
				<Loader size="xs" />
				<Text size="sm" c="dimmed">
					Preparing audio…
				</Text>
			</Group>
		);
	if (player.error)
		return (
			<Group justify="space-between" py="sm">
				<Text size="sm" c="dimmed">
					{player.error}
				</Text>
				<Button
					variant="subtle"
					size="xs"
					onClick={() => void player.prepare(recordingId)}
				>
					Retry audio
				</Button>
			</Group>
		);
	return (
		<Stack gap="xs" className="history-audio-player">
			<div ref={container} aria-hidden="true" />
			{waveformError && (
				<Text size="xs" c="dimmed">
					Waveform unavailable. You can still use the playback controls below.
				</Text>
			)}
			<Slider
				aria-label="Playback position"
				min={0}
				max={Math.max(1, player.duration)}
				step={0.1}
				value={player.position}
				onChange={player.seek}
				label={audioTime}
				size={2}
			/>
			<Group justify="space-between" gap="xs" wrap="wrap">
				<Group gap="xs">
					<ActionIcon
						variant="subtle"
						aria-label="Back 10 seconds"
						onClick={() => player.seek(player.position - 10)}
					>
						<RotateCcw size={17} />
					</ActionIcon>
					<ActionIcon
						variant="light"
						radius="xl"
						size="lg"
						aria-label={player.playing ? "Pause audio" : "Play audio"}
						onClick={() => void player.toggle(recordingId)}
					>
						{player.playing ? <Pause size={18} /> : <Play size={18} />}
					</ActionIcon>
					<ActionIcon
						variant="subtle"
						aria-label="Forward 10 seconds"
						onClick={() => player.seek(player.position + 10)}
					>
						<RotateCw size={17} />
					</ActionIcon>
					<Text
						size="xs"
						c="dimmed"
						style={{ fontVariantNumeric: "tabular-nums" }}
					>
						{audioTime(player.position)} / {audioTime(player.duration)}
					</Text>
				</Group>
				<Select
					aria-label="Playback speed"
					size="xs"
					w={85}
					value={String(player.rate)}
					onChange={(v) => player.setRate(Number(v))}
					data={[0.5, 0.75, 1, 1.25, 1.5, 2].map((r) => ({
						value: String(r),
						label: `${r}×`,
					}))}
					allowDeselect={false}
				/>
			</Group>
		</Stack>
	);
}
