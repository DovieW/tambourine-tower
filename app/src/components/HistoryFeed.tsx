import { useClipboard, useDisclosure, useMediaQuery } from "@mantine/hooks";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useState } from "react";
import { formatErrorMessage } from "../lib/formatError";
import { useHistoryFeedOrchestration } from "../lib/history/orchestration";
import {
	type AnalysisPromptStyle,
	buildAnalysisPrompt,
	estimateTokenCount,
	getHistoryFeedEmptyState,
	groupHistoryForDisplay,
	HISTORY_PAGE_SIZE,
} from "../lib/history/readModel";
import {
	getHistoryPageCount,
	useHistoryFeedFilters,
} from "../lib/history/useHistoryFeedFilters";
import { listAllLlmModelKeys, listAllSttModelKeys } from "../lib/modelOptions";
import {
	useHistoryAll,
	useHistoryPage,
	useRecordingsStats,
	useRequestLogs,
	useRetryTranscription,
	useSettings,
} from "../lib/queries";
import {
	dataAPI,
	type HistoryDeleteMode,
	llmAPI,
	recordingsAPI,
	tauriAPI,
} from "../lib/tauri";
import { useRecordingPlayer } from "../lib/useRecordingPlayer";
import { HistoryAnalysisPanel } from "./history/HistoryAnalysisPanel";
import { HistoryDeleteDialogs } from "./history/HistoryDeleteDialogs";
import { HistoryFeedFilterToolbar } from "./history/HistoryFeedFilterToolbar";
import { HistoryFeedList } from "./history/HistoryFeedList";
import { HistoryFeedPagination } from "./history/HistoryFeedPagination";
import { prunePendingHistoryDocuments } from "./history/HistoryReader";

