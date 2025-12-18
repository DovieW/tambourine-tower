import { Loader, Tooltip } from "@mantine/core";
import { useResizeObserver, useTimeout } from "@mantine/hooks";
import { useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useDrag } from "@use-gesture/react";
import { useCallback, useEffect, useRef, useState } from "react";
import Logo from "./assets/logo.svg?react";
import { useAddHistoryEntry, useSettings, useTypeText } from "./lib/queries";
import { type ConnectionState, tauriAPI } from "./lib/tauri";
import "./app.css";

/**
 * Pipeline state machine states (matches Rust PipelineState)
 */
type PipelineState = "idle" | "recording" | "transcribing" | "error";

/**
 * Error info for user feedback
 */
interface ErrorInfo {
	message: string;
	recoverable: boolean;
}

/**
 * Parse error message to user-friendly format
 */
function parseError(error: unknown): ErrorInfo {
	const errorStr = String(error);

	// Network/API errors
	if (errorStr.includes("Network") || errorStr.includes("network")) {
		return { message: "Network error - check connection", recoverable: true };
	}
	if (errorStr.includes("timeout") || errorStr.includes("Timeout")) {
		return { message: "Request timed out - try again", recoverable: true };
	}
	if (errorStr.includes("API error") || errorStr.includes("401")) {
		return { message: "API error - check API key", recoverable: true };
	}
	if (errorStr.includes("rate limit") || errorStr.includes("429")) {
		return { message: "Rate limited - wait and retry", recoverable: true };
	}

	// Provider errors
	if (errorStr.includes("NoProvider") || errorStr.includes("No STT provider")) {
		return { message: "No STT provider configured", recoverable: true };
	}

	// Recording errors
	if (errorStr.includes("NotRecording")) {
		return { message: "Not recording", recoverable: true };
	}
	if (errorStr.includes("AlreadyRecording")) {
		return { message: "Already recording", recoverable: true };
	}
	if (errorStr.includes("RecordingTooLarge")) {
		return { message: "Recording too long", recoverable: true };
	}

	// Audio errors
	if (errorStr.includes("audio") || errorStr.includes("Audio")) {
		return { message: "Audio capture error", recoverable: true };
	}

	// Generic fallback
	return { message: "An error occurred", recoverable: true };
}

/**
 * Map pipeline state to connection state for UI compatibility
 */
function pipelineToConnectionState(state: PipelineState): ConnectionState {
	switch (state) {
		case "idle":
			return "idle";
		case "recording":
			return "recording";
		case "transcribing":
			return "processing";
		case "error":
			return "disconnected";
	}
}

/**
 * Error indicator icon component
 */
