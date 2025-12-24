import {
  ActionIcon,
  Badge,
  Box,
  Button,
  Checkbox,
  Collapse,
  Divider,
  Group,
  Indicator,
  Loader,
  Modal,
  Popover,
  ScrollArea,
  Stack,
  Switch,
  Text,
  TextInput,
  Tooltip,
  UnstyledButton,
} from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { useClipboard, useDisclosure } from "@mantine/hooks";
import { useQueryClient } from "@tanstack/react-query";
import { format, isToday, isYesterday } from "date-fns";
import {
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronsLeft,
  ChevronsRight,
  Copy,
  Filter,
  FolderOpen,
  MessageSquare,
  Pause,
  Play,
  RotateCcw,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Store } from "@tauri-apps/plugin-store";
import {
  useClearHistory,
  useDeleteHistoryEntry,
  useHistory,
  useRecordingsStats,
  useRetryTranscription,
} from "../lib/queries";
import { recordingsAPI, tauriAPI } from "../lib/tauri";
import { useRecordingPlayer } from "../lib/useRecordingPlayer";
import { listAllLlmModelKeys, listAllSttModelKeys } from "../lib/modelOptions";

const HISTORY_FILTERS_STORE_FILE = "ui.json";
const HISTORY_FILTERS_STORE_KEY = "history_feed_filters_v1";

type PersistedHistoryFilters = {
  filterText: string;
  showFailed: boolean;
  showEmptyTranscript: boolean;
  selectedSttModelKeys: string[];
  selectedLlmModelKeys: string[];
};

let historyFiltersStore: Store | null = null;

async function getHistoryFiltersStore(): Promise<Store> {
  if (!historyFiltersStore) {
    historyFiltersStore = await Store.load(HISTORY_FILTERS_STORE_FILE);
  }
  return historyFiltersStore;
}

function normalizeStringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is string => typeof item === "string");
}

function normalizePersistedHistoryFilters(
  value: unknown
): PersistedHistoryFilters | null {
  if (!value || typeof value !== "object") return null;
  const v = value as Record<string, unknown>;

  const filterText = typeof v.filterText === "string" ? v.filterText : "";
  const showFailed = typeof v.showFailed === "boolean" ? v.showFailed : true;
  const showEmptyTranscript =
    typeof v.showEmptyTranscript === "boolean" ? v.showEmptyTranscript : false;

  const rawSelectedSttModelKeys = normalizeStringArray(v.selectedSttModelKeys);
  const rawSelectedLlmModelKeys = normalizeStringArray(v.selectedLlmModelKeys);

  // Defensive: drop unknown keys so the checkbox UI doesn't get stuck with
  // selections that it can't render/unselect.
  const knownSttKeys = new Set(listAllSttModelKeys().map((o) => o.key));
  const knownLlmKeys = new Set(listAllLlmModelKeys().map((o) => o.key));

  const selectedSttModelKeys = rawSelectedSttModelKeys.filter((k) =>
    knownSttKeys.has(k)
  );
  const selectedLlmModelKeys = rawSelectedLlmModelKeys.filter((k) =>
    knownLlmKeys.has(k)
  );

  return {
    filterText,
    showFailed,
    showEmptyTranscript,
    selectedSttModelKeys,
    selectedLlmModelKeys,
  };
}

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
    stt_provider?: string | null;
    stt_model?: string | null;
    llm_provider?: string | null;
    llm_model?: string | null;
  }>;
}

