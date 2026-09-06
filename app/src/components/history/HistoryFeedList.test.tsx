// @vitest-environment happy-dom
import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { groupHistoryForDisplay } from "../../lib/history/readModel";
import { tauriAPI } from "../../lib/tauri";
import type { RecordingPlayerControls } from "../../lib/useRecordingPlayer";
import { HistoryFeedList } from "./HistoryFeedList";

vi.mock("wavesurfer.js", () => ({
	default: { create: vi.fn(() => ({ destroy: vi.fn(), on: vi.fn() })) },
}));
vi.mock("../../lib/tauri", () => ({
	tauriAPI: {
		getHistoryDetail: vi.fn(),
		saveHistoryEdit: vi.fn(),
		emitHistoryChanged: vi.fn(async () => {}),
	},
}));
const original = "A complete transcript. ".repeat(100);
let host: HTMLDivElement;
let root: Root;
let client: QueryClient;
const player = {
	id: "one",
	loading: false,
	playing: false,
	position: 0,
	duration: 100,
	rate: 1,
	waveform: { duration_seconds: 100, peaks: [-1, 1] },
	error: null,
	media: null,
	prepare: vi.fn(async () => {}),
	toggle: vi.fn(async () => {}),
	seek: vi.fn(),
	setRate: vi.fn(),
	stop: vi.fn(),
	isPlaying: () => false,
	isLoading: () => false,
} satisfies RecordingPlayerControls;
const copy = vi.fn();
async function settle() {
	await act(async () => {
		await vi.advanceTimersByTimeAsync(1);
	});
}
async function click(element: Element | null) {
	expect(element).not.toBeNull();
	await act(async () => (element as HTMLElement).click());
	await settle();
}
function button(text: string) {
	return (
		[...document.querySelectorAll("button")].find((b) =>
			b.textContent?.includes(text),
		) ?? null
	);
}
beforeEach(async () => {
	vi.useFakeTimers();
	vi.clearAllMocks();
	Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
	client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
	vi.mocked(tauriAPI.getHistoryDetail).mockImplementation(async (id) => ({
		entry: {
			id,
			text: original,
			timestamp: "2026-01-01T00:00:00Z",
			status: "success",
		},
		original_text: original,
		revision: 0,
		edited: false,
		edit_error: null,
	}));
	vi.mocked(tauriAPI.saveHistoryEdit).mockImplementation(async (input) => ({
		title: input.title,
		text: input.text,
		revision: input.expected_revision + 1,
	}));
	host = document.createElement("div");
	document.body.append(host);
	root = createRoot(host);
	await act(async () =>
		root.render(
			<QueryClientProvider client={client}>
				<MantineProvider env="test">
					<HistoryFeedList
						isInitialLoading={false}
						hasError={false}
						emptyState={null}
						groupedHistory={groupHistoryForDisplay(
							["one", "two"].map((id) => ({
								id,
								title: id,
								timestamp: "2026-01-01T00:00:00Z",
								text: "Short preview",
							})),
						)}
						copiedEntryId={null}
						onCopyEntry={copy}
						onRetryEntry={vi.fn()}
						isRetryPending={false}
						recordingExistsById={new Map()}
						player={player}
						requestLogIds={new Set()}
						onDeleteEntry={vi.fn()}
						isDeleteDisabled={false}
					/>
				</MantineProvider>
			</QueryClientProvider>,
		),
	);
	await settle();
});
afterEach(async () => {
	await act(async () => root.unmount());
	client.clear();
	host.remove();
	vi.useRealTimers();
});

describe("History cards and reader", () => {
	it("expands a card instead of copying, loads full text on demand, and only keeps one open", async () => {
		expect(tauriAPI.getHistoryDetail).not.toHaveBeenCalled();
		await click(document.querySelector(".history-preview"));
		expect(document.querySelectorAll("[data-history-detail]")).toHaveLength(1);
		expect(copy).not.toHaveBeenCalled();
		expect(player.toggle).not.toHaveBeenCalled();
		expect(
			document.querySelector(".history-transcript-inline")?.textContent,
		).toBe(original);
		await click(document.querySelectorAll(".history-card-toggle")[1] ?? null);
		expect(document.querySelectorAll("[data-history-detail]")).toHaveLength(1);
		expect(document.querySelector("#history-detail-two")).not.toBeNull();
		expect(player.stop).toHaveBeenCalled();
		await click(document.querySelectorAll(".history-card-toggle")[1] ?? null);
		expect(document.querySelector("[data-history-detail]")).toBeNull();
	});
	it("copies full text explicitly and offers the full reader without starting playback", async () => {
		await click(document.querySelector('[aria-label="Copy transcript"]'));
		expect(copy).toHaveBeenCalledWith("one", original);
		await click(document.querySelector(".history-card-toggle"));
		await click(button("Open full view"));
		expect(document.querySelector('[role="dialog"]')).not.toBeNull();
		expect(
			document.querySelector('[aria-label="Full transcript"]')?.textContent,
		).toBe(original);
		expect(
			document.querySelector('[aria-label="Search transcript"]'),
		).not.toBeNull();
		expect(button("Edit")).not.toBeNull();
		expect(player.toggle).not.toHaveBeenCalled();
		// Portal events bubble through React's tree, but must not toggle the card.
		await click(document.querySelector('[aria-label="Full transcript"]'));
		expect(document.querySelector('[role="dialog"]')).not.toBeNull();
		expect(document.querySelector("#history-detail-one")).not.toBeNull();
		await click(document.querySelector(".mantine-Modal-close"));
		expect(player.stop).toHaveBeenCalled();
	});
	it("flushes corrections when closing and keeps the reader open if saving fails", async () => {
		await click(document.querySelector(".history-card-toggle"));
		await click(button("Open full view"));
		await click(button("Edit"));
		const input = document.querySelector(
			'[aria-label="Edit transcript"]',
		) as HTMLTextAreaElement;
		await act(async () => {
			Object.getOwnPropertyDescriptor(
				HTMLTextAreaElement.prototype,
				"value",
			)?.set?.call(input, "Corrected meeting");
			input.dispatchEvent(new Event("input", { bubbles: true }));
		});
		vi.mocked(tauriAPI.saveHistoryEdit).mockRejectedValueOnce(
			new Error("Disk unavailable"),
		);
		await click(document.querySelector(".mantine-Modal-close"));
		expect(tauriAPI.saveHistoryEdit).toHaveBeenCalledWith(
			expect.objectContaining({
				text: "Corrected meeting",
				expected_revision: 0,
			}),
		);
		expect(document.querySelector('[role="dialog"]')).not.toBeNull();
		expect(input.value).toBe("Corrected meeting");
		expect(document.body.textContent).toContain("Your draft is still here");
		await click(document.querySelector(".mantine-Modal-close"));
		expect(tauriAPI.saveHistoryEdit).toHaveBeenCalledTimes(2);
		expect(document.querySelector('[role="dialog"]')).toBeNull();
	});
});
