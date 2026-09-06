import {
	ActionIcon,
	Alert,
	Button,
	Group,
	Loader,
	Modal,
	Paper,
	Popover,
	SegmentedControl,
	Stack,
	Switch,
	Text,
	Tooltip,
} from "@mantine/core";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
	CircleAlert,
	Ellipsis,
	Mic,
	Pause,
	Play,
	Square,
	X,
} from "lucide-react";
import { useEffect, useState } from "react";
import { formatErrorMessage } from "../lib/formatError";
import { recordingControlsAPI } from "../lib/tauri/commands";
import type { RecordingPreferences } from "../lib/tauri/types";
import { MeetingModelDialog } from "./MeetingModelDialog";

/** Uses the backend pipeline as owner, including recordings started with F3. */
export function RecordingBar() {
	const client = useQueryClient();
	const [computerAudio, setComputerAudio] = useState(false);
	const [optionsOpen, setOptionsOpen] = useState(false);
	const [recoveryOpen, setRecoveryOpen] = useState(false);
	const [modelOpen, setModelOpen] = useState(false);
	const preferences = useQuery({
		queryKey: ["recording-preferences"],
		queryFn: recordingControlsAPI.getPreferences,
	});
	const recordingPreferences = preferences.data ?? {
		mode: "dictation" as const,
		meeting_model: null,
	};
	const savePreferences = useMutation({
		mutationFn: recordingControlsAPI.setPreferences,
		onSuccess: (_, value) => {
			client.setQueryData(["recording-preferences"], value);
			setModelOpen(false);
		},
	});
	const updatePreferences = (patch: Partial<RecordingPreferences>) =>
		savePreferences.mutate({ ...recordingPreferences, ...patch });
	const capability = useQuery({
		queryKey: ["computer-audio-capability"],
		queryFn: recordingControlsAPI.computerAudioAvailable,
		staleTime: 60_000,
	});
	const state = useQuery({
		queryKey: ["home-recording-state"],
		queryFn: recordingControlsAPI.getState,
		refetchInterval: 500,
	});
	const action = useMutation({
		mutationFn: async (operation: "start" | "stop" | "cancel") => {
			if (operation === "start")
				await recordingControlsAPI.start(
					recordingPreferences.mode === "meeting" && computerAudio,
				);
			else await recordingControlsAPI[operation]();
		},
		onSettled: async () => {
			await client.invalidateQueries({ queryKey: ["home-recording-state"] });
		},
	});
	const recording = state.data === "recording";
	const progress = useQuery({
		queryKey: ["recording-seconds"],
		queryFn: recordingControlsAPI.getSeconds,
		refetchInterval: 1000,
		retry: false,
	});
	const canPause = useQuery({
		queryKey: ["recording-can-pause"],
		queryFn: recordingControlsAPI.canPause,
		refetchInterval: 500,
	});
	const recovery = useQuery({
		queryKey: ["recording-recovery"],
		queryFn: recordingControlsAPI.listRecovery,
		refetchInterval: 3000,
	});
	const recover = useMutation({
		mutationFn: recordingControlsAPI.recover,
		onSettled: () =>
			client.invalidateQueries({ queryKey: ["recording-recovery"] }),
	});
	const discard = useMutation({
		mutationFn: recordingControlsAPI.discardRecovery,
		onSettled: () =>
			client.invalidateQueries({ queryKey: ["recording-recovery"] }),
	});
	const paused = useQuery({
		queryKey: ["home-recording-paused"],
		queryFn: recordingControlsAPI.getPaused,
		refetchInterval: 500,
	});
	const pause = useMutation({
		mutationFn: recordingControlsAPI.setPaused,
		onSettled: () =>
			client.invalidateQueries({ queryKey: ["home-recording-paused"] }),
	});
	const cancel = useMutation({
		mutationFn: recordingControlsAPI.cancel,
		onSettled: () =>
			client.invalidateQueries({ queryKey: ["home-recording-state"] }),
	});
	const idle = state.data === "idle" || state.data === "error";
	const error =
		preferences.error ??
		savePreferences.error ??
		progress.error ??
		recover.error ??
		discard.error ??
		recovery.error ??
		pause.error ??
		cancel.error ??
		action.error ??
		state.error;

	const errorMessage = error ? formatErrorMessage(error) : null;
	useEffect(() => {
		if (errorMessage) {
			setOptionsOpen(false);
			setRecoveryOpen(true);
		}
	}, [errorMessage]);
	const savedCount = recovery.data?.length ?? 0;
	const pending = action.isPending || recover.isPending;
	const busy = !idle && !recording;
	const optionsLabel = errorMessage
		? "Recording options: error"
		: savedCount
			? `Recording options: ${savedCount} saved recordings`
			: "Recording options";

	return (
		<Paper
			withBorder
			shadow="md"
			radius="xl"
			p={8}
			role="region"
			aria-label="Recorder"
			style={{
				position: "fixed",
				bottom: 24,
				right: 24,
				zIndex: 100,
				maxWidth: "calc(100vw - 112px)",
			}}
		>
			<Group gap={6} wrap="nowrap">
				{recording ? (
					<>
						<Text
							size="sm"
							aria-label={
								paused.data ? "Recording paused" : "Recording duration"
							}
							style={{
								minWidth: 52,
								textAlign: "center",
								fontVariantNumeric: "tabular-nums",
							}}
						>
							{Math.floor((progress.data ?? 0) / 60)}:
							{String(Math.floor((progress.data ?? 0) % 60)).padStart(2, "0")}
						</Text>
						{canPause.data ? (
							<Tooltip label={paused.data ? "Resume" : "Pause"}>
								<ActionIcon
									size={34}
									variant="subtle"
									aria-label={paused.data ? "Resume" : "Pause"}
									disabled={pause.isPending || pending}
									onClick={() => pause.mutate(!paused.data)}
								>
									{paused.data ? <Play size={17} /> : <Pause size={17} />}
								</ActionIcon>
							</Tooltip>
						) : null}
						<Tooltip label="Stop & transcribe">
							<ActionIcon
								size={34}
								radius="xl"
								variant="filled"
								aria-label="Stop & transcribe"
								disabled={pending}
								onClick={() => action.mutate("stop")}
							>
								<Square size={15} fill="currentColor" />
							</ActionIcon>
						</Tooltip>
					</>
				) : busy || pending ? (
					<Tooltip label={state.data ? "Processing" : "Connecting"}>
						<span
							role="status"
							aria-label={state.data ? "Processing" : "Connecting"}
						>
							<Loader size={16} mx={9} />
						</span>
					</Tooltip>
				) : (
					<ActionIcon
						size={34}
						variant="filled"
						radius="xl"
						aria-label="Record"
						disabled={
							state.isError ||
							!idle ||
							discard.isPending ||
							preferences.isPending ||
							savePreferences.isPending
						}
						onClick={() => action.mutate("start")}
					>
						<Mic size={17} />
					</ActionIcon>
				)}
				{(!idle && state.data) || pending ? (
					<Tooltip label="Cancel">
						<ActionIcon
							size={34}
							variant="subtle"
							color="gray"
							aria-label="Cancel"
							disabled={cancel.isPending}
							onClick={() => cancel.mutate()}
						>
							<X size={17} />
						</ActionIcon>
					</Tooltip>
				) : null}
				<Popover
					opened={optionsOpen}
					onChange={setOptionsOpen}
					position="top-end"
					withArrow
					shadow="md"
					width={260}
					withinPortal
				>
					<Popover.Target>
						<ActionIcon
							size={34}
							variant="subtle"
							radius="xl"
							color={errorMessage ? "red" : savedCount ? "orange" : "gray"}
							aria-label={optionsLabel}
							onClick={() => setOptionsOpen((open) => !open)}
						>
							{errorMessage ? (
								<CircleAlert size={19} />
							) : (
								<Ellipsis size={19} />
							)}
						</ActionIcon>
					</Popover.Target>
					<Popover.Dropdown
						style={{
							maxWidth: "calc(100vw - 32px)",
						}}
					>
						<Stack gap="sm">
							<SegmentedControl
								aria-label="Recording mode"
								size="xs"
								fullWidth
								data={[
									{ value: "dictation", label: "Dictation" },
									{ value: "meeting", label: "Meeting" },
								]}
								value={recordingPreferences.mode}
								disabled={!idle || pending || savePreferences.isPending}
								onChange={(mode) =>
									updatePreferences({
										mode: mode === "meeting" ? "meeting" : "dictation",
									})
								}
							/>
							{recordingPreferences.mode === "meeting" && (
								<>
									<Button
										size="compact-xs"
										variant="subtle"
										onClick={() => {
											setOptionsOpen(false);
											setModelOpen(true);
										}}
										disabled={!idle || pending}
									>
										Meeting model
									</Button>
									<Switch
										label="Computer audio"
										checked={computerAudio}
										onChange={(event) =>
											setComputerAudio(event.currentTarget.checked)
										}
										disabled={!idle || !capability.data || pending}
										description={
											capability.data ? undefined : "Unavailable on this device"
										}
									/>
								</>
							)}
							{savedCount > 0 && (
								<Button
									variant="subtle"
									size="compact-xs"
									onClick={() => {
										setOptionsOpen(false);
										setRecoveryOpen(true);
									}}
								>
									Saved recordings ({savedCount})
								</Button>
							)}
						</Stack>
					</Popover.Dropdown>
				</Popover>
			</Group>
			{modelOpen && (
				<MeetingModelDialog
					preferences={recordingPreferences}
					onClose={() => setModelOpen(false)}
					onSave={(value) => savePreferences.mutate(value)}
					saving={savePreferences.isPending}
				/>
			)}
			<Modal
				opened={recoveryOpen}
				onClose={() => setRecoveryOpen(false)}
				title="Saved recordings"
				centered
			>
				<Stack gap="md">
					{errorMessage && <Alert color="red">{errorMessage}</Alert>}
					<Text size="sm" c="dimmed">
						Audio is saved locally until transcription finishes. Recover or
						discard interrupted recordings here.
					</Text>
					{recovery.data?.map((id, index) => (
						<Stack key={id} gap={4}>
							<Text size="xs">Saved audio {index + 1}</Text>
							<Group gap={6}>
								<Button
									size="compact-xs"
									disabled={!idle || pending || discard.isPending}
									onClick={() => recover.mutate(id)}
								>
									Transcribe
								</Button>
								<Button
									size="compact-xs"
									color="red"
									variant="subtle"
									disabled={!idle || pending || discard.isPending}
									onClick={() => discard.mutate(id)}
								>
									Discard
								</Button>
							</Group>
						</Stack>
					))}
				</Stack>
			</Modal>
		</Paper>
	);
}
