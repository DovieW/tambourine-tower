import { Loader } from "@mantine/core";
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

type AudioWaveProps = {
  isActive: boolean;
  isVisible?: boolean;
  selectedMicId?: string | null;
};

function AudioWave({
  isActive,
  isVisible = true,
  selectedMicId,
}: AudioWaveProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const animationRef = useRef<number | null>(null);
  const analyserRef = useRef<AnalyserNode | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const audioContextRef = useRef<AudioContext | null>(null);
  const lastPointsRef = useRef<Float32Array | null>(null);

  const cleanupAudio = useCallback(() => {
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

    if (audioContextRef.current) {
      audioContextRef.current.close().catch(() => {
        // ignore
      });
      audioContextRef.current = null;
    }

    analyserRef.current = null;
    lastPointsRef.current = null;
  }, []);

  useEffect(() => {
    if (!isVisible) {
      cleanupAudio();
      return;
    }

    if (!isActive) {
      // Idle animation (no mic permissions required)
      let mounted = true;
      const canvas = canvasRef.current;
      const ctx = canvas?.getContext("2d") ?? null;
      if (!canvas || !ctx) return;

      const points = 64;
      const wave = new Float32Array(points);

      const drawIdle = (t: number) => {
        if (!mounted) return;

        const logicalW = canvas.clientWidth || 168;
        const logicalH = canvas.clientHeight || 24;
        const dpr = window.devicePixelRatio || 1;
        const targetW = Math.max(1, Math.floor(logicalW * dpr));
        const targetH = Math.max(1, Math.floor(logicalH * dpr));
        if (canvas.width !== targetW || canvas.height !== targetH) {
          canvas.width = targetW;
          canvas.height = targetH;
        }
        ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
        ctx.clearRect(0, 0, logicalW, logicalH);

        const midY = logicalH / 2;
        const amp = logicalH * 0.1;
        const phase = t / 650;
        for (let i = 0; i < points; i++) {
          const x = i / (points - 1);
          wave[i] =
            Math.sin(phase + x * Math.PI * 2) *
            (0.6 + 0.4 * Math.sin(phase * 0.9));
        }

        const grad = ctx.createLinearGradient(0, 0, logicalW, 0);
        grad.addColorStop(0, "rgba(255,255,255,0.18)");
        grad.addColorStop(0.5, "rgba(255,255,255,0.30)");
        grad.addColorStop(1, "rgba(255,255,255,0.18)");

        ctx.save();
        ctx.lineWidth = 2;
        ctx.strokeStyle = grad;
        ctx.lineCap = "round";
        ctx.lineJoin = "round";
        ctx.beginPath();
        for (let i = 0; i < points; i++) {
          const x = (i / (points - 1)) * logicalW;
          const y = midY + wave[i] * amp;
          if (i === 0) ctx.moveTo(x, y);
          else ctx.lineTo(x, y);
        }
        ctx.stroke();
        ctx.restore();

        animationRef.current = requestAnimationFrame(drawIdle);
      };

      animationRef.current = requestAnimationFrame(drawIdle);

      return () => {
        mounted = false;
        cleanupAudio();
      };
    }

    // Cleanup any idle loop before starting active audio.
    cleanupAudio();

    let mounted = true;

    const setupAudio = async () => {
      try {
        const constraints: MediaStreamConstraints = {
          audio:
            selectedMicId && selectedMicId.length > 0
              ? { deviceId: { exact: selectedMicId } }
              : true,
        };

        let stream: MediaStream;
        try {
          stream = await navigator.mediaDevices.getUserMedia(constraints);
        } catch (error) {
          // If a specific deviceId fails (disconnected / permission / etc.), fall back.
          if (selectedMicId && selectedMicId.length > 0) {
            stream = await navigator.mediaDevices.getUserMedia({ audio: true });
          } else {
            throw error;
          }
        }
        if (!mounted) {
          for (const track of stream.getTracks()) {
            track.stop();
          }
          return;
        }

        streamRef.current = stream;
        const audioContext = new AudioContext();
        audioContextRef.current = audioContext;
        const source = audioContext.createMediaStreamSource(stream);
        const analyser = audioContext.createAnalyser();
        // Higher resolution + smoother motion.
        analyser.fftSize = 2048;
        analyser.smoothingTimeConstant = 0.85;
        source.connect(analyser);
        analyserRef.current = analyser;

        const canvas = canvasRef.current;
        if (!canvas) return;
        const ctx = canvas.getContext("2d");
        if (!ctx) return;

        const draw = () => {
          if (!analyserRef.current || !mounted) return;
          const canvas = canvasRef.current;
          if (!canvas) return;
          const ctx = canvas.getContext("2d");
          if (!ctx) return;

          // HiDPI: scale the backing store to devicePixelRatio for crisp lines.
          const logicalW = canvas.clientWidth || canvas.width;
          const logicalH = canvas.clientHeight || canvas.height;
          const dpr = window.devicePixelRatio || 1;
          const targetW = Math.max(1, Math.floor(logicalW * dpr));
          const targetH = Math.max(1, Math.floor(logicalH * dpr));
          if (canvas.width !== targetW || canvas.height !== targetH) {
            canvas.width = targetW;
            canvas.height = targetH;
          }

          ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
          ctx.clearRect(0, 0, logicalW, logicalH);

          const analyserNode = analyserRef.current;
          const bufferLength = analyserNode.fftSize;
          const dataArray = new Uint8Array(bufferLength);
          analyserNode.getByteTimeDomainData(dataArray);

          // Downsample the waveform to a fixed number of points to keep it smooth.
          const points = 64;
          const next = new Float32Array(points);
          for (let i = 0; i < points; i++) {
            const idx = Math.floor((i / (points - 1)) * (bufferLength - 1));
            const v = (dataArray[idx] ?? 128) - 128;
            next[i] = v / 128;
          }

          // Smooth between frames to avoid jitter.
          const prev = lastPointsRef.current;
          if (!prev || prev.length !== points) {
            lastPointsRef.current = next;
          } else {
            for (let i = 0; i < points; i++) {
              prev[i] = prev[i] * 0.72 + next[i] * 0.28;
            }
            lastPointsRef.current = prev;
          }

          const wave = lastPointsRef.current ?? next;
          const midY = logicalH / 2;
          const amp = logicalH * 0.36;

          const grad = ctx.createLinearGradient(0, 0, logicalW, 0);
          grad.addColorStop(0, "rgba(255,255,255,0.60)");
          grad.addColorStop(0.5, "rgba(255,255,255,0.92)");
          grad.addColorStop(1, "rgba(255,255,255,0.60)");

          const drawPath = (lineWidth: number, alpha: number, blur: number) => {
            ctx.save();
            ctx.globalAlpha = alpha;
            ctx.lineWidth = lineWidth;
            ctx.strokeStyle = grad;
            ctx.lineCap = "round";
            ctx.lineJoin = "round";
            ctx.shadowColor = "rgba(255,255,255,0.35)";
            ctx.shadowBlur = blur;

            ctx.beginPath();
            for (let i = 0; i < points; i++) {
              const x = (i / (points - 1)) * logicalW;
              const y = midY + wave[i] * amp;
              if (i === 0) ctx.moveTo(x, y);
              else ctx.lineTo(x, y);
            }
            ctx.stroke();
            ctx.restore();
          };

          // Glow pass + crisp pass.
          drawPath(4, 0.28, 8);
          drawPath(2, 0.92, 0);

          animationRef.current = requestAnimationFrame(draw);
        };

        draw();
      } catch (error) {
        console.error("[AudioWave] Failed to setup audio:", error);
      }
    };

    setupAudio();

    return () => {
      mounted = false;
      cleanupAudio();
    };
  }, [cleanupAudio, isActive, isVisible, selectedMicId]);

  return (
    <canvas
      ref={canvasRef}
      className="overlay-wave"
      style={{ display: "block" }}
    />
  );
}

