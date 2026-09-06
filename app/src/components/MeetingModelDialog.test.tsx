// @vitest-environment happy-dom
import { MantineProvider } from "@mantine/core";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { MeetingModelDialog } from "./MeetingModelDialog";

const state = vi.hoisted(() => ({ managed: false, openai: false }));
vi.mock("../lib/queries/providers", () => ({
	useAvailableProviders: () => ({
		data: {
			stt: state.openai
				? [{ value: "openai", label: "OpenAI" }]
				: state.managed
					? []
					: [{ value: "whisper", label: "Local Whisper" }],
		},
	}),
	useWhisperModels: () => ({
		data: [
			{ id: "base", name: "Base", is_downloaded: true },
			{ id: "small", name: "Small", is_downloaded: true },
			{ id: "large", name: "Large", is_downloaded: false },
		],
	}),
	useManagedModels: () => ({
		isSuccess: true,
		data: state.openai
			? [
					{
						provider: "openai",
						id: "gpt-4o-transcribe-diarize",
						display_name: "Speaker model",
						capabilities: ["transcription"],
					},
				]
			: [],
	}),
}));
vi.mock("../lib/queries/license", () => ({
	useLicenseAuthContext: () => ({ data: {} }),
}));
vi.mock("../lib/tauri/managedInference", () => ({
	hasManagedInferenceAccess: () => state.managed,
}));
let host: HTMLDivElement;
let root: Root;
const save = vi.fn();
beforeEach(() => {
	state.managed = false;
	state.openai = false;
	save.mockClear();
	Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
	host = document.createElement("div");
	document.body.append(host);
	root = createRoot(host);
});
afterEach(async () => {
	await act(async () => root.unmount());
	host.remove();
});
async function render() {
	await act(async () =>
		root.render(
			<MantineProvider env="test">
				<MeetingModelDialog
					preferences={{ mode: "meeting", meeting_model: null }}
					onSave={save}
					onClose={() => {}}
					saving={false}
				/>
			</MantineProvider>,
		),
	);
}
it("lets meetings select another downloaded local model without changing dictation", async () => {
	await render();
	await act(async () =>
		(
			document.querySelector('[placeholder="Choose a model"]') as HTMLElement
		).click(),
	);
	const options = [...document.querySelectorAll('[role="option"]')];
	expect(options.map((option) => option.textContent)).toEqual([
		"Base · Local Whisper",
		"Small · Local Whisper",
	]);
	await act(async () => (options[1] as HTMLElement).click());
	await act(async () =>
		[...document.querySelectorAll("button")]
			.find((button) => button.textContent === "Save")
			?.click(),
	);
	expect(save).toHaveBeenCalledWith({
		mode: "meeting",
		meeting_model: {
			provider: "local-whisper",
			model: "small",
			use_managed: false,
		},
	});
});
it("does not silently seed or enable a model when Edge publishes an empty managed catalog", async () => {
	state.managed = true;
	await render();
	expect(document.body.textContent).toContain("No models are available");
	expect(
		[...document.querySelectorAll("button")].find(
			(button) => button.textContent === "Save",
		)?.disabled,
	).toBe(true);
	expect(save).not.toHaveBeenCalled();
});

it("keeps managed and user-key choices distinct without depending on dictation settings", async () => {
	state.managed = true;
	state.openai = true;
	await render();
	const select = document.querySelector(
		'[placeholder="Choose a model"]',
	) as HTMLElement;
	await act(async () => select.click());
	await act(async () =>
		(
			[...document.querySelectorAll('[role="option"]')].find((option) =>
				option.textContent?.includes("· Managed"),
			) as HTMLElement
		).click(),
	);
	const submit = () =>
		[...document.querySelectorAll("button")]
			.find((button) => button.textContent === "Save")
			?.click();
	await act(async () => submit());
	expect(save).toHaveBeenLastCalledWith({
		mode: "meeting",
		meeting_model: {
			provider: "openai",
			model: "gpt-4o-transcribe-diarize",
			use_managed: true,
		},
	});
	await act(async () => select.click());
	await act(async () =>
		(
			[...document.querySelectorAll('[role="option"]')].find(
				(option) =>
					option.textContent?.includes("speaker labels") &&
					option.textContent?.includes("Your key"),
			) as HTMLElement
		).click(),
	);
	await act(async () => submit());
	expect(save).toHaveBeenLastCalledWith({
		mode: "meeting",
		meeting_model: {
			provider: "openai",
			model: "gpt-4o-transcribe-diarize",
			use_managed: false,
		},
	});
});
