import { Store } from "@tauri-apps/plugin-store";
import { format, isToday, isYesterday } from "date-fns";
import { listAllLlmModelKeys, listAllSttModelKeys } from "../modelOptions";
import type { HistoryEntry } from "../tauri/types";

export const HISTORY_FILTERS_STORE_FILE = "ui.json";
export const HISTORY_FILTERS_STORE_KEY = "history_feed_filters_v1";

export type PersistedHistoryFilters = {
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

export function normalizePersistedHistoryFilters(
	value: unknown,
	knownKeys: {
		stt?: Iterable<string>;
		llm?: Iterable<string>;
	} = {},
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
	// selections that it can't render/unselect after provider/model catalogs change.
	const knownSttKeys = new Set(
		knownKeys.stt ?? listAllSttModelKeys().map((option) => option.key),
	);
	const knownLlmKeys = new Set(
		knownKeys.llm ?? listAllLlmModelKeys().map((option) => option.key),
	);

	const selectedSttModelKeys = rawSelectedSttModelKeys.filter((key) =>
		knownSttKeys.has(key),
	);
	const selectedLlmModelKeys = rawSelectedLlmModelKeys.filter((key) =>
		knownLlmKeys.has(key),
	);

	return {
		filterText,
		showFailed,
		showEmptyTranscript,
		selectedSttModelKeys,
		selectedLlmModelKeys,
	};
}

export async function readPersistedHistoryFilters(): Promise<PersistedHistoryFilters | null> {
	const store = await getHistoryFiltersStore();
	const raw = await store.get(HISTORY_FILTERS_STORE_KEY);
	return normalizePersistedHistoryFilters(raw);
}

export async function writePersistedHistoryFilters(
	filters: PersistedHistoryFilters,
): Promise<void> {
	const store = await getHistoryFiltersStore();
	await store.set(HISTORY_FILTERS_STORE_KEY, filters);
	await store.save();
}

export const HISTORY_PAGE_SIZE = 25;

export function formatHistoryTime(timestamp: string): string {
	return format(new Date(timestamp), "h:mm a");
}

export function formatHistoryDate(timestamp: string): string {
	const date = new Date(timestamp);
	if (isToday(date)) return "Today";
	if (isYesterday(date)) return "Yesterday";
	return format(date, "MMM d");
}

export interface GroupedHistory {
	date: string;
	items: HistoryEntry[];
}

export type HistoryEntryContentKind =
	| "in_progress"
	| "error"
	| "text"
	| "empty";

export interface HistoryEntryViewModel {
	id: string;
	timestampLabel: string;
	contentKind: HistoryEntryContentKind;
	displayText: string;
	displayTitle?: string;
	copyValue: string | null;
	hasCopyValue: boolean;
	profilePresetLabel: string | null;
	recordingRequestId: string | null;
	title?: string;
	durationSeconds?: number | null;
}

export interface GroupedHistoryViewModel {
	date: string;
	items: HistoryEntryViewModel[];
}

export interface HistoryFeedEmptyState {
	title: string;
	message: string;
}

function trimOrNull(value: string | null | undefined): string | null {
	const trimmed = (value ?? "").trim();
	return trimmed.length > 0 ? trimmed : null;
}

export function groupHistoryByDate(history: HistoryEntry[]): GroupedHistory[] {
	const groups: Record<string, GroupedHistory> = {};

	for (const item of history) {
		const dateKey = formatHistoryDate(item.timestamp);
		if (!groups[dateKey]) {
			groups[dateKey] = { date: dateKey, items: [] };
		}
		groups[dateKey].items.push(item);
	}

	return Object.values(groups);
}

export function getHistoryEntryProfileBadgeLabel(
	entry: Pick<
		HistoryEntry,
		"profile_name" | "profile_id" | "preset_name" | "preset_id"
	>,
): string | null {
	const profileName = trimOrNull(entry.profile_name);
	const profileId = trimOrNull(entry.profile_id);
	const presetName = trimOrNull(entry.preset_name);
	const presetId = trimOrNull(entry.preset_id);

	const profileLabel =
		profileName ??
		(!profileId || profileId === "default" ? "Default" : profileId);
	const presetLabel = presetName ?? presetId ?? "Default";
	const isDefaultProfile =
		profileId === "default" || profileLabel.toLowerCase() === "default";

	if (isDefaultProfile && presetLabel.toLowerCase() === "default") {
		return null;
	}

	return `${profileLabel}: ${presetLabel}`;
}

export function toHistoryEntryViewModel(
	entry: HistoryEntry,
): HistoryEntryViewModel {
	const status = entry.status ?? "success";
	const errorMessage = trimOrNull(entry.error_message);
	const transcript = trimOrNull(entry.text);
	const recordingRequestId = trimOrNull(entry.recording_request_id) ?? entry.id;
	const metadata = {
		title:
			entry.title ||
			(entry.recording_mode === "meeting" ? "Meeting" : "Voice recording"),
		durationSeconds: entry.duration_seconds,
	};

	if (status === "in_progress") {
		return {
			...metadata,
			id: entry.id,
			timestampLabel: formatHistoryTime(entry.timestamp),
			contentKind: "in_progress",
			displayText: "Transcribing…",
			copyValue: null,
			hasCopyValue: false,
			profilePresetLabel: getHistoryEntryProfileBadgeLabel(entry),
			recordingRequestId,
		};
	}

	if (status === "error") {
		return {
			...metadata,
			id: entry.id,
			timestampLabel: formatHistoryTime(entry.timestamp),
			contentKind: "error",
			displayText: errorMessage ?? "Try again",
			displayTitle: errorMessage ?? undefined,
			copyValue: errorMessage,
			hasCopyValue: Boolean(errorMessage),
			profilePresetLabel: getHistoryEntryProfileBadgeLabel(entry),
			recordingRequestId,
		};
	}

	if (!transcript) {
		return {
			...metadata,
			id: entry.id,
			timestampLabel: formatHistoryTime(entry.timestamp),
			contentKind: "empty",
			displayText: "No transcript",
			displayTitle: "No transcript was produced",
			copyValue: null,
			hasCopyValue: false,
			profilePresetLabel: getHistoryEntryProfileBadgeLabel(entry),
			recordingRequestId,
		};
	}

	return {
		id: entry.id,
		...metadata,
		timestampLabel: formatHistoryTime(entry.timestamp),
		contentKind: "text",
		displayText: entry.text,
		copyValue: entry.text,
		hasCopyValue: true,
		profilePresetLabel: getHistoryEntryProfileBadgeLabel(entry),
		recordingRequestId,
	};
}

export function groupHistoryForDisplay(
	history: HistoryEntry[],
): GroupedHistoryViewModel[] {
	return groupHistoryByDate(history).map((group) => ({
		date: group.date,
		items: group.items.map((item) => toHistoryEntryViewModel(item)),
	}));
}

export function getHistoryFeedEmptyState(args: {
	totalHistoryCount: number;
	isFiltering: boolean;
}): HistoryFeedEmptyState {
	if (args.totalHistoryCount === 0) {
		return {
			title: "No dictation history yet",
			message:
				"Your transcribed text will appear here after you use voice dictation.",
		};
	}

	if (args.isFiltering) {
		return {
			title: "No matches",
			message: "Try a different filter.",
		};
	}

	return {
		title: "Nothing to show",
		message: "Start your first recording to see it here.",
	};
}

export function estimateTokenCount(text: string): number {
	// Heuristic: ~4 characters per token for English-ish text. Good enough for an
	// on-screen estimate; provider billing still uses backend telemetry.
	const normalized = (text ?? "").trim();
	if (!normalized) return 0;
	return Math.max(1, Math.ceil(normalized.length / 4));
}

export type AnalysisPromptStyle = "productive" | "insightful" | "structured";

export function analysisStyleLabel(style: AnalysisPromptStyle): string {
	switch (style) {
		case "productive":
			return "Productive";
		case "insightful":
			return "Insightful";
		case "structured":
			return "Structured";
	}
}

export function buildAnalysisSystemPrompt(style: AnalysisPromptStyle): string {
	switch (style) {
		case "productive":
			return (
				"You are an expert assistant. Analyze the following voice dictation transcripts." +
				"\n\nGoals:" +
				"\n- Identify recurring themes, priorities, open questions, and next actions." +
				"\n- Produce a concise summary and a structured list of action items." +
				"\n- Call out contradictions, missing context, and risks." +
				"\n\nOutput format:" +
				"\n1) Executive summary (5-10 bullets)" +
				"\n2) Themes (grouped)" +
				"\n3) Action items (with suggested owners + priority)" +
				"\n4) Open questions" +
				"\n5) Notable quotes (optional)"
			);
		case "insightful":
			return (
				"You are an insightful analyst. Read the transcripts and infer intent, context, and patterns." +
				"\n\nFocus:" +
				"\n- Hidden assumptions and recurring frustrations" +
				"\n- Opportunities, risks, and what to do next" +
				"\n- What seems important but unstated" +
				"\n\nOutput format:" +
				"\n1) Key insights (bullets)" +
				"\n2) Themes & evidence (quotes or references)" +
				"\n3) Recommendations" +
				"\n4) Open questions"
			);
		case "structured":
			return (
				"You are a meticulous organizer. Turn these transcripts into a clean plan." +
				"\n\nRules:" +
				"\n- Be concise." +
				"\n- Use headings and bullet lists." +
				"\n- Prefer concrete next steps." +
				"\n\nOutput format:" +
				"\n## Summary" +
				"\n## Goals" +
				"\n## Tasks (priority-ordered)" +
				"\n## Decisions needed" +
				"\n## Questions"
			);
	}
}

export function buildTranscriptsUserPrompt(args: {
	transcripts: Array<{ timestamp: string; text: string }>;
}): string {
	const lines: string[] = [];
	lines.push("---\nTRANSCRIPTS\n---");
	args.transcripts.forEach((entry, idx) => {
		const ts = format(new Date(entry.timestamp), "yyyy-MM-dd HH:mm");
		lines.push(`\n[Recording ${idx + 1} • ${ts}]\n${entry.text}`);
	});
	return lines.join("\n");
}

export function buildAnalysisPrompt(
	history: Array<Pick<HistoryEntry, "id" | "text" | "timestamp" | "status">>,
	options?: {
		includeFromLastHours?: number | null;
		style?: AnalysisPromptStyle;
		nowMs?: number;
	},
): {
	prompt: string;
	systemPrompt: string;
	userPrompt: string;
	includedCount: number;
	totalCount: number;
	availableTranscriptsCount: number;
} {
	const totalCount = history.length;
	const style: AnalysisPromptStyle = options?.style ?? "productive";

	const allTranscripts = history
		.filter((entry) => (entry.status ?? "success") === "success")
		.map((entry) => ({ ...entry, text: (entry.text ?? "").trim() }))
		.filter((entry) => entry.text.length > 0);

	const availableTranscriptsCount = allTranscripts.length;

	const includeFromLastHours = options?.includeFromLastHours;
	const cutoffMs =
		typeof includeFromLastHours === "number" &&
		Number.isFinite(includeFromLastHours) &&
		includeFromLastHours > 0
			? (options?.nowMs ?? Date.now()) - includeFromLastHours * 60 * 60 * 1000
			: null;

	const filtered =
		typeof cutoffMs === "number"
			? allTranscripts.filter(
					(transcript) => new Date(transcript.timestamp).getTime() >= cutoffMs,
				)
			: allTranscripts;

	const transcripts = [...filtered].sort(
		(a, b) => new Date(a.timestamp).getTime() - new Date(b.timestamp).getTime(),
	);

	const includedCount = transcripts.length;
	const systemPrompt = buildAnalysisSystemPrompt(style);
	const userPrompt = buildTranscriptsUserPrompt({
		transcripts: transcripts.map((transcript) => ({
			timestamp: transcript.timestamp,
			text: transcript.text,
		})),
	});

	const promptParts = [systemPrompt, userPrompt];
	if (includedCount === 0) {
		promptParts.push(
			"(No non-empty transcripts matched your filter. Record something first, then try again.)",
		);
	}

	return {
		prompt: promptParts.join("\n\n"),
		systemPrompt,
		userPrompt,
		includedCount,
		totalCount,
		availableTranscriptsCount,
	};
}