function RecordingControl() {
  const queryClient = useQueryClient();
  const [pipelineState, setPipelineState] = useState<PipelineState>("idle");
  const [lastError, setLastError] = useState<ErrorInfo | null>(null);
  const [containerRef, rect] = useResizeObserver();
  const hasDragStartedRef = useRef(false);
  const [animState, setAnimState] = useState<"enter" | "visible" | "exit">(
    "visible"
  );
  const exitTimerRef = useRef<number | null>(null);

  // Collapsed/expanded UI state
  const [expanded, setExpanded] = useState(false);
  // We only render the expanded widget after the native window has resized wide enough,
  // to avoid a one-frame clipped/"missing border" intermediate.
  const [renderExpanded, setRenderExpanded] = useState(false);
  const prevPipelineForExpandRef = useRef<PipelineState>("idle");

  // Load settings (overlay mode + selected mic)
  const { data: settings } = useSettings();

  // TanStack Query hooks
  const typeTextMutation = useTypeText();
  const addHistoryEntry = useAddHistoryEntry();

  // Clear error after 5 seconds
  const { start: startErrorTimeout, clear: clearErrorTimeout } = useTimeout(
    () => {
      setLastError(null);
    },
    5000
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

  // Resize the native window for the target widget, and only then render it.
  // This avoids the "intermediate step" where the widget is wider than the window
  // (or vice versa) for a frame.
  useEffect(() => {
    if (expanded) {
      setRenderExpanded(false);
      tauriAPI.resizeOverlay(264, 56);
      return;
    }

    // Collapse: hide expanded immediately, then shrink window.
    setRenderExpanded(false);
    tauriAPI.resizeOverlay(56, 56);
  }, [expanded]);

  useEffect(() => {
    if (!expanded) return;
    if (rect.width >= 260) {
      setRenderExpanded(true);
    }
  }, [expanded, rect.width]);

  // Keep expanded while active; collapse when returning to idle.
  useEffect(() => {
    const prev = prevPipelineForExpandRef.current;
    prevPipelineForExpandRef.current = pipelineState;

    // In recording-only overlay mode, we never want to show the collapsed widget.
    // The window itself is shown/hidden by the backend; the overlay should stay in
    // its full state whenever it is visible.
    if (settings?.overlay_mode === "recording_only") {
      setExpanded(true);
      return;
    }

    if (
      pipelineState === "recording" ||
      pipelineState === "transcribing" ||
      pipelineState === "error"
    ) {
      setExpanded(true);
      return;
    }

    // Collapse immediately after finishing an active state.
    if (pipelineState === "idle" && prev !== "idle") {
      setExpanded(false);
    }
  }, [pipelineState, settings?.overlay_mode]);

  // If the user switches into recording-only mode while the window is visible,
  // immediately force expanded so we don't flash the collapsed state.
  useEffect(() => {
    if (settings?.overlay_mode === "recording_only") {
      setExpanded(true);
    }
  }, [settings?.overlay_mode]);

  // If we switch *out* of recording-only mode into always-visible while idle,
  // collapse back to the default logo-only state immediately (otherwise we'd stay
  // expanded until the next recording cycle flips pipelineState away from idle).
  useEffect(() => {
    if (settings?.overlay_mode !== "always") return;
    if (pipelineState !== "idle") return;
    setExpanded(false);
  }, [pipelineState, settings?.overlay_mode]);

  const requestAnimatedHide = useCallback(() => {
    if (exitTimerRef.current) {
      window.clearTimeout(exitTimerRef.current);
      exitTimerRef.current = null;
    }

    setAnimState("exit");
    // Keep duration in sync with CSS transition (180ms) + a tiny buffer.
    exitTimerRef.current = window.setTimeout(() => {
      invoke("hide_overlay").catch(console.error);
      // Prep for next entrance.
      setAnimState("enter");
      exitTimerRef.current = null;
    }, 210);
  }, []);

  const requestAnimatedShow = useCallback(() => {
    if (exitTimerRef.current) {
      window.clearTimeout(exitTimerRef.current);
      exitTimerRef.current = null;
    }

    // Force a transition even if we were previously visible.
    setAnimState("enter");
    requestAnimationFrame(() => {
      setAnimState("visible");
    });
  }, []);

  // Entrance animation when recording starts (recording-only mode shows the window)
  useEffect(() => {
    if (settings?.overlay_mode === "always") {
      setAnimState("visible");
      return;
    }

    if (pipelineState === "recording" || pipelineState === "transcribing") {
      requestAnimatedShow();
    }
  }, [pipelineState, requestAnimatedShow, settings?.overlay_mode]);

  // Backend can request a hide (so we can animate out before the window hides)
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    const setup = async () => {
      unlisten = await listen("overlay-hide-requested", () => {
        requestAnimatedHide();
      });
    };

    setup();
    return () => {
      unlisten?.();
    };
  }, [requestAnimatedHide, settings?.overlay_mode]);

  // If the overlay itself was used to record (not hotkey path), honor recording-only by
  // animating out when we return to idle.
  const prevPipelineStateRef = useRef<PipelineState>("idle");
  useEffect(() => {
    const prev = prevPipelineStateRef.current;
    prevPipelineStateRef.current = pipelineState;

    if (settings?.overlay_mode !== "recording_only") return;
    if (pipelineState !== "idle") return;
    if (prev === "recording" || prev === "transcribing" || prev === "error") {
      requestAnimatedHide();
    }
  }, [pipelineState, requestAnimatedHide, settings?.overlay_mode]);

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
        })
      );

      unlisteners.push(
        await listen("pipeline-transcription-started", () => {
          setPipelineState("transcribing");
          startResponseTimeout();
        })
      );

      unlisteners.push(
        await listen("pipeline-cancelled", () => {
          setPipelineState("idle");
          clearResponseTimeout();
        })
      );

      unlisteners.push(
        await listen("pipeline-reset", () => {
          setPipelineState("idle");
          clearResponseTimeout();
        })
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
        })
      );

      // Listen for successful transcription (from hotkey-triggered recordings)
      unlisteners.push(
        await listen<string>("pipeline-transcript-ready", () => {
          clearResponseTimeout();
          setPipelineState("idle");
        })
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

  // Click behavior:
  // - idle + collapsed: expand and start recording immediately
  // - idle + expanded: start recording
  // - recording: stop recording
  const handleClick = useCallback(() => {
    if (pipelineState === "recording") {
      onStopRecording();
      return;
    }

    if (pipelineState === "idle") {
      if (!expanded) {
        setExpanded(true);
      }
      onStartRecording();
    }
  }, [expanded, onStartRecording, onStopRecording, pipelineState]);

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
    { filterTaps: true }
  );

  const isLoading = pipelineState === "transcribing";
  const isRecording = pipelineState === "recording";
  const isError = pipelineState === "error" || lastError !== null;

  const renderIcon = () => {
    if (isLoading) return <Loader size="xs" color="white" />;
    if (isError) {
      return (
        <div style={{ color: "#ef4444" }} aria-label="Error">
          <ErrorIcon />
        </div>
      );
    }
    return <Logo className="size-5" />;
  };

  return (
    <div
      ref={containerRef}
      role="application"
      {...bindDrag()}
      className="overlay-widget"
      data-anim={animState}
      style={{
        width: "100%",
        height: "100%",
        cursor: "grab",
        userSelect: "none",
      }}
    >
      <div className="overlay-stage">
        {/* Collapsed widget */}
        {!renderExpanded && settings?.overlay_mode !== "recording_only" ? (
          <button
            type="button"
            onClick={handleClick}
            disabled={isLoading}
            className="overlay-button overlay-button--collapsed"
            style={
              isError ? { background: "rgba(127, 29, 29, 0.92)" } : undefined
            }
          >
            <div className="overlay-icon">{renderIcon()}</div>
          </button>
        ) : null}

        {/* Expanded widget */}
        {renderExpanded ? (
          <button
            type="button"
            onClick={handleClick}
            disabled={isLoading}
            className="overlay-button overlay-button--expanded"
            style={
              isError ? { background: "rgba(127, 29, 29, 0.92)" } : undefined
            }
          >
            <div className="overlay-icon">{renderIcon()}</div>
            <div className="overlay-center">
              {isError && lastError ? (
                <div className="overlay-error-text" title={lastError.message}>
                  {lastError.message}
                </div>
              ) : (
                <AudioWave
                  isActive={isRecording}
                  isVisible={true}
                  selectedMicId={settings?.selected_mic_id ?? null}
                />
              )}
            </div>
            <div className="overlay-meta">
              <div
                className="overlay-pill"
                data-variant={isRecording ? "rec" : "dim"}
              >
                REC
              </div>
            </div>
          </button>
        ) : null}
      </div>
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
          width: 56,
          height: 56,
          backgroundColor: "rgba(0, 0, 0, 0.9)",
          borderRadius: 16,
        }}
      >
        <Loader size="xs" color="white" />
      </div>
    );
  }

  return <RecordingControl />;
}
