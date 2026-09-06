import { afterEach, describe, expect, it, vi } from "vitest";
import type { HistoryDetail, HistoryEditInput } from "../tauri/types";
import { HistoryDocument, transcriptMatches } from "./document";

const detail: HistoryDetail = {
	entry: {
		id: "recording",
		text: "Original",
		timestamp: "2026-01-01",
		status: "success",
	},
	original_text: "Original",
	revision: 0,
	edited: false,
	edit_error: null,
};
function setup() {
	const save = vi.fn(async (input: HistoryEditInput) => ({
		revision: input.expected_revision + 1,
		title: input.title,
		text: input.text,
	}));
	return { save, document: new HistoryDocument(detail, save) };
}
afterEach(() => vi.useRealTimers());
describe("History correction ownership", () => {
	it("debounces edits, flushes on close, and restores original without resetting revision", async () => {
		vi.useFakeTimers();
		const { save, document } = setup();
		document.change({ text: "First" });
		document.change({ text: "Second", title: "Meeting" });
		await vi.advanceTimersByTimeAsync(599);
		expect(save).not.toHaveBeenCalled();
		await vi.advanceTimersByTimeAsync(1);
		expect(save).toHaveBeenCalledTimes(1);
		document.change({ text: "Third" });
		await document.flush();
		expect(document.snapshot().revision).toBe(2);
		await document.restoreOriginal();
		expect(document.snapshot().text).toBe("Original");
		expect(save.mock.lastCall?.[0]).toMatchObject({
			text: null,
			expected_revision: 2,
			title: "Meeting",
		});
	});
	it("saves within five seconds during continuous typing", async () => {
		vi.useFakeTimers();
		const { save, document } = setup();
		for (let n = 0; n < 10; n++) {
			document.change({ text: String(n) });
			await vi.advanceTimersByTimeAsync(500);
		}
		expect(save).toHaveBeenCalledTimes(1);
		expect(document.snapshot().status).toBe("saved");
	});
	it("preserves drafts and revisions on disk failure or conflict", async () => {
		const { save, document } = setup();
		save.mockRejectedValueOnce(new Error("HISTORY_EDIT_CONFLICT"));
		document.change({ text: "Keep this draft" });
		expect(await document.flush()).toBe(false);
		expect(document.snapshot()).toMatchObject({
			text: "Keep this draft",
			revision: 0,
			status: "error",
		});
		expect(document.dirty).toBe(true);
		expect(await document.flush()).toBe(true);
	});
	it("serializes writes while preserving newer typing during an in-flight save", async () => {
		let finish!: () => void;
		const { save, document } = setup();
		save.mockImplementationOnce(async (input) => {
			await new Promise<void>((resolve) => {
				finish = resolve;
			});
			return { ...input, revision: 1 };
		});
		document.change({ text: "First" });
		const flush = document.flush();
		document.change({ text: "Newer" });
		expect(save).toHaveBeenCalledTimes(1);
		finish();
		await flush;
		expect(save).toHaveBeenCalledTimes(2);
		expect(save.mock.lastCall?.[0]).toMatchObject({
			text: "Newer",
			expected_revision: 1,
		});
		expect(document.snapshot()).toMatchObject({
			text: "Newer",
			revision: 2,
			status: "saved",
		});
	});
	it("only overwrites a newer correction after explicit conflict resolution", async () => {
		const { save, document } = setup();
		save.mockRejectedValueOnce(new Error("HISTORY_EDIT_CONFLICT"));
		document.change({ text: "My draft" });
		await document.flush();
		expect(document.snapshot().revision).toBe(0);
		expect(
			await document.resolveConflict(async () => ({
				...detail,
				entry: { ...detail.entry, text: "Other window's correction" },
				revision: 4,
			})),
		).toBe(true);
		expect(save.mock.lastCall?.[0]).toMatchObject({
			text: "My draft",
			expected_revision: 4,
		});
		expect(document.snapshot()).toMatchObject({
			text: "My draft",
			revision: 5,
			status: "saved",
		});
	});
	it("searches literal Unicode and bounds repetitive long documents", () => {
		expect(transcriptMatches("[hello] HELLO 語", "[hello]")).toEqual([
			{ start: 0, end: 7 },
		]);
		expect(transcriptMatches("[hello] HELLO 語", "hello")).toHaveLength(2);
		expect(transcriptMatches("語".repeat(20000), "語")).toHaveLength(10000);
	});
	it("discards pending autosaves after the owning recording is deleted", async () => {
		vi.useFakeTimers();
		const { save, document } = setup();
		document.change({ text: "Pending correction" });
		document.discard();
		document.change({ text: "Stale input event" });
		await vi.advanceTimersByTimeAsync(6000);
		await document.flush();
		expect(document.dirty).toBe(false);
		expect(save).not.toHaveBeenCalled();
	});
});
