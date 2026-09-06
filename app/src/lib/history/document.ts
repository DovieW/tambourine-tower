import { formatErrorMessage } from "../formatError";
import type {
	HistoryDetail,
	HistoryEdit,
	HistoryEditInput,
} from "../tauri/types";

export type DocumentState = {
	title: string;
	text: string;
	revision: number;
	status: "saved" | "pending" | "saving" | "error";
	error: string | null;
};

/** A serialized, debounced document writer. The maximum timer is not reset by
 * typing, and in-flight acknowledgements never replace a newer local draft. */
export class HistoryDocument {
	private state: DocumentState;
	private saved: { title: string; text: string };
	private listeners = new Set<() => void>();
	private debounce?: ReturnType<typeof setTimeout>;
	private maximum?: ReturnType<typeof setTimeout>;
	private running: Promise<boolean> | null = null;
	private discarded = false;
	constructor(
		readonly detail: HistoryDetail,
		private save: (input: HistoryEditInput) => Promise<HistoryEdit>,
		private onSaved: () => void = () => {},
	) {
		this.saved = { title: detail.entry.title ?? "", text: detail.entry.text };
		this.state = {
			...this.saved,
			revision: detail.revision,
			status: detail.edit_error ? "error" : "saved",
			error: detail.edit_error,
		};
	}
	snapshot = () => this.state;
	subscribe = (listener: () => void) => {
		this.listeners.add(listener);
		return () => {
			this.listeners.delete(listener);
		};
	};
	get dirty() {
		return (
			!this.discarded &&
			(this.state.title !== this.saved.title ||
				this.state.text !== this.saved.text)
		);
	}
	private update(patch: Partial<DocumentState>) {
		this.state = { ...this.state, ...patch };
		for (const listener of this.listeners) listener();
	}
	change = (patch: Partial<Pick<DocumentState, "title" | "text">>) => {
		if (this.discarded) return;
		this.update({ ...patch, status: "pending" });
		clearTimeout(this.debounce);
		this.debounce = setTimeout(() => {
			void this.flush();
		}, 600);
		this.maximum ??= setTimeout(() => {
			void this.flush();
		}, 5000);
	};
	/** Called only after the owning History entry is confirmed deleted. */
	discard = () => {
		this.discarded = true;
		this.clearTimers();
	};
	private clearTimers() {
		clearTimeout(this.debounce);
		clearTimeout(this.maximum);
		this.debounce = undefined;
		this.maximum = undefined;
	}
	flush = (): Promise<boolean> => {
		this.clearTimers();
		if (this.running) return this.running;
		if (this.detail.edit_error) return Promise.resolve(!this.dirty);
		this.running = this.write().finally(() => {
			this.running = null;
		});
		return this.running;
	};
	private async write() {
		while (this.dirty) {
			const draft = { title: this.state.title, text: this.state.text };
			this.update({ status: "saving", error: null });
			try {
				const result = await this.save({
					id: this.detail.entry.id,
					expected_revision: this.state.revision,
					title: draft.title || null,
					text: draft.text === this.detail.original_text ? null : draft.text,
				});
				if (this.discarded) return true;
				this.saved = draft;
				this.update({ revision: result.revision });
				// Cache refresh is not part of the durable save acknowledgement.
				try {
					this.onSaved();
				} catch {
					/* Keep the committed revision. */
				}
			} catch (error) {
				this.update({ status: "error", error: formatErrorMessage(error) });
				return false;
			}
		}
		this.update({ status: "saved", error: null });
		return true;
	}
	restoreOriginal = async () => {
		if (!(await this.flush())) return false;
		this.change({ text: this.detail.original_text });
		return this.flush();
	};
	/** Only called after the user explicitly chooses their draft over a newer save. */
	resolveConflict = async (load: () => Promise<HistoryDetail | null>) => {
		try {
			const latest = await load();
			if (
				!latest ||
				latest.entry.id !== this.detail.entry.id ||
				latest.edit_error
			)
				throw new Error(
					"The saved document could not be read. Your draft is still here.",
				);
			this.update({ revision: latest.revision });
			return this.flush();
		} catch (error) {
			this.update({ status: "error", error: formatErrorMessage(error) });
			return false;
		}
	};
}

/** Search uses literal text, not an operator-supplied regular expression. */
export function transcriptMatches(
	text: string,
	query: string,
): Array<{ start: number; end: number }> {
	if (!query.trim()) return [];
	const matches: Array<{ start: number; end: number }> = [];
	const pattern = new RegExp(
		query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"),
		"giu",
	);
	for (const match of text.matchAll(pattern)) {
		matches.push({ start: match.index, end: match.index + match[0].length });
		// Bound rendered search highlights even for extremely repetitive documents.
		if (matches.length === 10000) break;
	}
	return matches;
}
