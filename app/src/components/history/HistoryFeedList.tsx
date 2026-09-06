import {
	ActionIcon,
	Badge,
	Group,
	Loader,
	Menu,
	Paper,
	Stack,
	Text,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import {
	Check,
	ChevronDown,
	ChevronRight,
	Copy,
	FileText,
	MessageSquare,
	MoreHorizontal,
	RotateCcw,
	Trash2,
} from "lucide-react";
import { useEffect, useState } from "react";
import type {
	GroupedHistoryViewModel,
	HistoryFeedEmptyState,
} from "../../lib/history/readModel";
import { tauriAPI } from "../../lib/tauri";
import type { RecordingPlayerControls } from "../../lib/useRecordingPlayer";
import { audioTime } from "./HistoryAudioPlayer";
import { HistoryReader } from "./HistoryReader";

export function HistoryFeedList({
	isInitialLoading,
	hasError,
	emptyState,
	groupedHistory,
	copiedEntryId,
	onCopyEntry,
	onRetryEntry,
	isRetryPending,
	retryPendingEntryId,
	recordingExistsById,
	player,
	requestLogIds,
	onJumpToLog,
	onDeleteEntry,
	isDeleteDisabled,
}: {
	isInitialLoading: boolean;
	hasError: boolean;
	emptyState: HistoryFeedEmptyState | null;
	groupedHistory: GroupedHistoryViewModel[];
	copiedEntryId: string | null;
	onCopyEntry: (entryId: string, text: string | null | undefined) => void;
	onRetryEntry: (entryId: string) => void;
	isRetryPending: boolean;
	retryPendingEntryId?: string;
	recordingExistsById: Map<string, { exists: boolean; checkedAt: number }>;
	player: RecordingPlayerControls;
	requestLogIds: Set<string>;
	onJumpToLog?: (logId: string) => void;
	onDeleteEntry: (entryId: string) => void;
	isDeleteDisabled: boolean;
}) {
	const [expanded, setExpanded] = useState<string | null>(null);
	const [copying, setCopying] = useState<string | null>(null);
	const visible = groupedHistory.some((g) =>
		g.items.some((entry) => entry.id === expanded),
	);
	useEffect(() => {
		if (!visible) {
			setExpanded(null);
			player.stop();
		}
	}, [visible, player.stop]);
	const copy = async (id: string) => {
		setCopying(id);
		try {
			const detail = await tauriAPI.getHistoryDetail(id);
			if (detail)
				onCopyEntry(id, detail.entry.text || detail.entry.error_message);
		} catch {
			notifications.show({
				color: "red",
				title: "Copy",
				message: "Could not load the complete transcript. Please try again.",
			});
		} finally {
			setCopying(null);
		}
	};
	if (isInitialLoading)
		return (
			<div className="empty-state">
				<Loader size="sm" />
				<p className="empty-state-text">Loading history…</p>
			</div>
		);
	if (hasError)
		return (
			<div className="empty-state">
				<Text c="red">Failed to load history</Text>
			</div>
		);
	if (emptyState)
		return (
			<div className="empty-state">
				<MessageSquare className="empty-state-icon" />
				<h4 className="empty-state-title">{emptyState.title}</h4>
				<p className="empty-state-text">{emptyState.message}</p>
			</div>
		);
	return (
		<Stack gap="lg">
			{groupedHistory.map((group) => (
				<section key={group.date} aria-label={group.date}>
					<Text size="xs" c="dimmed" fw={600} mb="xs">
						{group.date}
					</Text>
					<Stack gap="xs">
						{group.items.map((entry) => {
							const open = expanded === entry.id;
							const recordingId = entry.recordingRequestId ?? entry.id;
							const missing =
								recordingExistsById.get(recordingId)?.exists === false;
							const busy = entry.contentKind === "in_progress";
							return (
								<Paper
									key={entry.id}
									withBorder
									radius="md"
									className="history-card"
									onClick={(event) => {
										if (
											!event.currentTarget.contains(event.target as Node) ||
											(event.target as HTMLElement).closest(
												"button, a, input, textarea, [role=menuitem], [data-history-detail]",
											) ||
											window.getSelection()?.isCollapsed === false
										)
											return;
										player.stop();
										setExpanded(open ? null : entry.id);
									}}
								>
									<Group gap="xs" wrap="nowrap" align="flex-start" p="md">
										<Stack gap={6} style={{ flex: 1, minWidth: 0 }}>
											<button
												type="button"
												className="history-card-toggle"
												aria-expanded={open}
												aria-controls={`history-detail-${entry.id}`}
												onClick={() => {
													player.stop();
													setExpanded(open ? null : entry.id);
												}}
											>
												{open ? (
													<ChevronDown size={16} />
												) : (
													<ChevronRight size={16} />
												)}
												<span>{entry.title || "Voice recording"}</span>
												<Text component="span" size="xs" c="dimmed">
													{entry.timestampLabel}
												</Text>
												{entry.durationSeconds != null && (
													<Text component="span" size="xs" c="dimmed">
														{audioTime(entry.durationSeconds)}
													</Text>
												)}
											</button>
											<Group gap={6}>
												{busy ? (
													<Badge
														size="xs"
														variant="light"
														leftSection={<Loader size={10} />}
													>
														Transcribing
													</Badge>
												) : entry.contentKind === "error" ? (
													<Badge size="xs" variant="light" color="red">
														Failed
													</Badge>
												) : (
													<Badge size="xs" variant="light" color="gray">
														Saved
													</Badge>
												)}
												{entry.profilePresetLabel && (
													<Text size="xs" c="dimmed">
														{entry.profilePresetLabel}
													</Text>
												)}
											</Group>
											<Text
												size="sm"
												c="dimmed"
												lineClamp={2}
												className="history-preview"
											>
												{entry.displayText}
											</Text>
										</Stack>
										<Group gap={4} wrap="nowrap">
											<ActionIcon
												aria-label="Copy transcript"
												variant="subtle"
												disabled={!entry.hasCopyValue}
												loading={copying === entry.id}
												onClick={() => void copy(entry.id)}
											>
												{copiedEntryId === entry.id ? (
													<Check size={17} />
												) : (
													<Copy size={17} />
												)}
											</ActionIcon>
											<Menu position="bottom-end" withinPortal>
												<Menu.Target>
													<ActionIcon
														aria-label="Recording actions"
														variant="subtle"
													>
														<MoreHorizontal size={18} />
													</ActionIcon>
												</Menu.Target>
												<Menu.Dropdown>
													<Menu.Item
														leftSection={<RotateCcw size={15} />}
														disabled={missing || busy || isRetryPending}
														onClick={() => onRetryEntry(entry.id)}
													>
														{isRetryPending && retryPendingEntryId === entry.id
															? "Rerunning…"
															: "Rerun as new result"}
													</Menu.Item>
													{onJumpToLog && requestLogIds.has(entry.id) && (
														<Menu.Item
															leftSection={<FileText size={15} />}
															onClick={() => {
																player.stop();
																onJumpToLog(entry.id);
															}}
														>
															View request log
														</Menu.Item>
													)}
													<Menu.Divider />
													<Menu.Item
														color="red"
														leftSection={<Trash2 size={15} />}
														disabled={isDeleteDisabled || busy}
														onClick={() => {
															player.stop();
															onDeleteEntry(entry.id);
														}}
													>
														Delete
													</Menu.Item>
												</Menu.Dropdown>
											</Menu>
										</Group>
									</Group>
									{open && (
										<div id={`history-detail-${entry.id}`} data-history-detail>
											<HistoryReader
												id={entry.id}
												recordingId={recordingId}
												player={player}
												onCopy={(text) => onCopyEntry(entry.id, text)}
											/>
										</div>
									)}
								</Paper>
							);
						})}
					</Stack>
				</section>
			))}
		</Stack>
	);
}
