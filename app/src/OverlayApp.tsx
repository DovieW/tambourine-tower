import { Loader } from "@mantine/core";
import { useResizeObserver } from "@mantine/hooks";
import { useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useDrag } from "@use-gesture/react";
import { useCallback, useEffect, useRef, useState } from "react";
import Logo from "./assets/logo.svg?react";
import { useSettings, useTypeText } from "./lib/queries";
import { type ConnectionState, tauriAPI } from "./lib/tauri";
import "./app.css";

/**
 * Pipeline state machine states (matches Rust PipelineState)
 */
type PipelineState =
  | "idle"
  | "recording"
  | "transcribing"
  | "rewriting"
  | "error";

function isPipelineState(value: string): value is PipelineState {
  return (
    value === "idle" ||
    value === "recording" ||
    value === "transcribing" ||
    value === "rewriting" ||
    value === "error"
  );
}

type PipelineErrorPayload = {
  message: string;
  request_id?: string | null;
};

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

  // Missing persisted audio (retry can't run)
  if (
    errorStr.includes("Failed to read recording") ||
    errorStr.includes("Recording store") ||
    errorStr.includes("Cannot save recording")
  ) {
    return { message: "No saved audio", recoverable: true };
  }

  // Network/API errors
  if (errorStr.includes("Network") || errorStr.includes("network")) {
    return { message: "Network error", recoverable: true };
  }
  if (errorStr.includes("timeout") || errorStr.includes("Timeout")) {
    return { message: "Timed out", recoverable: true };
  }
  if (errorStr.includes("API error") || errorStr.includes("401")) {
    return { message: "API error", recoverable: true };
  }
  if (errorStr.includes("rate limit") || errorStr.includes("429")) {
    return { message: "Rate limited", recoverable: true };
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
  // If we have a real message, keep it short-ish and let the UI tooltip show the full text.
  const trimmed = errorStr.trim();
  if (trimmed && trimmed.length <= 64) {
    return { message: trimmed, recoverable: true };
  }
  return { message: "Error", recoverable: true };
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
    case "rewriting":
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
  const drawPointsRef = useRef<Float32Array | null>(null);
  const smoothedPeakRef = useRef(0);
  const noiseFloorRef = useRef(0);
  const voiceEnergyRef = useRef(0);

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
    drawPointsRef.current = null;
    smoothedPeakRef.current = 0;
    noiseFloorRef.current = 0;
    voiceEnergyRef.current = 0;
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

      const applyEdgeTaper = (v: number, i: number, n: number) => {
        // Make the waveform return to baseline at the start/end so it doesn't look
        // "cut off" by the canvas edge.
        const last = n - 1;
        if (last <= 0) return 0;
        const edgePoints = Math.max(6, Math.floor(n * 0.1));
        const d = Math.min(i, last - i);
        if (d >= edgePoints) return v;
        const t = d / edgePoints; // 0..1
        // Raised cosine ramp: 0 at the edge, 1 after edgePoints.
        const w = 0.5 - 0.5 * Math.cos(Math.PI * t);
        return v * w;
      };

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
        // Keep the idle animation subtle so it doesn't read as "picking up noise".
        const amp = logicalH * 0.055;
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
        // Avoid clipping stroke caps at the canvas edges.
        const xPad = 2;
        const xSpan = Math.max(1, logicalW - xPad * 2);
        for (let i = 0; i < points; i++) {
          const x = xPad + (i / (points - 1)) * xSpan;
          const tapered = applyEdgeTaper(wave[i] ?? 0, i, points);
          const y = midY + tapered * amp;
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
        // For visualization we want a signal with more natural dynamics.
        // Try to disable browser post-processing (noise suppression / AGC) when supported.
        const baseTrackConstraints: MediaTrackConstraints = {
          echoCancellation: false,
          noiseSuppression: false,
        };
        // Not all TS DOM libs include autoGainControl yet.
        (
          baseTrackConstraints as MediaTrackConstraints & {
            autoGainControl?: boolean;
          }
        ).autoGainControl = false;

        const constraints: MediaStreamConstraints = {
          audio:
            selectedMicId && selectedMicId.length > 0
              ? {
                  ...baseTrackConstraints,
                  deviceId: { exact: selectedMicId },
                }
              : baseTrackConstraints,
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

        // If recording starts via a global shortcut (not a direct user gesture),
        // some environments may create the AudioContext in a suspended state.
        // Resuming is safe even if already running.
        try {
          await audioContext.resume();
        } catch {
          // ignore
        }

        const source = audioContext.createMediaStreamSource(stream);

        // Filter out low-frequency rumble (bus/handling noise) and extreme highs,
        // so the visualization is driven primarily by speech frequencies.
        const highpass = audioContext.createBiquadFilter();
        highpass.type = "highpass";
        highpass.frequency.value = 180;
        highpass.Q.value = 0.7;

        const lowpass = audioContext.createBiquadFilter();
        lowpass.type = "lowpass";
        lowpass.frequency.value = 3800;
        lowpass.Q.value = 0.7;

        const analyser = audioContext.createAnalyser();
        // Higher resolution + smoother motion.
        analyser.fftSize = 2048;
        // Keep this relatively low; we do our own smoothing and want fast release.
        analyser.smoothingTimeConstant = 0.55;

        // Source -> filters -> analyser.
        source.connect(highpass);
        highpass.connect(lowpass);
        lowpass.connect(analyser);
        analyserRef.current = analyser;

        const canvas = canvasRef.current;
        if (!canvas) return;
        const ctx = canvas.getContext("2d");
        if (!ctx) return;

        const analyserNode = analyser;
        const timeData = new Uint8Array(analyserNode.fftSize);
        const freqData = new Uint8Array(analyserNode.frequencyBinCount);

        const draw = () => {
          if (!analyserRef.current || !mounted) return;
          const canvas = canvasRef.current;
          if (!canvas) return;
          const ctx = canvas.getContext("2d");
          if (!ctx) return;

          // HiDPI: scale the backing store to devicePixelRatio for crisp lines.
          const logicalW = canvas.clientWidth > 0 ? canvas.clientWidth : 168;
          const logicalH = canvas.clientHeight > 0 ? canvas.clientHeight : 24;
          const dpr = window.devicePixelRatio || 1;
          const targetW = Math.max(1, Math.floor(logicalW * dpr));
          const targetH = Math.max(1, Math.floor(logicalH * dpr));
          if (canvas.width !== targetW || canvas.height !== targetH) {
            canvas.width = targetW;
            canvas.height = targetH;
          }

          ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
          ctx.clearRect(0, 0, logicalW, logicalH);

          // Time + frequency data from the filtered signal.
          analyserNode.getByteTimeDomainData(timeData);
          analyserNode.getByteFrequencyData(freqData);

          // Fewer points + smoothing yields a calmer, less "spiky" line.
          const points = 64;
          const next = new Float32Array(points);
          for (let i = 0; i < points; i++) {
            const idx = Math.floor((i / (points - 1)) * (timeData.length - 1));
            const v = (timeData[idx] ?? 128) - 128;
            next[i] = v / 128;
          }

          // Auto-gain: normalize based on the *raw* peak so quiet mics still animate.
          // We'll apply a small deadzone after gain to keep silence from buzzing.
          let peak = 0;
          let sumSq = 0;
          for (let i = 0; i < points; i++) {
            const a = Math.abs(next[i] ?? 0);
            if (a > peak) peak = a;
            const v = next[i] ?? 0;
            sumSq += v * v;
          }

          // Voice energy (0..1) based on *speech-band* frequency content.
          // This rejects low-frequency rumble (bus/handling) much better than time-domain RMS.
          const binHz = audioContext.sampleRate / analyserNode.fftSize;
          const clampIdx = (i: number) =>
            Math.min(analyserNode.frequencyBinCount - 1, Math.max(0, i));

          const speechLo = clampIdx(Math.floor(300 / binHz));
          const speechHi = clampIdx(Math.ceil(3400 / binHz));
          const totalLo = clampIdx(Math.floor(60 / binHz));
          const totalHi = clampIdx(Math.ceil(8000 / binHz));

          let speechSum = 0;
          let totalSum = 0;
          let speechCount = 0;
          let totalCount = 0;
          for (let i = totalLo; i <= totalHi; i++) {
            const v = (freqData[i] ?? 0) / 255;
            const vv = v * v;
            totalSum += vv;
            totalCount++;
            if (i >= speechLo && i <= speechHi) {
              speechSum += vv;
              speechCount++;
            }
          }

          const totalRms =
            totalCount > 0 ? Math.sqrt(totalSum / totalCount) : 0;
          const speechRms =
            speechCount > 0 ? Math.sqrt(speechSum / speechCount) : 0;
          const speechRatio = totalRms > 1e-6 ? speechRms / totalRms : 0;

          // Adaptive floor - only rises when speechRatio is LOW (noise-like)
          // This prevents the floor from chasing continuous speech
          const prevNoiseFloor = noiseFloorRef.current;
          const isNoiseLike = speechRatio < 1.02; // Broadband = noise
          const riseRate = isNoiseLike ? 0.08 : 0.002; // Rise only for noise
          const fallRate = 0.01; // Very slow fall
          const nfRate = speechRms > prevNoiseFloor ? riseRate : fallRate;
          const noiseFloor =
            prevNoiseFloor + (speechRms - prevNoiseFloor) * nfRate;
          noiseFloorRef.current = noiseFloor;

          // Voice detection: use speechRatio as a soft multiplier
          // Bus noise tends to have speechRatio closer to 1.0 (broadband)
          // Voice has speechRatio often > 1.0 (concentrated in speech band)
          const aboveFloor = Math.max(0, speechRms - noiseFloor * 0.8);
          // Soft voice likelihood: 0 when ratio<=1.0, ramps up smoothly above 1.0
          const voiceLikelihood = Math.max(
            0,
            Math.min(1, (speechRatio - 1.0) * 8)
          );
          // Energy based on raw level, boosted by voice likelihood
          const rawEnergy = Math.pow(Math.min(1, aboveFloor / 0.015), 0.6);
          const voiceEnergyInstant = rawEnergy * (0.2 + voiceLikelihood * 0.8);

          // Smooth voice energy so it doesn't flicker between syllables
          // (which makes the waveform feel timid/twitchy).
          const prevVE = voiceEnergyRef.current;
          const attack = 0.55;
          const decay = 0.12;
          const ve =
            voiceEnergyInstant > prevVE
              ? prevVE * (1 - attack) + voiceEnergyInstant * attack
              : prevVE * (1 - decay) + voiceEnergyInstant * decay;
          voiceEnergyRef.current = ve;

          const prevPeak = smoothedPeakRef.current;
          // Relatively quick decay so gain relaxes when you stop speaking.
          const decayed = prevPeak * 0.9;
          smoothedPeakRef.current = Math.max(peak, decayed);

          // Floor prevents runaway gain on background noise.
          const effectivePeak = Math.max(0.02, smoothedPeakRef.current);
          const targetPeak = 0.9 + ve * 0.8;
          // Make voice visually pop hard.
          const maxGain = 3.0 + ve * 50;
          const gain = Math.min(
            maxGain,
            Math.max(1, targetPeak / effectivePeak)
          );

          // Apply gain directly without voiceScale limiting
          // Let the waveform grow to use more vertical space
          for (let i = 0; i < points; i++) {
            const v = (next[i] ?? 0) * gain;
            // Very light soft clip - allow values to grow larger before compressing
            next[i] = Math.tanh(v * 0.5) * 2.0;
          }

          // Smooth between frames to avoid jitter.
          const prev = lastPointsRef.current;
          if (!prev || prev.length !== points) {
            lastPointsRef.current = next;
          } else {
            // Higher responsiveness for more fluid constant motion
            const responsiveness = 0.25;
            const hold = 1 - responsiveness;
            for (let i = 0; i < points; i++) {
              const prevVal = prev[i] ?? 0;
              const nextVal = next[i] ?? 0;
              prev[i] = prevVal * hold + nextVal * responsiveness;
            }
            lastPointsRef.current = prev;
          }

          // Use the temporally-smoothed wave directly (no spatial smoothing)
          const drawWave = lastPointsRef.current ?? next;

          const applyEdgeTaper = (v: number, i: number, n: number) => {
            // Visually taper the waveform to a point at both ends.
            const last = n - 1;
            if (last <= 0) return 0;
            const edgePoints = Math.max(8, Math.floor(n * 0.1));
            const d = Math.min(i, last - i);
            if (d >= edgePoints) return v;
            const t = d / edgePoints; // 0..1
            const w = 0.5 - 0.5 * Math.cos(Math.PI * t);
            return v * w;
          };

          const midY = logicalH / 2;
          // Amplitude now controlled by voiceScale above, keep base amp high
          const maxAmp = Math.max(1, logicalH / 2 - 1);
          const amp = maxAmp * 0.95;

          // Plain white waveform
          const colorMid = `rgba(255,255,255,0.92)`;
          const colorEdge = `rgba(255,255,255,0.60)`;

          const grad = ctx.createLinearGradient(0, 0, logicalW, 0);
          grad.addColorStop(0, colorEdge);
          grad.addColorStop(0.5, colorMid);
          grad.addColorStop(1, colorEdge);

          const drawPath = (lineWidth: number, alpha: number, blur: number) => {
            ctx.save();
            ctx.globalAlpha = alpha;
            ctx.lineWidth = lineWidth;
            ctx.strokeStyle = grad;
            ctx.lineCap = "round";
            ctx.lineJoin = "round";
            ctx.shadowColor = "rgba(255,255,255,0.35)";
            ctx.shadowBlur = blur;

            // Prevent stroke end caps from being clipped by the canvas bounds.
            const xPad = Math.max(2, lineWidth * 0.75);
            const xSpan = Math.max(1, logicalW - xPad * 2);

            ctx.beginPath();
            for (let i = 0; i < points; i++) {
              const x = xPad + (i / (points - 1)) * xSpan;
              const tapered = applyEdgeTaper(drawWave[i] ?? 0, i, points);
              const y = midY + tapered * amp;
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
      width={168}
      height={24}
      className="overlay-wave"
      style={{ display: "block" }}
    />
  );
}

function RecordingControl() {
  const queryClient = useQueryClient();
  const [pipelineState, setPipelineState] = useState<PipelineState>("idle");
  const [lastError, setLastError] = useState<ErrorInfo | null>(null);
  const [lastErrorDetail, setLastErrorDetail] = useState<string | null>(null);
  const [lastFailedRequestId, setLastFailedRequestId] = useState<string | null>(
    null
  );
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
        if (isPipelineState(state)) {
          setPipelineState(state);
        } else {
          setPipelineState("idle");
        }
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
      pipelineState === "rewriting" ||
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

  const dismissError = useCallback(() => {
    // Reset pipeline state in backend so polling reflects reality.
    invoke("pipeline_force_reset").catch(console.error);
    setLastError(null);
    setLastErrorDetail(null);
    setLastFailedRequestId(null);

    // If we force-showed the window for an error (recording_only/never), allow the user to hide it.
    if (settings?.overlay_mode !== "always") {
      requestAnimatedHide();
    }
  }, [requestAnimatedHide, settings?.overlay_mode]);

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

    if (
      pipelineState === "recording" ||
      pipelineState === "transcribing" ||
      pipelineState === "rewriting"
    ) {
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
    if (
      prev === "recording" ||
      prev === "transcribing" ||
      prev === "rewriting" ||
      prev === "error"
    ) {
      requestAnimatedHide();
    }
  }, [pipelineState, requestAnimatedHide, settings?.overlay_mode]);

  // Start recording using the Rust pipeline
  const onStartRecording = useCallback(async () => {
    if (pipelineState !== "idle") return;

    // Clear any previous error when starting
    setLastError(null);
    setLastErrorDetail(null);
    setLastFailedRequestId(null);

    try {
      await invoke("pipeline_start_recording");
      setPipelineState("recording");
    } catch (error) {
      console.error("[Pipeline] Failed to start recording:", error);
      const errorInfo = parseError(error);
      setLastError(errorInfo);
      setLastErrorDetail(String(error));
    }
  }, [pipelineState]);

  // Stop recording and transcribe
  const onStopRecording = useCallback(async () => {
    if (pipelineState !== "recording") return;

    try {
      setPipelineState("transcribing");

      const transcript = await invoke<string>("pipeline_stop_and_transcribe");

      if (transcript) {
        // Type the transcript
        try {
          await typeTextMutation.mutateAsync(transcript);
        } catch (error) {
          console.error("[Pipeline] Failed to type text:", error);
          const errorInfo = parseError(error);
          setLastError(errorInfo);
          setLastErrorDetail(String(error));
        }
      }

      setPipelineState("idle");
      setLastError(null);
      setLastErrorDetail(null);
      setLastFailedRequestId(null);
    } catch (error) {
      console.error("[Pipeline] Failed to stop and transcribe:", error);
      setPipelineState("error");

      // Show error to user
      const errorInfo = parseError(error);
      setLastError(errorInfo);
      setLastErrorDetail(String(error));
    }
  }, [pipelineState, typeTextMutation]);

  const onRetry = useCallback(async () => {
    if (!lastFailedRequestId) return;
    try {
      setPipelineState("transcribing");
      setLastError(null);
      setLastErrorDetail(null);

      const transcript = await invoke<string>("pipeline_retry_transcription", {
        requestId: lastFailedRequestId,
      });

      if (transcript) {
        try {
          await typeTextMutation.mutateAsync(transcript);
        } catch (error) {
          console.error("[Pipeline] Failed to type retry transcript:", error);
          setLastError(parseError(error));
          setLastErrorDetail(String(error));
        }
      }

      setPipelineState("idle");
      setLastFailedRequestId(null);
    } catch (error) {
      console.error("[Pipeline] Retry failed:", error);
      setPipelineState("error");
      setLastError(parseError(error));
      setLastErrorDetail(String(error));
    }
  }, [lastFailedRequestId, typeTextMutation]);

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
      });
    };

    setup();

    return () => {
      unlistenStart?.();
      unlistenStop?.();
    };
  }, []);

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
        })
      );

      unlisteners.push(
        await listen("pipeline-cancelled", () => {
          setPipelineState("idle");
          setLastError(null);
          setLastErrorDetail(null);
          setLastFailedRequestId(null);
        })
      );

      unlisteners.push(
        await listen("pipeline-reset", () => {
          setPipelineState("idle");
          setLastError(null);
          setLastErrorDetail(null);
          setLastFailedRequestId(null);
        })
      );

      // Listen for pipeline errors (e.g., transcription failures from hotkey-triggered recordings)
      unlisteners.push(
        await listen<PipelineErrorPayload>("pipeline-error", (event) => {
          console.error("[Pipeline] Error from Rust:", event.payload);
          setPipelineState("error");

          const errorInfo = parseError(event.payload?.message);
          setLastError(errorInfo);
          setLastErrorDetail(event.payload?.message ?? null);
          setLastFailedRequestId(event.payload?.request_id ?? null);
        })
      );

      // Listen for successful transcription (from hotkey-triggered recordings)
      unlisteners.push(
        await listen<string>("pipeline-transcript-ready", () => {
          setPipelineState("idle");
          setLastError(null);
          setLastErrorDetail(null);
          setLastFailedRequestId(null);
        })
      );
    };

    setup();

    return () => {
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
  }, []);

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

    if (pipelineState === "idle" || pipelineState === "error") {
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

  const isLoading =
    pipelineState === "transcribing" || pipelineState === "rewriting";
  const isRecording = pipelineState === "recording";
  const isError = pipelineState === "error";
  const centerPhaseText =
    pipelineState === "rewriting"
      ? "rewriting..."
      : pipelineState === "transcribing"
      ? "transcribing..."
      : null;

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
                <div
                  style={{ display: "flex", flexDirection: "column", gap: 4 }}
                >
                  <div className="overlay-error-text" title={lastError.message}>
                    {lastError.message}
                  </div>
                </div>
              ) : centerPhaseText ? (
                <div className="overlay-phase-text" aria-live="polite">
                  {centerPhaseText}
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
                data-variant={isError ? "dim" : isRecording ? "rec" : "dim"}
                role={isError && !!lastFailedRequestId ? "button" : undefined}
                tabIndex={isError && !!lastFailedRequestId ? 0 : undefined}
                onClick={(e) => {
                  if (!isError || !lastFailedRequestId) return;
                  e.stopPropagation();
                  onRetry();
                }}
                onKeyDown={(e) => {
                  if (!isError || !lastFailedRequestId) return;
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    e.stopPropagation();
                    onRetry();
                  }
                }}
              >
                {isError ? "Retry" : "REC"}
              </div>

              {isError ? (
                <div
                  className="overlay-pill overlay-pill--close"
                  role="button"
                  tabIndex={0}
                  aria-label="Close"
                  title="Close"
                  onClick={(e) => {
                    e.stopPropagation();
                    dismissError();
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      e.stopPropagation();
                      dismissError();
                    }
                  }}
                >
                  ×
                </div>
              ) : null}
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