function ErrorIcon() {
	return (
		<svg
			width="20"
			height="20"
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			strokeWidth="2"
			strokeLinecap="round"
			strokeLinejoin="round"
			role="img"
			aria-label="Error"
		>
			<circle cx="12" cy="12" r="10" />
			<line x1="12" y1="8" x2="12" y2="12" />
			<line x1="12" y1="16" x2="12.01" y2="16" />
		</svg>
	);
}
function AudioVisualizer({
	isActive,
	barColor = "#eeeeee",
}: {
	isActive: boolean;
	barColor?: string;
}) {
	const canvasRef = useRef<HTMLCanvasElement>(null);
	const animationRef = useRef<number | null>(null);
	const analyserRef = useRef<AnalyserNode | null>(null);
	const streamRef = useRef<MediaStream | null>(null);

	useEffect(() => {
		if (!isActive) {
			// Cleanup when not active
			if (animationRef.current) {
				cancelAnimationFrame(animationRef.current);
				animationRef.current = null;
			}
			if (streamRef.current) {
				for (const track of streamRef.current.getTracks()) {
					track.stop();
				}
				streamRef.current = null;
			}
			analyserRef.current = null;
			return;
		}

		let mounted = true;

		const setupAudio = async () => {
			try {
				const stream = await navigator.mediaDevices.getUserMedia({
					audio: true,
				});
				if (!mounted) {
					for (const track of stream.getTracks()) {
						track.stop();
					}
					return;
				}

				streamRef.current = stream;
				const audioContext = new AudioContext();
				const source = audioContext.createMediaStreamSource(stream);
				const analyser = audioContext.createAnalyser();
				analyser.fftSize = 64;
				source.connect(analyser);
				analyserRef.current = analyser;

				const canvas = canvasRef.current;
				if (!canvas) return;
				const ctx = canvas.getContext("2d");
				if (!ctx) return;

				const draw = () => {
					if (!analyserRef.current || !mounted) return;

					const bufferLength = analyserRef.current.frequencyBinCount;
					const dataArray = new Uint8Array(bufferLength);
					analyserRef.current.getByteFrequencyData(dataArray);

					ctx.clearRect(0, 0, canvas.width, canvas.height);

					const barCount = 5;
					const barWidth = 3;
					const gap = 2;
					const totalWidth = barCount * barWidth + (barCount - 1) * gap;
					const startX = (canvas.width - totalWidth) / 2;
					const maxBarHeight = canvas.height * 0.7;

					for (let i = 0; i < barCount; i++) {
						// Sample from different parts of the frequency spectrum
						const dataIndex = Math.floor((i / barCount) * (bufferLength * 0.6));
						const value = dataArray[dataIndex] ?? 0;
						const barHeight = Math.max(4, (value / 255) * maxBarHeight);

						const x = startX + i * (barWidth + gap);
						const y = (canvas.height - barHeight) / 2;

						ctx.fillStyle = barColor;
						ctx.beginPath();
						ctx.roundRect(x, y, barWidth, barHeight, 1.5);
						ctx.fill();
					}

					animationRef.current = requestAnimationFrame(draw);
				};

				draw();
			} catch (error) {
				console.error("[AudioVisualizer] Failed to setup audio:", error);
			}
		};

		setupAudio();

		return () => {
			mounted = false;
			if (animationRef.current) {
				cancelAnimationFrame(animationRef.current);
			}
			if (streamRef.current) {
				for (const track of streamRef.current.getTracks()) {
					track.stop();
				}
			}
		};
	}, [isActive, barColor]);

	return (
		<canvas
			ref={canvasRef}
			width={48}
			height={48}
			style={{ display: isActive ? "block" : "none" }}
		/>
	);
}