function groupHistoryByDate(
  history: Array<{
    id: string;
    text: string;
    timestamp: string;
    status?: "in_progress" | "success" | "error";
    error_message?: string | null;
    stt_provider?: string | null;
    stt_model?: string | null;
    llm_provider?: string | null;
    llm_model?: string | null;
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
  const recordingsStats = useRecordingsStats();
  const deleteEntry = useDeleteHistoryEntry();
  const clearHistory = useClearHistory();
  const retryMutation = useRetryTranscription();
  const clipboard = useClipboard();

  const recordingsGbForTooltip = (() => {
    const bytes = recordingsStats.data?.bytes;
    if (typeof bytes !== "number" || !Number.isFinite(bytes)) return null;
    return bytes / 1024 ** 3;
  })();

  const player = useRecordingPlayer({
    onError: (message) => {
      notifications.show({
        title: "Playback",
        message,
        color: "red",
      });
    },
  });
  const [confirmOpened, { open: openConfirm, close: closeConfirm }] =
    useDisclosure(false);
  const [filtersOpened, filtersHandlers] = useDisclosure(false);
  const [sttExpanded, setSttExpanded] = useState(false);
  const [llmExpanded, setLlmExpanded] = useState(false);
  const [filterText, setFilterText] = useState("");
  const [page, setPage] = useState(1);

  const [showFailed, setShowFailed] = useState(true);
  const [showEmptyTranscript, setShowEmptyTranscript] = useState(false);
  const [selectedSttModelKeys, setSelectedSttModelKeys] = useState<string[]>(
    []
  );
  const [selectedLlmModelKeys, setSelectedLlmModelKeys] = useState<string[]>(
    []
  );

  // Persist history filters (UI-only) across app restarts.
  // Hydration is async; we gate saving until after it completes.
  const [hasHydratedPersistedFilters, setHasHydratedPersistedFilters] =
    useState(false);

  useEffect(() => {
    let cancelled = false;

    const hydrate = async () => {
      try {
        const store = await getHistoryFiltersStore();
        const raw = await store.get(HISTORY_FILTERS_STORE_KEY);
        const normalized = normalizePersistedHistoryFilters(raw);

        if (!normalized || cancelled) return;

        setFilterText(normalized.filterText);
        setShowFailed(normalized.showFailed);
        setShowEmptyTranscript(normalized.showEmptyTranscript);
        setSelectedSttModelKeys(normalized.selectedSttModelKeys);
        setSelectedLlmModelKeys(normalized.selectedLlmModelKeys);
      } catch (e) {
        // Never block history UI if persistence fails.
        console.warn("Failed to hydrate history filters:", e);
      } finally {
        if (!cancelled) setHasHydratedPersistedFilters(true);
      }
    };

    hydrate();

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!hasHydratedPersistedFilters) return;

    const timeout = setTimeout(() => {
      const persist = async () => {
        try {
          const store = await getHistoryFiltersStore();
          const payload: PersistedHistoryFilters = {
            filterText,
            showFailed,
            showEmptyTranscript,
            selectedSttModelKeys,
            selectedLlmModelKeys,
          };
          await store.set(HISTORY_FILTERS_STORE_KEY, payload);
          await store.save();
        } catch (e) {
          console.warn("Failed to persist history filters:", e);
        }
      };

      persist();
    }, 250);

    return () => clearTimeout(timeout);
  }, [
    hasHydratedPersistedFilters,
    filterText,
    showFailed,
    showEmptyTranscript,
    selectedSttModelKeys,
    selectedLlmModelKeys,
  ]);

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

  const handleOpenFolder = async () => {
    try {
      await recordingsAPI.openRecordingsFolder();
    } catch (e) {
      notifications.show({
        title: "Recordings",
        message: String(e),
        color: "red",
      });
    }
  };

  const sttModelUsageCounts = useMemo(() => {
    const counts = new Map<string, number>();
    if (!history) return counts;
    for (const entry of history) {
      if (!entry.stt_provider || !entry.stt_model) continue;
      const key = `${entry.stt_provider}::${entry.stt_model}`;
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    return counts;
  }, [history]);

  const llmModelUsageCounts = useMemo(() => {
    const counts = new Map<string, number>();
    if (!history) return counts;
    for (const entry of history) {
      if (!entry.llm_provider || !entry.llm_model) continue;
      const key = `${entry.llm_provider}::${entry.llm_model}`;
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    return counts;
  }, [history]);

  const availableSttModelOptions = useMemo(() => listAllSttModelKeys(), []);
  const availableLlmModelOptions = useMemo(() => listAllLlmModelKeys(), []);

  // Check if any filters are active (for showing indicator on filter button)
  const hasActiveFilters = useMemo(() => {
    return (
      !showFailed ||
      showEmptyTranscript ||
      selectedSttModelKeys.length > 0 ||
      selectedLlmModelKeys.length > 0
    );
  }, [
    showFailed,
    showEmptyTranscript,
    selectedSttModelKeys,
    selectedLlmModelKeys,
  ]);

  const resetFilters = () => {
    setShowFailed(true);
    setShowEmptyTranscript(false);
    setSelectedSttModelKeys([]);
    setSelectedLlmModelKeys([]);
  };

  const filteredHistory = useMemo(() => {
    if (!history) return [];
    const query = filterText.trim().toLowerCase();

    return history.filter((entry) => {
      // 1) Text search (existing behavior)
      if (query) {
        const text = (entry.text ?? "").toLowerCase();
        const status = (entry.status ?? "success").toLowerCase();
        const err = (entry.error_message ?? "").toLowerCase();
        const matchesText =
          text.includes(query) || status.includes(query) || err.includes(query);
        if (!matchesText) return false;
      }

      // 2) Show Failed
      if (!showFailed && (entry.status ?? "success") === "error") {
        return false;
      }

      // 3) Show Empty transcript
      if (
        !showEmptyTranscript &&
        (entry.status ?? "success") === "success" &&
        !entry.text?.trim()
      ) {
        return false;
      }

      // 4) STT model filter
      if (
        selectedSttModelKeys.length > 0 &&
        availableSttModelOptions.length > 0
      ) {
        const provider = entry.stt_provider;
        const model = entry.stt_model;
        if (!provider || !model) return false;
        const key = `${provider}::${model}`;
        if (!selectedSttModelKeys.includes(key)) return false;
      }

      // 5) LLM model filter (rewrite step)
      if (
        selectedLlmModelKeys.length > 0 &&
        availableLlmModelOptions.length > 0
      ) {
        const provider = entry.llm_provider;
        const model = entry.llm_model;
        if (!provider || !model) return false;
        const key = `${provider}::${model}`;
        if (!selectedLlmModelKeys.includes(key)) return false;
      }

      return true;
    });
  }, [
    history,
    filterText,
    showFailed,
    showEmptyTranscript,
    availableSttModelOptions,
    availableLlmModelOptions,
    selectedSttModelKeys,
    selectedLlmModelKeys,
  ]);

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
  }, [
    filterText,
    showFailed,
    showEmptyTranscript,
    selectedSttModelKeys,
    selectedLlmModelKeys,
  ]);

  const pageHistory = useMemo(() => {
    const start = (page - 1) * HISTORY_PAGE_SIZE;
    return filteredHistory.slice(start, start + HISTORY_PAGE_SIZE);
  }, [filteredHistory, page]);

  if (isLoading) {
    return (
      <div className="animate-in animate-in-delay-2">
        <div className="section-header">
          <span className="section-title section-title--no-accent">
            History
          </span>
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
          <span className="section-title section-title--no-accent">
            History
          </span>
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
          <span className="section-title section-title--no-accent">
            History
          </span>
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

  const totalHistoryCount = history?.length ?? 0;
  const isFiltering = filterText.trim().length > 0 || hasActiveFilters;

  return (
    <div className="animate-in animate-in-delay-2">
      <div className="section-header">
        <span className="section-title section-title--no-accent">History</span>
        <Group gap={6}>
          <Tooltip label="Clear all history" withArrow>
            <span style={{ display: "inline-block" }}>
              <Button
                variant="subtle"
                size="compact-sm"
                color="gray"
                onClick={openConfirm}
                disabled={clearHistory.isPending}
              >
                Clear All
              </Button>
            </span>
          </Tooltip>

          <Tooltip
            label={
              recordingsStats.isLoading || recordingsGbForTooltip === null
                ? "Open recordings folder"
                : `Open recordings folder • ${recordingsGbForTooltip.toFixed(
                    2
                  )} GB`
            }
            withArrow
          >
            <Button
              variant="subtle"
              size="compact-sm"
              color="gray"
              px={6}
              onClick={handleOpenFolder}
              aria-label="Open recordings folder"
            >
              <FolderOpen size={14} />
            </Button>
          </Tooltip>
        </Group>
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
              backgroundColor: "transparent",
              borderColor: "var(--border-default)",
              color: "var(--text-primary)",
            },
          }}
          size="xs"
          style={{ width: 240 }}
        />

        <Popover
          opened={filtersOpened}
          onChange={(opened) =>
            opened ? filtersHandlers.open() : filtersHandlers.close()
          }
          position="bottom-start"
          shadow="lg"
          radius="md"
        >
          <Popover.Target>
            <Indicator
              size={8}
              color="orange"
              offset={2}
              disabled={!hasActiveFilters}
              processing={hasActiveFilters}
            >
              <ActionIcon
                variant={hasActiveFilters ? "light" : "subtle"}
                size="sm"
                color={hasActiveFilters ? "orange" : "gray"}
                onClick={filtersHandlers.toggle}
                title="Filter options"
                aria-label="Filter options"
              >
                <Filter size={16} />
              </ActionIcon>
            </Indicator>
          </Popover.Target>
          <Popover.Dropdown
            p={0}
            style={{
              backgroundColor: "var(--bg-elevated)",
              border: "1px solid var(--border-default)",
              width: 280,
              overflow: "hidden",
            }}
          >
            {/* Header */}
            <Group justify="space-between" p="xs" pb={8}>
              <Text size="sm" fw={600}>
                Filters
              </Text>
              {hasActiveFilters && (
                <Button
                  variant="subtle"
                  size="compact-xs"
                  color="gray"
                  onClick={resetFilters}
                  styles={{ root: { height: 20, padding: "0 6px" } }}
                >
                  Reset
                </Button>
              )}
            </Group>

            <Divider color="var(--border-default)" />

            {/* Toggle filters */}
            <Stack gap={0} p="xs">
              <Group justify="space-between" py={4}>
                <Text size="xs">Show failed</Text>
                <Switch
                  size="xs"
                  checked={showFailed}
                  onChange={(e) => setShowFailed(e.currentTarget.checked)}
                />
              </Group>
              <Group justify="space-between" py={4}>
                <Text size="xs">Show empty transcripts</Text>
                <Switch
                  size="xs"
                  checked={showEmptyTranscript}
                  onChange={(e) =>
                    setShowEmptyTranscript(e.currentTarget.checked)
                  }
                />
              </Group>
            </Stack>

            <Divider color="var(--border-default)" />

            {/* STT Models Section */}
            <Box>
              <UnstyledButton
                onClick={() => setSttExpanded((v) => !v)}
                w="100%"
                py={8}
                px="xs"
                style={{
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "space-between",
                }}
              >
                <Group gap={8}>
                  <Text size="xs" fw={500}>
                    STT Models
                  </Text>
                  {selectedSttModelKeys.length > 0 && (
                    <Badge size="xs" variant="filled" color="orange" circle>
                      {selectedSttModelKeys.length}
                    </Badge>
                  )}
                </Group>
                <ChevronDown
                  size={14}
                  style={{
                    transform: sttExpanded ? "rotate(180deg)" : "rotate(0)",
                    transition: "transform 150ms ease",
                    color: "var(--text-secondary)",
                  }}
                />
              </UnstyledButton>
              <Collapse in={sttExpanded}>
                <Box px="xs" pb="xs">
                  {availableSttModelOptions.length === 0 ? (
                    <Text c="dimmed" size="xs">
                      No STT models available.
                    </Text>
                  ) : (
                    <ScrollArea.Autosize mah={140} type="auto" offsetScrollbars>
                      <Checkbox.Group
                        value={selectedSttModelKeys}
                        onChange={setSelectedSttModelKeys}
                      >
                        <Stack gap={6}>
                          {availableSttModelOptions.map((opt) => {
                            const count = sttModelUsageCounts.get(opt.key) ?? 0;
                            return (
                              <Checkbox
                                key={opt.key}
                                value={opt.key}
                                size="xs"
                                label={
                                  <Group gap={6} wrap="nowrap">
                                    <Text size="xs" style={{ flex: 1 }}>
                                      {opt.label}
                                    </Text>
                                    <Badge
                                      size="xs"
                                      variant="light"
                                      color={count > 0 ? "gray" : "dark"}
                                      styles={{
                                        root: {
                                          minWidth: 24,
                                          height: 16,
                                          padding: "0 4px",
                                        },
                                      }}
                                    >
                                      {count}
                                    </Badge>
                                  </Group>
                                }
                                styles={{
                                  label: { width: "100%" },
                                  body: { alignItems: "center" },
                                }}
                              />
                            );
                          })}
                        </Stack>
                      </Checkbox.Group>
                    </ScrollArea.Autosize>
                  )}
                </Box>
              </Collapse>
            </Box>

            <Divider color="var(--border-default)" />

            {/* LLM Models Section */}
            <Box>
              <UnstyledButton
                onClick={() => setLlmExpanded((v) => !v)}
                w="100%"
                py={8}
                px="xs"
                style={{
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "space-between",
                }}
              >
                <Group gap={8}>
                  <Text size="xs" fw={500}>
                    LLM Models
                  </Text>
                  {selectedLlmModelKeys.length > 0 && (
                    <Badge size="xs" variant="filled" color="orange" circle>
                      {selectedLlmModelKeys.length}
                    </Badge>
                  )}
                </Group>
                <ChevronDown
                  size={14}
                  style={{
                    transform: llmExpanded ? "rotate(180deg)" : "rotate(0)",
                    transition: "transform 150ms ease",
                    color: "var(--text-secondary)",
                  }}
                />
              </UnstyledButton>
              <Collapse in={llmExpanded}>
                <Box px="xs" pb="xs">
                  {availableLlmModelOptions.length === 0 ? (
                    <Text c="dimmed" size="xs">
                      No LLM models available.
                    </Text>
                  ) : (
                    <ScrollArea.Autosize mah={140} type="auto" offsetScrollbars>
                      <Checkbox.Group
                        value={selectedLlmModelKeys}
                        onChange={setSelectedLlmModelKeys}
                      >
                        <Stack gap={6}>
                          {availableLlmModelOptions.map((opt) => {
                            const count = llmModelUsageCounts.get(opt.key) ?? 0;
                            return (
                              <Checkbox
                                key={opt.key}
                                value={opt.key}
                                size="xs"
                                label={
                                  <Group gap={6} wrap="nowrap">
                                    <Text size="xs" style={{ flex: 1 }}>
                                      {opt.label}
                                    </Text>
                                    <Badge
                                      size="xs"
                                      variant="light"
                                      color={count > 0 ? "gray" : "dark"}
                                      styles={{
                                        root: {
                                          minWidth: 24,
                                          height: 16,
                                          padding: "0 4px",
                                        },
                                      }}
                                    >
                                      {count}
                                    </Badge>
                                  </Group>
                                }
                                styles={{
                                  label: { width: "100%" },
                                  body: { alignItems: "center" },
                                }}
                              />
                            );
                          })}
                        </Stack>
                      </Checkbox.Group>
                    </ScrollArea.Autosize>
                  )}
                </Box>
              </Collapse>
            </Box>
          </Popover.Dropdown>
        </Popover>

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
          {totalHistoryCount === 0 ? (
            <>
              <h4 className="empty-state-title">No dictation history yet</h4>
              <p className="empty-state-text">
                Your transcribed text will appear here after you use voice
                dictation.
              </p>
            </>
          ) : isFiltering ? (
            <>
              <h4 className="empty-state-title">No matches</h4>
              <p className="empty-state-text">Try a different filter.</p>
            </>
          ) : (
            <>
              <h4 className="empty-state-title">Nothing to show</h4>
              <p className="empty-state-text">
                Start your first recording to see it here.
              </p>
            </>
          )}
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
                        <Loader size="xs" color="orange" />
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
                      loading={player.isLoading(entry.id)}
                      onClick={() => player.toggle(entry.id)}
                      title={
                        player.isPlaying(entry.id)
                          ? "Pause recording"
                          : "Play recording"
                      }
                      aria-label={
                        player.isPlaying(entry.id)
                          ? "Pause recording"
                          : "Play recording"
                      }
                    >
                      {player.isPlaying(entry.id) ? (
                        <Pause size={14} />
                      ) : (
                        <Play size={14} />
                      )}
                    </ActionIcon>
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
                          color: "orange",
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
