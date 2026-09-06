import {
	ActionIcon,
	Alert,
	Button,
	Group,
	Loader,
	Modal,
	Stack,
	Text,
	Textarea,
	TextInput,
} from "@mantine/core";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
	ArrowDown,
	ArrowUp,
	Copy,
	Expand,
	Pencil,
	Search,
	Undo2,
} from "lucide-react";
import {
	Fragment,
	useEffect,
	useMemo,
	useRef,
	useState,
	useSyncExternalStore,
} from "react";
import { HistoryDocument, transcriptMatches } from "../../lib/history/document";
import { tauriAPI } from "../../lib/tauri";
import type { HistoryDetail } from "../../lib/tauri/types";
import type { RecordingPlayerControls } from "../../lib/useRecordingPlayer";
import { HistoryAudioPlayer } from "./HistoryAudioPlayer";

// Only unsaved sessions survive navigation; no transcripts go to browser storage.
const pendingDocuments = new Map<string, HistoryDocument>();

/** History deletion/retention also releases unsaved, in-memory editor sessions. */
export async function prunePendingHistoryDocuments() {
	await Promise.all(
		[...pendingDocuments].map(async ([id, document]) => {
			try {
				if (!(await tauriAPI.getHistoryDetail(id))) {
					document.discard();
					pendingDocuments.delete(id);
				}
			} catch {
				/* Failed lookup is not proof of deletion: keep the draft. */
			}
		}),
	);
}

export function HistoryReader({
	id,
	recordingId,
	player,
	onCopy,
}: {
	id: string;
	recordingId: string;
	player: RecordingPlayerControls;
	onCopy: (text: string) => void;
}) {
	const query = useQuery({
		queryKey: ["historyDetail", id],
		queryFn: () => tauriAPI.getHistoryDetail(id),
		staleTime: 0,
		refetchInterval: (query) =>
			query.state.data?.entry.status === "in_progress" ? 1000 : false,
	});
	const status = query.data?.entry.status;
	const { prepare, stop } = player;
	useEffect(() => {
		if (!status) return;
		void prepare(recordingId);
		return stop;
	}, [prepare, stop, recordingId, status]);
	if (query.isPending) return <Loader size="sm" m="md" />;
	if (query.error || !query.data)
		return (
			<Text size="sm" c="red" p="md">
				Could not load this recording. Close and reopen it to retry.
			</Text>
		);
	return (
		<HistoryDocumentView
			key={`${id}:${status}`}
			detail={query.data}
			recordingId={recordingId}
			player={player}
			onCopy={onCopy}
		/>
	);
}

