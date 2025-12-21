import {
  ActionIcon,
  Button,
  Group,
  Loader,
  Modal,
  Text,
  TextInput,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useClipboard, useDisclosure } from "@mantine/hooks";
import { useQueryClient } from "@tanstack/react-query";
import { format, isToday, isYesterday } from "date-fns";
import {
  ChevronLeft,
  ChevronRight,
  ChevronsLeft,
  ChevronsRight,
  Copy,
  MessageSquare,
  RotateCcw,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import {
  useClearHistory,
  useDeleteHistoryEntry,
  useHistory,
  useRetryTranscription,
} from "../lib/queries";
import { tauriAPI } from "../lib/tauri";

const HISTORY_PAGE_SIZE = 25;

function formatTime(timestamp: string): string {
  return format(new Date(timestamp), "h:mm a");
}

function formatDate(timestamp: string): string {
  const date = new Date(timestamp);
  if (isToday(date)) return "Today";
  if (isYesterday(date)) return "Yesterday";
  return format(date, "MMM d");
}

interface GroupedHistory {
  date: string;
  items: Array<{
    id: string;
    text: string;
    timestamp: string;
    status?: "in_progress" | "success" | "error";
    error_message?: string | null;
  }>;
}

function groupHistoryByDate(
  history: Array<{
    id: string;
    text: string;
    timestamp: string;
    status?: "in_progress" | "success" | "error";
    error_message?: string | null;
  }>
): GroupedHistory[] {
  const groups: Record<string, GroupedHistory> = {};

  for (const item of history) {
    const dateKey = formatDate(item.timestamp);
    if (!groups[dateKey]) {
      groups[dateKey] = { date: dateKey, items: [] };
    }
    groups[dateKey].items.push(item);
  }

  return Object.values(groups);
}

export function HistoryFeed() {
  const queryClient = useQueryClient();
  const { data: history, isLoading, error } = useHistory();
  const deleteEntry = useDeleteHistoryEntry();
  const clearHistory = useClearHistory();
  const retryMutation = useRetryTranscription();
  const clipboard = useClipboard();
  const [confirmOpened, { open: openConfirm, close: closeConfirm }] =
    useDisclosure(false);
  const [filterText, setFilterText] = useState("");
  const [page, setPage] = useState(1);

  // Listen for history changes from other windows (e.g., overlay after transcription)
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    const setup = async () => {
      unlisten = await tauriAPI.onHistoryChanged(() => {
        queryClient.invalidateQueries({ queryKey: ["history"] });
      });
    };

    setup();

    return () => {
      unlisten?.();
    };
  }, [queryClient]);

  const handleDelete = (id: string) => {
    deleteEntry.mutate(id);
  };

  const handleClearAll = () => {
    clearHistory.mutate(undefined, {
      onSuccess: () => {
        closeConfirm();
      },
    });
  };

  const filteredHistory = useMemo(() => {
    if (!history) return [];
    const query = filterText.trim().toLowerCase();
    if (!query) return history;
    return history.filter((entry) => {
      const text = (entry.text ?? "").toLowerCase();
      const status = (entry.status ?? "success").toLowerCase();
      const err = (entry.error_message ?? "").toLowerCase();
      return (
        text.includes(query) || status.includes(query) || err.includes(query)
      );
    });
  }, [history, filterText]);

  const totalPages = Math.max(
    1,
    Math.ceil(filteredHistory.length / HISTORY_PAGE_SIZE)
  );

  const canGoPrev = page > 1;
  const canGoNext = page < totalPages;

  // Keep the current page in bounds as history/filter changes.
  useEffect(() => {
    setPage((current) => Math.min(Math.max(1, current), totalPages));
  }, [totalPages]);

  // When the filter changes, reset to page 1 so results are predictable.
  useEffect(() => {
    setPage(1);
  }, [filterText]);

  const pageHistory = useMemo(() => {
    const start = (page - 1) * HISTORY_PAGE_SIZE;
    return filteredHistory.slice(start, start + HISTORY_PAGE_SIZE);
  }, [filteredHistory, page]);

  if (isLoading) {
    return (
      <div className="animate-in animate-in-delay-2">
        <div className="section-header">
          <span className="section-title">History</span>
        </div>
        <div className="empty-state">
          <p className="empty-state-text">Loading history...</p>
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="animate-in animate-in-delay-2">
        <div className="section-header">
          <span className="section-title">History</span>
        </div>
        <div className="empty-state">
          <p className="empty-state-text" style={{ color: "#ef4444" }}>
            Failed to load history
          </p>
        </div>
      </div>
    );
  }

  if (!history || history.length === 0) {
    return (
      <div className="animate-in animate-in-delay-2">
        <div className="section-header">
          <span className="section-title">History</span>
        </div>
        <div className="empty-state">
          <MessageSquare className="empty-state-icon" />
          <h4 className="empty-state-title">No dictation history yet</h4>
          <p className="empty-state-text">
            Your transcribed text will appear here after you use voice
            dictation.
          </p>
        </div>
      </div>
    );
  }

  const groupedHistory = groupHistoryByDate(pageHistory);

  return (
    <div className="animate-in animate-in-delay-2">
      <div className="section-header">
        <span className="section-title">History</span>
        <Button
          variant="subtle"
          size="compact-sm"
          color="gray"
          onClick={openConfirm}
          disabled={clearHistory.isPending}
        >
          Clear All
        </Button>
      </div>

      <div
        style={{
          display: "flex",
          gap: 12,
          alignItems: "center",
          marginBottom: 16,
          flexWrap: "wrap",
        }}
      >
        <TextInput
          value={filterText}
          onChange={(e) => setFilterText(e.currentTarget.value)}
          placeholder="Filter history…"
          leftSection={<Search size={14} />}
          rightSection={
            filterText.trim().length > 0 ? (
              <ActionIcon
                variant="subtle"
                size="sm"
                color="gray"
                onClick={() => setFilterText("")}
                title="Clear filter"
              >
                <X size={14} />
              </ActionIcon>
            ) : null
          }
          styles={{
            input: {
              backgroundColor: "var(--bg-card)",
              borderColor: "var(--border-default)",
              color: "var(--text-primary)",
            },
          }}
          size="xs"
          style={{ width: 240 }}
        />

        <Text c="dimmed" size="xs" style={{ whiteSpace: "nowrap" }}>
          {filteredHistory.length} result
          {filteredHistory.length === 1 ? "" : "s"}
        </Text>

        <Group style={{ marginLeft: "auto" }} gap={6}>
          <ActionIcon
            variant="subtle"
            size="sm"
            color="gray"
            onClick={() => setPage(1)}
            disabled={!canGoPrev}
            title="First page"
          >
            <ChevronsLeft size={16} />
          </ActionIcon>
          <ActionIcon
            variant="subtle"
            size="sm"
            color="gray"
            onClick={() => setPage((p) => Math.max(1, p - 1))}
            disabled={!canGoPrev}
            title="Previous page"
          >
            <ChevronLeft size={16} />
          </ActionIcon>
          <ActionIcon
            variant="subtle"
            size="sm"
            color="gray"
            onClick={() => setPage((p) => Math.min(totalPages, p + 1))}
            disabled={!canGoNext}
            title="Next page"
          >
            <ChevronRight size={16} />
          </ActionIcon>
          <ActionIcon
            variant="subtle"
            size="sm"
            color="gray"
            onClick={() => setPage(totalPages)}
            disabled={!canGoNext}
            title="Last page"
          >
            <ChevronsRight size={16} />
          </ActionIcon>
        </Group>
      </div>

      <Modal
        opened={confirmOpened}
        onClose={closeConfirm}
        title="Clear History"
        centered
        size="sm"
      >
        <Text size="sm" mb="lg">
          Are you sure you want to clear all history? This action cannot be
          undone.
        </Text>
        <Group justify="flex-end">
          <Button variant="default" onClick={closeConfirm}>
            Cancel
          </Button>
          <Button
            color="red"
            onClick={handleClearAll}
            loading={clearHistory.isPending}
          >
            Clear All
          </Button>
        </Group>
      </Modal>

      {filteredHistory.length === 0 ? (
        <div className="empty-state">
          <MessageSquare className="empty-state-icon" />
          <h4 className="empty-state-title">No matches</h4>
          <p className="empty-state-text">Try a different filter.</p>
        </div>
      ) : (
        groupedHistory.map((group) => (
          <div key={group.date} style={{ marginBottom: 24 }}>
            <p
              className="section-title"
              style={{ marginBottom: 12, fontSize: 11 }}
            >
              {group.date}
            </p>
            <div className="history-feed">
              {group.items.map((entry) => (
                <div key={entry.id} className="history-item">
                  <span className="history-time">
                    {formatTime(entry.timestamp)}
                  </span>
                  <div className="history-text">
                    {(entry.status ?? "success") === "in_progress" ? (
                      <Group gap={8} wrap="nowrap" style={{ minWidth: 0 }}>
                        <Loader size="xs" color="gray" />
                        <Text size="sm" c="dimmed" style={{ minWidth: 0 }}>
                          Transcribing…
                        </Text>
                      </Group>
                    ) : (entry.status ?? "success") === "error" ? (
                      <Group gap={8} wrap="nowrap" style={{ minWidth: 0 }}>
                        <Text size="sm" c="red">
                          Failed
                        </Text>
                        <Text
                          size="sm"
                          c="dimmed"
                          style={{
                            flex: 1,
                            minWidth: 0,
                            whiteSpace: "nowrap",
                            overflow: "hidden",
                            textOverflow: "ellipsis",
                          }}
                          title={entry.error_message ?? undefined}
                        >
                          {entry.error_message?.trim()
                            ? entry.error_message
                            : "Try again"}
                        </Text>
                      </Group>
                    ) : (
                      <Text
                        size="sm"
                        c={entry.text?.trim() ? undefined : "dimmed"}
                        style={
                          entry.text?.trim()
                            ? undefined
                            : { fontStyle: "italic" }
                        }
                        title={
                          entry.text?.trim()
                            ? undefined
                            : "No transcript was produced"
                        }
                      >
                        {entry.text?.trim() ? entry.text : "No transcript"}
                      </Text>
                    )}
                  </div>
                  <div className="history-actions">
                    <ActionIcon
                      variant="subtle"
                      size="sm"
                      color="gray"
                      disabled={(entry.status ?? "success") === "in_progress"}
                      loading={
                        retryMutation.isPending &&
                        retryMutation.variables === entry.id
                      }
                      onClick={() => {
                        notifications.show({
                          title: "Retrying",
                          message: "Re-running transcription…",
                          color: "blue",
                        });
                        retryMutation.mutate(entry.id, {
                          onSuccess: () => {
                            notifications.show({
                              title: "Retry complete",
                              message:
                                "Check History / Request Logs for the new entry.",
                              color: "teal",
                            });
                          },
                          onError: (e) => {
                            notifications.show({
                              title: "Retry failed",
                              message: String(e),
                              color: "red",
                            });
                          },
                        });
                      }}
                      title={
                        (entry.status ?? "success") === "in_progress"
                          ? "Already transcribing"
                          : "Retry transcription"
                      }
                    >
                      <RotateCcw size={14} />
                    </ActionIcon>
                    <ActionIcon
                      variant="subtle"
                      size="sm"
                      color="gray"
                      onClick={() => clipboard.copy(entry.text)}
                      title="Copy to clipboard"
                      disabled={!entry.text || entry.text.trim().length === 0}
                    >
                      <Copy size={14} />
                    </ActionIcon>
                    <ActionIcon
                      variant="subtle"
                      size="sm"
                      color="red"
                      onClick={() => handleDelete(entry.id)}
                      title="Delete"
                      disabled={deleteEntry.isPending}
                    >
                      <Trash2 size={14} />
                    </ActionIcon>
                  </div>
                </div>
              ))}
            </div>
          </div>
        ))
      )}
    </div>
  );
}
