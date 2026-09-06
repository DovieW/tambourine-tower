import type { RecordingWaveform } from "../tauri/types";

type PlaybackState = {
	id: string | null;
	loading: boolean;
	playing: boolean;
	position: number;
	duration: number;
	rate: number;
	waveform: RecordingWaveform | null;
	error: string | null;
};

/** Owns async source changes independently of card/modal mounts. No blob cache,
 * autoplay, or wall-clock timeout. Stale loads may never change the active source. */
export class RecordingPlayback {
	media: HTMLAudioElement | null = null;
	private generation = 0;
	private playIntent = 0;
	private positions = new Map<string, number>();
	private listeners = new Set<() => void>();
	private state: PlaybackState = {
		id: null,
		loading: false,
		playing: false,
		position: 0,
		duration: 0,
		rate: 1,
		waveform: null,
		error: null,
	};
	constructor(
		private dependencies: {
			createAudio: () => HTMLAudioElement;
			getUrl: (id: string) => Promise<string | null>;
			getWaveform: (id: string) => Promise<RecordingWaveform | null>;
			onError?: (message: string) => void;
		},
	) {}
	snapshot = () => this.state;
	subscribe = (listener: () => void) => {
		this.listeners.add(listener);
		return () => {
			this.listeners.delete(listener);
		};
	};
	private update(patch: Partial<PlaybackState>) {
		this.state = { ...this.state, ...patch };
		for (const listener of this.listeners) listener();
	}
	private ensureAudio() {
		if (this.media) return this.media;
		const audio = this.dependencies.createAudio();
		audio.preload = "metadata";
		audio.addEventListener("timeupdate", () => {
			if (this.media !== audio || !audio.getAttribute("src")) return;
			if (this.state.id) this.positions.set(this.state.id, audio.currentTime);
			this.update({ position: audio.currentTime });
		});
		audio.addEventListener("playing", () => {
			if (this.media === audio) this.update({ playing: true });
		});
		audio.addEventListener("pause", () => {
			if (this.media === audio) this.update({ playing: false });
		});
		audio.addEventListener("ended", () => {
			if (this.media === audio) this.update({ playing: false });
		});
		audio.addEventListener("loadedmetadata", () => {
			if (this.media !== audio) return;
			if (this.state.id) this.seek(this.positions.get(this.state.id) ?? 0);
		});
		audio.addEventListener("error", () => {
			if (this.media === audio && audio.getAttribute("src"))
				this.fail(
					"This audio could not be played. The saved recording has not been changed.",
				);
		});
		this.media = audio;
		return audio;
	}
	private fail(message: string) {
		this.update({ loading: false, playing: false, error: message });
		this.dependencies.onError?.(message);
	}
	prepare = async (id: string) => {
		if (
			this.state.id === id &&
			this.media?.getAttribute("src") &&
			!this.state.error
		)
			return;
		this.stop();
		const generation = ++this.generation;
		const audio = this.ensureAudio();
		audio.removeAttribute("src");
		audio.load();
		this.update({
			id,
			loading: true,
			error: null,
			waveform: null,
			duration: 0,
			position: this.positions.get(id) ?? 0,
		});
		try {
			const [url, waveform] = await Promise.all([
				this.dependencies.getUrl(id),
				this.dependencies.getWaveform(id),
			]);
			if (generation !== this.generation) return;
			if (!url || !waveform) {
				this.fail("No saved audio is available for this recording.");
				return;
			}
			audio.src = url;
			audio.playbackRate = this.state.rate;
			this.update({
				waveform,
				duration: waveform.duration_seconds,
				loading: false,
			});
			audio.load();
		} catch {
			if (generation === this.generation)
				this.fail("Could not load this recording. Try opening it again.");
		}
	};
	toggle = async (id: string) => {
		if (this.state.loading) return;
		const preparation =
			this.state.id !== id ||
			!this.media?.getAttribute("src") ||
			this.state.error
				? this.prepare(id)
				: Promise.resolve();
		const intent = ++this.playIntent;
		await preparation;
		if (
			intent !== this.playIntent ||
			this.state.id !== id ||
			this.state.loading ||
			this.state.error ||
			!this.media?.getAttribute("src")
		)
			return;
		if (!this.media.paused) {
			this.media.pause();
			return;
		}
		const generation = this.generation;
		try {
			await this.media.play();
			if (generation === this.generation) this.update({ playing: true });
		} catch {
			if (generation === this.generation)
				this.fail("Playback could not start. Press Play to try again.");
		}
	};
	seek = (seconds: number) => {
		if (!Number.isFinite(seconds) || !this.media || !this.state.duration)
			return;
		const position = Math.max(0, Math.min(seconds, this.state.duration));
		try {
			this.media.currentTime = position;
		} catch {
			return;
		}
		if (this.state.id) this.positions.set(this.state.id, position);
		this.update({ position });
	};
	setRate = (rate: number) => {
		if (![0.5, 0.75, 1, 1.25, 1.5, 2].includes(rate)) return;
		if (this.media) this.media.playbackRate = rate;
		this.update({ rate });
	};
	stop = () => {
		++this.playIntent;
		++this.generation;
		this.media?.pause();
		this.update({ playing: false, loading: false });
	};
	dispose = () => {
		this.stop();
		this.media?.removeAttribute("src");
		this.media?.load();
		this.media = null;
	};
}