function HistoryDocumentView({
	detail,
	recordingId,
	player,
	onCopy,
}: {
	detail: HistoryDetail;
	recordingId: string;
	player: RecordingPlayerControls;
	onCopy: (text: string) => void;
}) {
	const client = useQueryClient();
	const [document] = useState(
		() =>
			pendingDocuments.get(detail.entry.id) ??
			new HistoryDocument(detail, tauriAPI.saveHistoryEdit, () => {
				void tauriAPI.emitHistoryChanged().catch(() => {});
				void client.invalidateQueries({ queryKey: ["historyPage"] });
				void client.invalidateQueries({ queryKey: ["historyAll"] });
				void client.invalidateQueries({
					queryKey: ["historyDetail", detail.entry.id],
				});
			}),
	);
	const draft = useSyncExternalStore(
		document.subscribe,
		document.snapshot,
		document.snapshot,
	);
	const [full, setFull] = useState(false);
	const [editing, setEditing] = useState(false);
	const [search, setSearch] = useState("");
	const [match, setMatch] = useState(0);
	const transcript = useRef<HTMLDivElement>(null);
	const matches = useMemo(
		() => transcriptMatches(draft.text, search),
		[draft.text, search],
	);
	const currentMatch = matches.length ? match % matches.length : 0;
	useEffect(() => {
		if (!matches[currentMatch]) return;
		transcript.current
			?.querySelector('[data-current="true"]')
			?.scrollIntoView({ block: "nearest" });
	}, [currentMatch, matches]);
	useEffect(
		() => () => {
			if (document.dirty) {
				pendingDocuments.set(detail.entry.id, document);
				void document.flush().then((saved) => {
					if (saved) pendingDocuments.delete(detail.entry.id);
				});
			}
		},
		[document, detail.entry.id],
	);
	const close = async () => {
		player.stop();
		if (await document.flush()) {
			setFull(false);
			setEditing(false);
		}
	};
	const text = matches.length ? (
		<>
			{matches.map((range, index) => (
				<Fragment key={range.start}>
					{draft.text.slice(
						index ? (matches[index - 1]?.end ?? 0) : 0,
						range.start,
					)}
					<mark
						data-current={index === currentMatch}
						style={{
							background: index === currentMatch ? "#19bf65" : "#cab858",
							color: "#101510",
						}}
					>
						{draft.text.slice(range.start, range.end)}
					</mark>
				</Fragment>
			))}
			{draft.text.slice(matches.at(-1)?.end ?? 0)}
		</>
	) : (
		draft.text
	);
	const playerView = (
		<HistoryAudioPlayer player={player} recordingId={recordingId} />
	);
	const title =
		draft.title ||
		(detail.entry.recording_mode === "meeting" ? "Meeting" : "Voice recording");
	return (
		<>
			{!full && (
				<Stack gap="md" p="md" pt={0}>
					{playerView}
					<section
						className="history-transcript history-transcript-inline"
						// biome-ignore lint/a11y/noNoninteractiveTabindex: keyboard scrolling
						tabIndex={0}
						aria-label="Transcript"
					>
						{draft.text ||
							detail.entry.error_message ||
							"No transcript was produced."}
					</section>
					<Group justify="flex-end">
						<Button
							variant="subtle"
							size="xs"
							leftSection={<Expand size={15} />}
							onClick={() => setFull(true)}
						>
							Open full view
						</Button>
					</Group>
				</Stack>
			)}
			<Modal
				opened={full}
				onClose={() => void close()}
				title={title}
				size="calc(100vw - 48px)"
				yOffset={24}
				padding="lg"
				styles={{
					content: {
						height: "calc(100dvh - 48px)",
						display: "flex",
						flexDirection: "column",
					},
					body: {
						flex: 1,
						minHeight: 0,
						display: "flex",
						flexDirection: "column",
						overflow: "hidden",
					},
					header: { flexShrink: 0 },
				}}
			>
				<Stack gap="sm" className="history-reader-header">
					<Group justify="space-between" align="center">
						<TextInput
							aria-label="Recording title"
							placeholder="Untitled recording"
							value={draft.title}
							maxLength={200}
							onChange={(event) =>
								document.change({ title: event.currentTarget.value })
							}
							style={{ flex: 1 }}
							disabled={
								detail.entry.status === "in_progress" ||
								Boolean(detail.edit_error)
							}
						/>
						<Text
							size="xs"
							c={draft.status === "error" ? "red" : "dimmed"}
							aria-live="polite"
						>
							{draft.status === "saving" || draft.status === "pending"
								? "Saving…"
								: draft.status === "error"
									? "Not saved"
									: "Saved"}
						</Text>
						<Button
							size="xs"
							variant="subtle"
							leftSection={<Copy size={15} />}
							onClick={() => onCopy(draft.text)}
						>
							Copy
						</Button>
						<Button
							size="xs"
							variant={editing ? "light" : "subtle"}
							leftSection={<Pencil size={15} />}
							disabled={
								detail.entry.status === "in_progress" ||
								Boolean(detail.edit_error)
							}
							onClick={async () => {
								if (!editing || (await document.flush())) setEditing(!editing);
							}}
						>
							{editing ? "Done" : "Edit"}
						</Button>
					</Group>
					{playerView}
					<Group gap="xs">
						<TextInput
							aria-label="Search transcript"
							placeholder="Find in transcript"
							leftSection={<Search size={15} />}
							value={search}
							disabled={editing}
							onChange={(event) => {
								setSearch(event.currentTarget.value);
								setMatch(0);
							}}
							onKeyDown={(event) => {
								if (event.key === "Enter" && matches.length) {
									event.preventDefault();
									setMatch(
										(currentMatch +
											(event.shiftKey ? -1 : 1) +
											matches.length) %
											matches.length,
									);
								}
							}}
							style={{ flex: 1 }}
							size="xs"
						/>
						<Text size="xs" c="dimmed" aria-live="polite">
							{search
								? `${matches.length ? currentMatch + 1 : 0} / ${matches.length}${matches.length === 10000 ? "+" : ""}`
								: ""}
						</Text>
						<ActionIcon
							aria-label="Previous match"
							variant="subtle"
							disabled={!matches.length || editing}
							onClick={() =>
								setMatch((currentMatch - 1 + matches.length) % matches.length)
							}
						>
							<ArrowUp size={16} />
						</ActionIcon>
						<ActionIcon
							aria-label="Next match"
							variant="subtle"
							disabled={!matches.length || editing}
							onClick={() => setMatch((currentMatch + 1) % matches.length)}
						>
							<ArrowDown size={16} />
						</ActionIcon>
						{editing && (
							<Button
								size="xs"
								variant="subtle"
								leftSection={<Undo2 size={14} />}
								disabled={draft.text === detail.original_text}
								onClick={() => void document.restoreOriginal()}
							>
								Restore original
							</Button>
						)}
					</Group>
					{draft.error && (
						<Alert color="red" title="Your draft is still here">
							<Text size="sm">{draft.error}</Text>
							<Group mt="xs">
								{draft.error.includes("HISTORY_EDIT_CONFLICT") && (
									<Button
										size="xs"
										variant="light"
										onClick={() =>
											void document.resolveConflict(() =>
												tauriAPI.getHistoryDetail(detail.entry.id),
											)
										}
									>
										Save my draft over newer corrections
									</Button>
								)}
								<Button
									size="xs"
									variant="light"
									onClick={() => void document.flush()}
								>
									Retry save
								</Button>
								<Button
									size="xs"
									variant="subtle"
									onClick={() => onCopy(draft.text)}
								>
									Copy draft
								</Button>
							</Group>
						</Alert>
					)}
				</Stack>
				{editing ? (
					<Textarea
						aria-label="Edit transcript"
						value={draft.text}
						onChange={(event) =>
							document.change({ text: event.currentTarget.value })
						}
						className="history-transcript-editor"
						styles={{
							root: { flex: 1, minHeight: 0, marginTop: 16 },
							wrapper: { height: "100%" },
							input: { height: "100%", resize: "none", lineHeight: 1.8 },
						}}
					/>
				) : (
					<section
						ref={transcript}
						// biome-ignore lint/a11y/noNoninteractiveTabindex: keyboard scrolling
						tabIndex={0}
						aria-label="Full transcript"
						className="history-transcript history-transcript-full"
					>
						{text ||
							detail.entry.error_message ||
							"No transcript was produced."}
					</section>
				)}
			</Modal>
		</>
	);
}
