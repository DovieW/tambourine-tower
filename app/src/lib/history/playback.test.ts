import { describe, expect, it, vi } from "vitest";
import { RecordingPlayback } from "./playback";

class FakeAudio extends EventTarget {
	src = "";
	currentTime = 0;
	playbackRate = 1;
	preload = "";
	paused = true;
	load = vi.fn();
	play = vi.fn(async () => {
		this.paused = false;
		this.dispatchEvent(new Event("playing"));
	});
	pause() {
		this.paused = true;
		this.dispatchEvent(new Event("pause"));
	}
	getAttribute() {
		return this.src || null;
	}
	removeAttribute() {
		this.src = "";
	}
}
function deferred<T>() {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((r) => {
		resolve = r;
	});
	return { promise, resolve };
}
function setup(getUrl = async (id: string) => `asset:${id}`) {
	const audio = new FakeAudio();
	const player = new RecordingPlayback({
		createAudio: () => audio as unknown as HTMLAudioElement,
		getUrl,
		getWaveform: async () => ({ duration_seconds: 14400, peaks: [-1, 1] }),
	});
	return { audio, player };
}
describe("shared History playback", () => {
	it("never autoplays, supports seeking/speed, and retains each position", async () => {
		const { player, audio } = setup();
		await player.prepare("one");
		expect(audio.play).not.toHaveBeenCalled();
		player.seek(4000);
		player.setRate(1.5);
		await player.toggle("one");
		expect(audio.playbackRate).toBe(1.5);
		expect(player.snapshot().playing).toBe(true);
		player.stop();
		expect(audio.paused).toBe(true);
		await player.prepare("two");
		player.seek(90000);
		expect(player.snapshot().position).toBe(14400);
		await player.prepare("one");
		audio.dispatchEvent(new Event("loadedmetadata"));
		expect(player.snapshot().position).toBe(4000);
	});
	it("a collapsed pending Play request cannot start after loading", async () => {
		const url = deferred<string>();
		const { player, audio } = setup(() => url.promise);
		const playing = player.toggle("one");
		player.stop();
		url.resolve("asset:one");
		await playing;
		expect(audio.play).not.toHaveBeenCalled();
		expect(audio.src).toBe("");
	});
	it("a stale recording load cannot replace a newer selection", async () => {
		const url = deferred<string>();
		const { player, audio } = setup((id) =>
			id === "one" ? url.promise : Promise.resolve("asset:two"),
		);
		const first = player.prepare("one");
		await player.prepare("two");
		url.resolve("asset:one");
		await first;
		expect(audio.src).toBe("asset:two");
		expect(player.snapshot().id).toBe("two");
		player.dispose();
		audio.dispatchEvent(new Event("playing"));
		expect(player.snapshot().playing).toBe(false);
	});
	it("keeps useful missing audio errors without a timeout or autoplay", async () => {
		const { player, audio } = setup(async () => {
			throw new Error("gone");
		});
		await player.prepare("one");
		expect(player.snapshot().error).toContain("Could not load");
		expect(audio.play).not.toHaveBeenCalled();
	});
});