export function HistoryFeed({
	onJumpToLog,
}: {
	onJumpToLog?: (logId: string) => void;
} = {}) {
	const queryClient = useQueryClient();
	const recordingsStats = useRecordingsStats();

	const invalidateHistoryQueries = () => {
		void prunePendingHistoryDocuments();
		queryClient.invalidateQueries({ queryKey: ["history"] });
		queryClient.invalidateQueries({ queryKey: ["historyAll"] });
		queryClient.invalidateQueries({ queryKey: ["historyPage"] });
		queryClient.invalidateQueries({ queryKey: ["recordingsStats"] });
		// Notify other windows about history change
		tauriAPI.emitHistoryChanged();
	};

	const deleteHistoryEntryEx = useMutation({
		mutationFn: async (args: { id: string; mode: HistoryDeleteMode }) =>
			tauriAPI.deleteHistoryEntryEx(args.id, args.mode),
		onSuccess: () => {
			invalidateHistoryQueries();
		},
	});

	const deleteAllHistoryAndRecordings = useMutation({
		mutationFn: async () => {
			const deletedRecordings = await dataAPI.deleteAllRecordings();
			await tauriAPI.clearHistory();
			await tauriAPI.emitHistoryChanged();
			return deletedRecordings;
		},
		onSuccess: (deletedRecordings) => {
			void prunePendingHistoryDocuments();
			queryClient.invalidateQueries({ queryKey: ["history"] });
			queryClient.invalidateQueries({ queryKey: ["historyAll"] });
			queryClient.invalidateQueries({ queryKey: ["historyPage"] });
			queryClient.invalidateQueries({ queryKey: ["recordingsStats"] });

			notifications.show({
				title: "History",
				message: `Deleted transcripts and ${Number(
					deletedRecordings,
				).toLocaleString()} recording${deletedRecordings === 1 ? "" : "s"}.`,
				color: "green",
			});
		},
		onError: (e) => {
			notifications.show({
				title: "History",
				message: formatErrorMessage(e),
				color: "red",
			});
		},
	});
	const retryMutation = useRetryTranscription();
	const clipboard = useClipboard();

	const { data: settings } = useSettings();
	const requestLogsLimit = (() => {
		const fallback = 50;
		const mode = settings?.request_logs_retention_mode;
		if (mode === "amount") {
			const amount = settings?.request_logs_retention_amount;
			if (typeof amount === "number" && Number.isFinite(amount)) {
				return Math.max(1, Math.min(200, Math.floor(amount)));
			}
		}
		return fallback;
	})();
	const { data: requestLogs } = useRequestLogs(requestLogsLimit);
	const requestLogIds = useMemo(
		() => new Set((requestLogs ?? []).map((l) => l.id)),
		[requestLogs],
	);

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

	const isDeleteDialogBusy = deleteAllHistoryAndRecordings.isPending;
	const [confirmOpened, { open: openConfirm, close: closeConfirm }] =
		useDisclosure(false);
	const [analysisOpened, analysisHandlers] = useDisclosure(false);
	const filters = useHistoryFeedFilters();

	// Main view: fetch only the current page (server-side filtering + pagination).
	const { data: historyPage, isLoading, error } = useHistoryPage(filters.query);

	// Lightweight query used only to power the "Retry last failed" quick action.
	// We intentionally *don't* use the current filter state here so the action can
	// work even if the user is filtering the list.
	const retryActionHistoryQuery = useHistoryPage({
		filterText: "",
		showFailed: true,
		showEmptyTranscript: true,
		selectedSttModelKeys: [],
		selectedLlmModelKeys: [],
		page: 1,
		pageSize: 200,
		includeUsageCounts: false,
	});

	const getRecordingAssetUrl = useCallback(
		(requestId: string) => recordingsAPI.getRecordingAssetUrl({ requestId }),
		[],
	);
	const historyOrchestration = useHistoryFeedOrchestration({
		pageEntries: historyPage?.items ?? [],
		retryActionEntries: retryActionHistoryQuery.data?.items ?? [],
		copyToClipboard: clipboard.copy,
		getRecordingAssetUrl,
		getDeleteOptions: tauriAPI.getHistoryDeleteOptions,
		deleteHistoryEntry: (args) => deleteHistoryEntryEx.mutateAsync(args),
		retryEntry: (entryId) => retryMutation.mutateAsync(entryId),
	});

	// Optional: fetch full history only when the analysis modal is opened.
	const allHistoryQuery = useHistoryAll({ enabled: analysisOpened });

	const pageHistory = historyOrchestration.pageHistory;
	const totalHistoryCount = historyPage?.totalAll ?? 0;
	const totalFilteredCount = historyPage?.totalFiltered ?? 0;
	const totalPages = getHistoryPageCount(
		totalFilteredCount,
		historyPage?.pageSize ?? HISTORY_PAGE_SIZE,
	);

	// Keep local page state aligned with backend clamping.
	useEffect(() => {
		filters.syncServerPage(historyPage?.page);
	}, [filters.syncServerPage, historyPage?.page]);

	const [analysisPrompt, setAnalysisPrompt] = useState<string>("");
	const [analysisSystemPrompt, setAnalysisSystemPrompt] = useState<string>("");
	const [analysisUserPrompt, setAnalysisUserPrompt] = useState<string>("");
	const [analysisIncludedCount, setAnalysisIncludedCount] = useState(0);
	const [_analysisTotalCount, setAnalysisTotalCount] = useState(0);
	const [
		analysisAvailableTranscriptsCount,
		setAnalysisAvailableTranscriptsCount,
	] = useState(0);
	const [
		analysisIncludeFromLastHoursInput,
		setAnalysisIncludeFromLastHoursInput,
	] = useState<string | number>("");
	const [analysisPromptStyle, setAnalysisPromptStyle] =
		useState<AnalysisPromptStyle>("productive");

	const [sendDrawerOpened, sendDrawerHandlers] = useDisclosure(false);
	const isNarrow = useMediaQuery("(max-width: 900px)");

	const { data: llmProviders } = useQuery({
		queryKey: ["llmProviders"],
		queryFn: () => llmAPI.getLlmProviders(),
		staleTime: 60_000,
	});

	const hasAnyLlmProviders = (llmProviders?.length ?? 0) > 0;

	const [sendProvider, setSendProvider] = useState<string | null>(null);
	const [sendModel, setSendModel] = useState<string | null>(null);
	const [sendOutput, setSendOutput] = useState<string>("");
	const [sendProviderUsed, setSendProviderUsed] = useState<string>("");
	const [sendModelUsed, setSendModelUsed] = useState<string>("");

	const sendToLlmMutation = useMutation({
		mutationFn: async (args: {
			provider: string;
			model: string | null;
			systemPrompt: string;
			userPrompt: string;
		}) =>
			llmAPI.complete({
				provider: args.provider,
				model: args.model,
				systemPrompt: args.systemPrompt,
				userPrompt: args.userPrompt,
			}),
	});

	const analysisEstimatedTokens = useMemo(
		() => estimateTokenCount(analysisPrompt),
		[analysisPrompt],
	);

	// Listen for history changes from other windows (e.g., overlay after transcription)
	useEffect(() => {
		let unlisten: (() => void) | undefined;

		const setup = async () => {
			unlisten = await tauriAPI.onHistoryChanged(() => {
				void prunePendingHistoryDocuments();
				queryClient.invalidateQueries({ queryKey: ["historyDetail"] });
				queryClient.invalidateQueries({ queryKey: ["history"] });
				queryClient.invalidateQueries({ queryKey: ["historyAll"] });
				queryClient.invalidateQueries({ queryKey: ["historyPage"] });
			});
		};

		void setup();

		return () => {
			unlisten?.();
		};
	}, [queryClient]);

	const handleDelete = (id: string) => {
		void (async () => {
			try {
				const outcome = await historyOrchestration.requestDeleteEntry(id);

				switch (outcome.kind) {
					case "opened_shared_dialog":
						return;
					case "deleted_entry":
						notifications.show({
							title: "History",
							message: "Deleted transcript.",
							color: "green",
						});
						return;
					case "deleted_entry_and_recording":
						notifications.show({
							title: "History",
							message: outcome.result.deleted_recording
								? "Deleted transcript and recording."
								: "Deleted transcript.",
							color: "green",
						});
						return;
				}
			} catch (e) {
				notifications.show({
					title: "History",
					message: formatErrorMessage(e),
					color: "red",
				});
			}
		})();
	};

	const handleDeleteAll = () => {
		deleteAllHistoryAndRecordings.mutate(undefined, {
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
				message: formatErrorMessage(e),
				color: "red",
			});
		}
	};

	const handleGenerateAnalysisPrompt = () => {
		if (allHistoryQuery.isLoading) {
			notifications.show({
				title: "Analyze transcripts",
				message: "Loading history…",
				color: "gray",
			});
			return;
		}

		const parsedHours =
			typeof analysisIncludeFromLastHoursInput === "number"
				? analysisIncludeFromLastHoursInput
				: analysisIncludeFromLastHoursInput.trim().length > 0
					? Number.parseFloat(analysisIncludeFromLastHoursInput)
					: NaN;
		const includeFromLastHours =
			Number.isFinite(parsedHours) && parsedHours > 0 ? parsedHours : null;

		const {
			prompt,
			systemPrompt,
			userPrompt,
			includedCount,
			totalCount,
			availableTranscriptsCount,
		} = buildAnalysisPrompt(allHistoryQuery.data ?? [], {
			includeFromLastHours,
			style: analysisPromptStyle,
		});
		setAnalysisPrompt(prompt);
		setAnalysisSystemPrompt(systemPrompt);
		setAnalysisUserPrompt(userPrompt);
		setAnalysisIncludedCount(includedCount);
		setAnalysisTotalCount(totalCount);
		setAnalysisAvailableTranscriptsCount(availableTranscriptsCount);
	};

	const sttModelUsageCounts = useMemo(() => {
		const counts = new Map<string, number>();
		for (const item of historyPage?.sttModelUsage ?? []) {
			counts.set(item.key, item.count);
		}
		return counts;
	}, [historyPage?.sttModelUsage]);

	const llmModelUsageCounts = useMemo(() => {
		const counts = new Map<string, number>();
		for (const item of historyPage?.llmModelUsage ?? []) {
			counts.set(item.key, item.count);
		}
		return counts;
	}, [historyPage?.llmModelUsage]);

	const availableSttModelOptions = useMemo(() => listAllSttModelKeys(), []);
	const availableLlmModelOptions = useMemo(() => listAllLlmModelKeys(), []);

	const canGoPrev = filters.page > 1;
	const canGoNext = filters.page < totalPages;

	// Important UX detail:
	// keep the filter input mounted even while the query is refetching,
	// otherwise typing can cause focus loss / flashing.
	const isInitialLoading = isLoading && !historyPage;

	const groupedHistory = useMemo(
		() => groupHistoryForDisplay(pageHistory),
		[pageHistory],
	);
	const emptyState =
		totalFilteredCount === 0
			? getHistoryFeedEmptyState({
					totalHistoryCount,
					isFiltering: filters.isFiltering,
				})
			: null;

	const canRetryLastFailed = historyOrchestration.canRetryLastFailed;
	const retryLastFailedTooltip = historyOrchestration.retryLastFailedTooltip;
	const openRecordingsTooltip =
		recordingsStats.isLoading || recordingsGbForTooltip === null
			? "Open recordings folder"
			: `Open recordings folder • ${recordingsGbForTooltip.toFixed(2)} GB`;

	const handleRetryLastFailed = () => {
		void (async () => {
			try {
				const outcome = await historyOrchestration.retryLastFailed();

				switch (outcome.kind) {
					case "no_candidate":
						return;
					case "missing_recording":
						notifications.show({
							title: "Retry",
							message:
								"No saved audio found for the most recent failed request.",
							color: "yellow",
						});
						return;
					case "retried":
						notifications.show({
							title: "Retry",
							message:
								"Retried the most recent failed request and copied the result.",
							color: "green",
						});
						return;
				}
			} catch (e) {
				notifications.show({
					title: "Retry failed",
					message: formatErrorMessage(e),
					color: "red",
				});
			}
		})();
	};

	const handleRetryEntry = (entryId: string) => {
		notifications.show({
			title: "Rerunning",
			message: "Re-running transcription…",
			color: "orange",
		});

		retryMutation.mutate(entryId, {
			onSuccess: () => {
				notifications.show({
					title: "Rerun complete",
					message: "Check History / Request Logs for the new entry.",
					color: "teal",
				});
			},
			onError: (error) => {
				notifications.show({
					title: "Rerun failed",
					message: formatErrorMessage(error),
					color: "red",
				});
			},
		});
	};

	const handleCloseDeleteOneDialog = () => {
		if (historyOrchestration.deleteOneBusy || deleteHistoryEntryEx.isPending)
			return;
		historyOrchestration.closeDeleteOneDialog();
	};

	const handleDeleteOnlyThisTranscript = () => {
		void (async () => {
			try {
				const outcome = await historyOrchestration.deleteOnlyThisTranscript();
				if (outcome.kind !== "deleted_entry") return;

				notifications.show({
					title: "History",
					message: "Deleted transcript.",
					color: "green",
				});
			} catch (error) {
				notifications.show({
					title: "History",
					message: formatErrorMessage(error),
					color: "red",
				});
			}
		})();
	};

	const handleDeleteAllUsingRecording = () => {
		void (async () => {
			try {
				const outcome = await historyOrchestration.deleteAllUsingRecording();
				if (outcome.kind !== "deleted_recording_and_all_entries") return;

				notifications.show({
					title: "History",
					message: `Deleted ${outcome.result.deleted_entries.toLocaleString()} transcript${
						outcome.result.deleted_entries === 1 ? "" : "s"
					}${outcome.result.deleted_recording ? " and recording" : ""}.`,
					color: "green",
				});
			} catch (error) {
				notifications.show({
					title: "History",
					message: formatErrorMessage(error),
					color: "red",
				});
			}
		})();
	};

	const handleOpenSendDrawer = () => {
		if (!analysisSystemPrompt || !analysisUserPrompt) {
			handleGenerateAnalysisPrompt();
		}

		sendDrawerHandlers.open();

		const firstProvider = (llmProviders ?? [])[0];
		setSendProvider((current) => current ?? firstProvider?.id ?? null);
		setSendModel((current) => {
			if (current) return current;
			if (!firstProvider) return null;
			return firstProvider.default_model ?? firstProvider.models?.[0] ?? null;
		});
	};

	const handleSendProviderChange = (providerId: string | null) => {
		setSendProvider(providerId);
		const provider = (llmProviders ?? []).find(
			(item) => item.id === providerId,
		);
		setSendModel(provider?.default_model ?? provider?.models?.[0] ?? null);
	};

	const handleGenerateSendOutput = () => {
		void (async () => {
			const provider = sendProvider ?? "";
			if (!provider) {
				notifications.show({
					title: "Send to LLM",
					message: "Select a provider.",
					color: "red",
				});
				return;
			}

			if (!analysisSystemPrompt || !analysisUserPrompt) {
				handleGenerateAnalysisPrompt();
			}

			if (!analysisUserPrompt.trim()) {
				notifications.show({
					title: "Send to LLM",
					message:
						"No transcripts matched the filter. Try a larger hour window, or record more.",
					color: "red",
				});
				return;
			}

			try {
				const result = await sendToLlmMutation.mutateAsync({
					provider,
					model: sendModel ?? null,
					systemPrompt: analysisSystemPrompt,
					userPrompt: analysisUserPrompt,
				});
				setSendOutput(result.output);
				setSendProviderUsed(result.provider_used);
				setSendModelUsed(result.model_used);
			} catch (error) {
				notifications.show({
					title: "Send to LLM",
					message: formatErrorMessage(error),
					color: "red",
				});
			}
		})();
	};

	const retryPendingEntryId =
		typeof retryMutation.variables === "string"
			? retryMutation.variables
			: undefined;
	const deleteOnlyThisTranscriptLoading =
		historyOrchestration.deleteOneBusy &&
		deleteHistoryEntryEx.variables?.mode === "entry_only";
	const deleteAllUsingRecordingLoading =
		historyOrchestration.deleteOneBusy &&
		deleteHistoryEntryEx.variables?.mode === "recording_and_all_entries";
	const deleteOneActionsDisabled =
		historyOrchestration.deleteOneBusy || deleteHistoryEntryEx.isPending;

	return (
		<div className="animate-in animate-in-delay-2">
			<HistoryFeedFilterToolbar
				retryLastFailedTooltip={retryLastFailedTooltip}
				onRetryLastFailed={handleRetryLastFailed}
				canRetryLastFailed={canRetryLastFailed}
				isRetryPending={retryMutation.isPending}
				openRecordingsTooltip={openRecordingsTooltip}
				onOpenRecordingsFolder={handleOpenFolder}
				onOpenAnalysis={analysisHandlers.open}
				onOpenDeleteAll={openConfirm}
				isDeleteAllPending={deleteAllHistoryAndRecordings.isPending}
				filterText={filters.filterText}
				onFilterTextChange={filters.setFilterText}
				onClearFilter={() => filters.setFilterText("")}
				filtersOpened={filters.filtersOpened}
				onFiltersOpenedChange={filters.setFiltersOpened}
				onToggleFilters={filters.toggleFiltersOpened}
				hasActiveFilters={filters.hasActiveFilters}
				onResetFilters={filters.resetFilters}
				showFailed={filters.showFailed}
				onShowFailedChange={filters.setShowFailed}
				showEmptyTranscript={filters.showEmptyTranscript}
				onShowEmptyTranscriptChange={filters.setShowEmptyTranscript}
				filtersExpandedSection={filters.filtersExpandedSection}
				onFiltersExpandedSectionChange={filters.setFiltersExpandedSection}
				availableSttModelOptions={availableSttModelOptions}
				sttModelUsageCounts={sttModelUsageCounts}
				selectedSttModelKeys={filters.selectedSttModelKeys}
				onSelectedSttModelKeysChange={filters.setSelectedSttModelKeys}
				availableLlmModelOptions={availableLlmModelOptions}
				llmModelUsageCounts={llmModelUsageCounts}
				selectedLlmModelKeys={filters.selectedLlmModelKeys}
				onSelectedLlmModelKeysChange={filters.setSelectedLlmModelKeys}
				totalFilteredCount={totalFilteredCount}
				pagination={
					<HistoryFeedPagination
						canGoPrev={canGoPrev}
						canGoNext={canGoNext}
						onFirstPage={() => filters.setPage(1)}
						onPreviousPage={() =>
							filters.setPage((current) => Math.max(1, current - 1))
						}
						onNextPage={() =>
							filters.setPage((current) => Math.min(totalPages, current + 1))
						}
						onLastPage={() => filters.setPage(totalPages)}
					/>
				}
			/>

			<HistoryDeleteDialogs
				confirmOpened={confirmOpened}
				onCloseConfirm={() => {
					if (isDeleteDialogBusy) return;
					closeConfirm();
				}}
				onDeleteAll={handleDeleteAll}
				isDeleteAllPending={deleteAllHistoryAndRecordings.isPending}
				deleteOneOpened={historyOrchestration.deleteOneOpened}
				onCloseDeleteOne={handleCloseDeleteOneDialog}
				deleteOneContext={historyOrchestration.deleteOneContext}
				disableDeleteOneActions={deleteOneActionsDisabled}
				deleteOnlyThisTranscriptLoading={deleteOnlyThisTranscriptLoading}
				deleteAllUsingRecordingLoading={deleteAllUsingRecordingLoading}
				onDeleteOnlyThisTranscript={handleDeleteOnlyThisTranscript}
				onDeleteAllUsingRecording={handleDeleteAllUsingRecording}
			/>

			<HistoryAnalysisPanel
				analysisOpened={analysisOpened}
				onCloseAnalysis={analysisHandlers.close}
				analysisIncludedCount={analysisIncludedCount}
				analysisEstimatedTokens={analysisEstimatedTokens}
				analysisAvailableTranscriptsCount={analysisAvailableTranscriptsCount}
				analysisIncludeFromLastHoursInput={analysisIncludeFromLastHoursInput}
				onAnalysisIncludeFromLastHoursInputChange={
					setAnalysisIncludeFromLastHoursInput
				}
				analysisPromptStyle={analysisPromptStyle}
				onAnalysisPromptStyleChange={setAnalysisPromptStyle}
				onGenerateAnalysisPrompt={handleGenerateAnalysisPrompt}
				isAnalysisLoading={analysisOpened && allHistoryQuery.isLoading}
				analysisPrompt={analysisPrompt}
				onAnalysisPromptChange={setAnalysisPrompt}
				onCopyAnalysisPrompt={() => clipboard.copy(analysisPrompt)}
				hasAnyLlmProviders={hasAnyLlmProviders}
				onOpenSendDrawer={handleOpenSendDrawer}
				sendDrawerOpened={sendDrawerOpened}
				onCloseSendDrawer={sendDrawerHandlers.close}
				isNarrow={Boolean(isNarrow)}
				llmProviders={llmProviders}
				sendProvider={sendProvider}
				onSendProviderChange={handleSendProviderChange}
				sendModel={sendModel}
				onSendModelChange={setSendModel}
				sendProviderUsed={sendProviderUsed}
				sendModelUsed={sendModelUsed}
				onGenerateSendOutput={handleGenerateSendOutput}
				isSendPending={sendToLlmMutation.isPending}
				sendOutput={sendOutput}
				onSendOutputChange={setSendOutput}
				onCopySendOutput={() => clipboard.copy(sendOutput)}
			/>

			<HistoryFeedList
				isInitialLoading={isInitialLoading}
				hasError={Boolean(error)}
				emptyState={emptyState}
				groupedHistory={groupedHistory}
				copiedEntryId={historyOrchestration.copiedEntryId}
				onCopyEntry={historyOrchestration.handleCopyEntry}
				onRetryEntry={handleRetryEntry}
				isRetryPending={retryMutation.isPending}
				retryPendingEntryId={retryPendingEntryId}
				recordingExistsById={historyOrchestration.recordingExistsById}
				player={player}
				requestLogIds={requestLogIds}
				onJumpToLog={onJumpToLog}
				onDeleteEntry={handleDelete}
				isDeleteDisabled={
					deleteHistoryEntryEx.isPending || historyOrchestration.deleteOneBusy
				}
			/>
		</div>
	);
}