function RecordingControl() {
	const queryClient = useQueryClient();
	const [pipelineState, setPipelineState] = useState<PipelineState>("idle");
	const [lastError, setLastError] = useState<ErrorInfo | null>(null);
	const [containerRef, rect] = useResizeObserver();
	const hasDragStartedRef = useRef(false);

	// Load settings (used for context but not directly in this component)
	useSettings();

	// TanStack Query hooks
	const typeTextMutation = useTypeText();
	const addHistoryEntry = useAddHistoryEntry();

	// Clear error after 5 seconds
	const { start: startErrorTimeout, clear: clearErrorTimeout } = useTimeout(
		() => {
			setLastError(null);
		},
		5000,
	);

	// Response timeout (30s for transcription)
	const { start: startResponseTimeout, clear: clearResponseTimeout } =
		useTimeout(() => {
			if (pipelineState === "transcribing") {
				// Reset to idle on timeout
				setPipelineState("idle");
				invoke("pipeline_force_reset").catch(console.error);
			}
		}, 30000);

	// Emit connection state changes to other windows
	useEffect(() => {
		const connectionState = pipelineToConnectionState(pipelineState);
		tauriAPI.emitConnectionState(connectionState);
	}, [pipelineState]);

	// Poll pipeline state periodically to stay in sync
	useEffect(() => {
		const syncState = async () => {
			try {
				const state = await invoke<string>("pipeline_get_state");
				setPipelineState(state as PipelineState);
			} catch (error) {
				console.error("[Pipeline] Failed to get state:", error);
			}
		};

		// Initial sync
		syncState();

		// Poll every 500ms
		const interval = setInterval(syncState, 500);
		return () => clearInterval(interval);
	}, []);

	// Auto-resize window to fit content
	useEffect(() => {
		if (rect.width > 0 && rect.height > 0) {
			tauriAPI.resizeOverlay(Math.ceil(rect.width), Math.ceil(rect.height));
		}
	}, [rect.width, rect.height]);

	// Start recording using the Rust pipeline
	const onStartRecording = useCallback(async () => {
		if (pipelineState !== "idle") return;

		// Clear any previous error when starting
		setLastError(null);
		clearErrorTimeout();

		try {
			await invoke("pipeline_start_recording");
			setPipelineState("recording");
		} catch (error) {
			console.error("[Pipeline] Failed to start recording:", error);
			const errorInfo = parseError(error);
			setLastError(errorInfo);
			startErrorTimeout();
		}
	}, [pipelineState, clearErrorTimeout, startErrorTimeout]);

	// Stop recording and transcribe
	const onStopRecording = useCallback(async () => {
		if (pipelineState !== "recording") return;

		try {
			setPipelineState("transcribing");
			startResponseTimeout();

			const transcript = await invoke<string>("pipeline_stop_and_transcribe");
			clearResponseTimeout();

			if (transcript) {
				// Type the transcript
				try {
					await typeTextMutation.mutateAsync(transcript);
				} catch (error) {
					console.error("[Pipeline] Failed to type text:", error);
					const errorInfo = parseError(error);
					setLastError(errorInfo);
					startErrorTimeout();
				}
				// Add to history
				addHistoryEntry.mutate(transcript);
			}

			setPipelineState("idle");
		} catch (error) {
			console.error("[Pipeline] Failed to stop and transcribe:", error);
			clearResponseTimeout();
			setPipelineState("error");

			// Show error to user
			const errorInfo = parseError(error);
			setLastError(errorInfo);
			startErrorTimeout();

			// Attempt to recover
			setTimeout(async () => {
				try {
					await invoke("pipeline_force_reset");
					setPipelineState("idle");
				} catch (resetError) {
					console.error("[Pipeline] Failed to reset:", resetError);
				}
			}, 1000);
		}
	}, [
		pipelineState,
		startResponseTimeout,
		clearResponseTimeout,
		typeTextMutation,
		addHistoryEntry,
		startErrorTimeout,
	]);

	// Hotkey event listeners
	// Listen for recording state changes from shortcuts (Rust handles the actual recording)
	useEffect(() => {
		let unlistenStart: (() => void) | undefined;
		let unlistenStop: (() => void) | undefined;

		const setup = async () => {
			// When shortcut triggers recording, just update UI state (don't call command again)
			unlistenStart = await tauriAPI.onStartRecording(() => {
				setPipelineState("recording");
			});
			unlistenStop = await tauriAPI.onStopRecording(() => {
				setPipelineState("transcribing");
				startResponseTimeout();
			});
		};

		setup();

		return () => {
			unlistenStart?.();
			unlistenStop?.();
		};
	}, [startResponseTimeout]);

	// Listen for pipeline events from Rust
	useEffect(() => {
		const unlisteners: (() => void)[] = [];

		const setup = async () => {
			unlisteners.push(
				await listen("pipeline-recording-started", () => {
					setPipelineState("recording");
				}),
			);

			unlisteners.push(
				await listen("pipeline-transcription-started", () => {
					setPipelineState("transcribing");
					startResponseTimeout();
				}),
			);

			unlisteners.push(
				await listen("pipeline-cancelled", () => {
					setPipelineState("idle");
					clearResponseTimeout();
				}),
			);

			unlisteners.push(
				await listen("pipeline-reset", () => {
					setPipelineState("idle");
					clearResponseTimeout();
				}),
			);

			// Listen for pipeline errors (e.g., transcription failures from hotkey-triggered recordings)
			unlisteners.push(
				await listen<string>("pipeline-error", (event) => {
					console.error("[Pipeline] Error from Rust:", event.payload);
					clearResponseTimeout();
					setPipelineState("error");

					const errorInfo = parseError(event.payload);
					setLastError(errorInfo);
					startErrorTimeout();

					// Attempt to recover after showing error
					setTimeout(async () => {
						try {
							await invoke("pipeline_force_reset");
							setPipelineState("idle");
						} catch (resetError) {
							console.error("[Pipeline] Failed to reset:", resetError);
						}
					}, 1000);
				}),
			);

			// Listen for successful transcription (from hotkey-triggered recordings)
			unlisteners.push(
				await listen<string>("pipeline-transcript-ready", () => {
					clearResponseTimeout();
					setPipelineState("idle");
				}),
			);
		};

		setup();

		return () => {
			for (const unlisten of unlisteners) {
				unlisten();
			}
		};
	}, [clearResponseTimeout, startResponseTimeout, startErrorTimeout]);

	// Listen for settings changes from main window
	useEffect(() => {
		let unlisten: (() => void) | undefined;

		const setup = async () => {
			unlisten = await tauriAPI.onSettingsChanged(() => {
				queryClient.invalidateQueries({ queryKey: ["settings"] });
				// Sync pipeline config when settings change
				invoke("sync_pipeline_config").catch(console.error);
			});
		};

		setup();

		return () => {
			unlisten?.();
		};
	}, [queryClient]);

	// Click handler (toggle mode)
	const handleClick = useCallback(() => {
		if (pipelineState === "recording") {
			onStopRecording();
		} else if (pipelineState === "idle") {
			onStartRecording();
		}
	}, [pipelineState, onStartRecording, onStopRecording]);

	// Drag handler using @use-gesture/react
	const bindDrag = useDrag(
		({ movement: [mx, my], first, last, memo }) => {
			if (first) {
				hasDragStartedRef.current = false;
				return false;
			}

			const distance = Math.sqrt(mx * mx + my * my);
			const DRAG_THRESHOLD = 5;

			if (!memo && distance > DRAG_THRESHOLD) {
				hasDragStartedRef.current = true;
				tauriAPI.startDragging();
				return true;
			}

			if (last) {
				hasDragStartedRef.current = false;
			}

			return memo;
		},
		{ filterTaps: true },
	);

	const isLoading = pipelineState === "transcribing";
	const isRecording = pipelineState === "recording";
	const isError = pipelineState === "error" || lastError !== null;

	// Determine button content
	const renderButtonContent = () => {
		if (isLoading) {
			return <Loader size="sm" color="white" />;
		}
		if (isError && lastError) {
			return (
				<Tooltip
					label={lastError.message}
					position="top"
					withArrow
					opened={true}
					styles={{
						tooltip: {
							backgroundColor: "rgba(220, 38, 38, 0.95)",
							color: "white",
							fontSize: "12px",
						},
					}}
				>
					<div style={{ color: "#ef4444" }}>
						<ErrorIcon />
					</div>
				</Tooltip>
			);
		}
		if (isRecording) {
			return <AudioVisualizer isActive={true} barColor="#eeeeee" />;
		}
		return <Logo className="size-5" />;
	};

	return (
		<div
			ref={containerRef}
			role="application"
			{...bindDrag()}
			style={{
				width: "fit-content",
				height: "fit-content",
				backgroundColor: isError
					? "rgba(127, 29, 29, 0.95)"
					: "rgba(0, 0, 0, 0.9)",
				borderRadius: 12,
				padding: 2,
				cursor: "grab",
				userSelect: "none",
				transition: "background-color 0.2s ease",
			}}
		>
			<button
				type="button"
				onClick={handleClick}
				disabled={isLoading}
				style={{
					width: 48,
					height: 48,
					display: "flex",
					alignItems: "center",
					justifyContent: "center",
					background: "transparent",
					border: "none",
					cursor: isLoading ? "wait" : "pointer",
					borderRadius: 10,
					position: "relative",
					opacity: isLoading ? 0.7 : 1,
				}}
				className="bg-black text-white hover:bg-gray-900"
			>
				{renderButtonContent()}
			</button>
		</div>
	);
}

export default function OverlayApp() {
	const [ready, setReady] = useState(false);

	// Sync pipeline config on mount
	useEffect(() => {
		const init = async () => {
			try {
				await invoke("sync_pipeline_config");
				setReady(true);
			} catch (error) {
				console.error("[Overlay] Failed to sync pipeline config:", error);
				// Still show UI even if sync fails
				setReady(true);
			}
		};

		init();
	}, []);

	if (!ready) {
		return (
			<div
				className="flex items-center justify-center"
				style={{
					width: 48,
					height: 48,
					backgroundColor: "rgba(0, 0, 0, 0.9)",
					borderRadius: 12,
				}}
			>
				<Loader size="xs" color="white" />
			</div>
		);
	}

	return <RecordingControl />;
}
