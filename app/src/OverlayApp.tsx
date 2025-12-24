import { Loader } from "@mantine/core";
import { useResizeObserver } from "@mantine/hooks";
import { useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useDrag } from "@use-gesture/react";
import { useCallback, useEffect, useRef, useState } from "react";
import Logo from "./assets/logo.svg?react";
import { applyAccentColor } from "./lib/accentColor";
import { useSettings, useTypeText } from "./lib/queries";
import { type ConnectionState, tauriAPI } from "./lib/tauri";
import "./app.css";

/**
 * Pipeline state machine states (matches Rust PipelineState)
 */
type PipelineState =
  | "idle"
  | "arming"
  | "recording"
  | "transcribing"
  | "rewriting"
  | "error";

function isPipelineState(value: string): value is PipelineState {
  return (
    value === "idle" ||
    // NOTE: "arming" is a UI-only state; Rust will never return it.
    value === "arming" ||
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
  // Handle object errors (e.g., CommandError from Tauri with { message, error_type })
  let errorStr: string;
  if (error && typeof error === "object" && "message" in error) {
    errorStr = String((error as { message: unknown }).message);
  } else {
    errorStr = String(error);
  }

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
    case "arming":
      return "connecting";
    case "recording":
      return "recording";
    case "transcribing":
    case "rewriting":
      return "processing";
    case "error":
      return "disconnected";
    default: {
      const _exhaustive: never = state;
      return _exhaustive;
    }
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
  className?: string;
};

type BackendAudioLevelPayload = {
  seq: number;
  rms: number;
  peak: number;
  wave_seq?: number;
  mins?: number[];
  maxes?: number[];
};

function BackendAudioWave({
  isActive,
  isVisible = true,
  className,
}: {
  isActive: boolean;
  isVisible?: boolean;
  className?: string;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const animationRef = useRef<number | null>(null);
  const levelRef = useRef(0);
  const historyRef = useRef<Float32Array>(new Float32Array(64));
  const smoothLevelRef = useRef(0);
  const hasFrameRef = useRef(false);
  const waveMinRef = useRef<Float32Array | null>(null);
  const waveMaxRef = useRef<Float32Array | null>(null);
  const hasWaveformRef = useRef(false);
  const waveMaxAbsRef = useRef(0);
  const waveGainRef = useRef(1);
  const waveHistBinsRef = useRef(0);
  const waveHistWriteRef = useRef(0);
  const waveHistBufRef = useRef<Float32Array | null>(null);
  const waveHistMaxAbsRef = useRef<Float32Array | null>(null);

  // More frames = more visible peaks/valleys across the width (more temporal detail).
  const WAVE_HISTORY_FRAMES = 14;

  const getAccentRgb = useCallback(() => {
    const raw =
      getComputedStyle(document.documentElement)
        .getPropertyValue("--accent-primary")
        .trim() || "#f97316";

    const hex = raw.startsWith("#") ? raw.slice(1) : null;
    if (hex) {
      const h =
        hex.length === 3
          ? hex
              .split("")
              .map((c) => c + c)
              .join("")
          : hex;
      if (h.length === 6) {
        const r = Number.parseInt(h.slice(0, 2), 16);
        const g = Number.parseInt(h.slice(2, 4), 16);
        const b = Number.parseInt(h.slice(4, 6), 16);
        if (Number.isFinite(r) && Number.isFinite(g) && Number.isFinite(b)) {
          return { r, g, b };
        }
      }
    }

    const m = raw
      .replace(/\s+/g, "")
      .match(/^rgba?\((\d+),(\d+),(\d+)(?:,(\d*\.?\d+))?\)$/i);
    if (m) {
      return {
        r: Number.parseInt(m[1] ?? "0", 10),
        g: Number.parseInt(m[2] ?? "0", 10),
        b: Number.parseInt(m[3] ?? "0", 10),
      };
    }

    return { r: 249, g: 115, b: 22 };
  }, []);

  useEffect(() => {
    if (!isVisible) {
      hasFrameRef.current = false;
      levelRef.current = 0;
      historyRef.current.fill(0);
      smoothLevelRef.current = 0;
      waveMinRef.current = null;
      waveMaxRef.current = null;
      hasWaveformRef.current = false;
      waveMaxAbsRef.current = 0;
      waveGainRef.current = 1;
      waveHistBinsRef.current = 0;
      waveHistWriteRef.current = 0;
      waveHistBufRef.current = null;
      waveHistMaxAbsRef.current = null;
      return;
    }

    let unlisten: (() => void) | undefined;
    const setup = async () => {
      unlisten = await listen<BackendAudioLevelPayload>(
        "overlay-audio-level",
        (event) => {
          const p = event.payload;
          if (!p) return;

          // If waveform buckets are available, prefer those for true DAW-style rendering.
          if (
            Array.isArray(p.mins) &&
            Array.isArray(p.maxes) &&
            p.mins.length > 0 &&
            p.mins.length === p.maxes.length
          ) {
            const n = p.mins.length;
            const mins = new Float32Array(n);
            const maxes = new Float32Array(n);
            for (let i = 0; i < n; i++) {
              const mn = p.mins[i] ?? 0;
              const mx = p.maxes[i] ?? 0;
              mins[i] = Number.isFinite(mn) ? Math.max(-1, Math.min(1, mn)) : 0;
              maxes[i] = Number.isFinite(mx)
                ? Math.max(-1, Math.min(1, mx))
                : 0;
            }

            // Smooth across time to avoid jittery outlines.
            const prevMins = waveMinRef.current;
            const prevMaxes = waveMaxRef.current;
            if (
              prevMins &&
              prevMaxes &&
              prevMins.length === n &&
              prevMaxes.length === n
            ) {
              // Lower smoothing = more transient detail (peaks/valleys).
              const a = 0.55; // higher = steadier
              for (let i = 0; i < n; i++) {
                mins[i] = (prevMins[i] ?? 0) * a + (mins[i] ?? 0) * (1 - a);
                maxes[i] = (prevMaxes[i] ?? 0) * a + (maxes[i] ?? 0) * (1 - a);
              }
            }

            // Light smoothing across bins (X axis) to keep the DAW bars stable
            // without washing out contrast.
            const mins2 = new Float32Array(n);
            const maxes2 = new Float32Array(n);
            for (let i = 0; i < n; i++) {
              const l = i > 0 ? i - 1 : i;
              const r = i + 1 < n ? i + 1 : i;
              mins2[i] =
                (mins[l] ?? 0) * 0.2 +
                (mins[i] ?? 0) * 0.6 +
                (mins[r] ?? 0) * 0.2;
              maxes2[i] =
                (maxes[l] ?? 0) * 0.2 +
                (maxes[i] ?? 0) * 0.6 +
                (maxes[r] ?? 0) * 0.2;
            }

            // Convert to half-amplitude (DAW-style) and store a short rolling
            // history so the waveform shows peaks/valleys across time.
            if (waveHistBinsRef.current !== n || !waveHistBufRef.current) {
              waveHistBinsRef.current = n;
              waveHistWriteRef.current = 0;
              waveHistBufRef.current = new Float32Array(
                WAVE_HISTORY_FRAMES * n
              );
              waveHistMaxAbsRef.current = new Float32Array(WAVE_HISTORY_FRAMES);
            }

            const buf = waveHistBufRef.current;
            const maxAbsByFrame = waveHistMaxAbsRef.current;
            const write = waveHistWriteRef.current;
            const base = write * n;

            let frameMaxAbs = 0;
            for (let i = 0; i < n; i++) {
              const half = ((maxes2[i] ?? 0) - (mins2[i] ?? 0)) * 0.5;
              buf[base + i] = half;
              frameMaxAbs = Math.max(frameMaxAbs, Math.abs(half));
            }
            if (maxAbsByFrame) maxAbsByFrame[write] = frameMaxAbs;
            waveHistWriteRef.current = (write + 1) % WAVE_HISTORY_FRAMES;

            // Track a recent max for auto-gain. (Small N, so scan is cheap.)
            let recentMax = frameMaxAbs;
            if (maxAbsByFrame) {
              for (let i = 0; i < WAVE_HISTORY_FRAMES; i++) {
                recentMax = Math.max(recentMax, maxAbsByFrame[i] ?? 0);
              }
            }

            // Always prefer true waveform buckets when present; renderer applies an
            // auto-gain so normal speaking volume doesn't look flat.
            waveMinRef.current = mins2;
            waveMaxRef.current = maxes2;
            waveMaxAbsRef.current = recentMax;
            hasWaveformRef.current = true;
            hasFrameRef.current = true;
            return;
          }

          // If we reach here, we either don't have buckets or they look like silence.
          hasWaveformRef.current = false;

          // Fallback: loudness-based visualization.
          // Backend values are in [0, 1], but typical voice RMS can be quite small.
          // A dB mapping is much more perceptually linear and prevents a "nearly flat"
          // waveform when RMS lives around 0.005–0.02.
          const rms = Number.isFinite(p.rms)
            ? Math.max(0, Math.min(1, p.rms))
            : 0;
          const peak = Number.isFinite(p.peak)
            ? Math.max(0, Math.min(1, p.peak))
            : 0;

          const clamp01 = (v: number) => Math.max(0, Math.min(1, v));
          const eps = 1e-6;
          const rmsDb = 20 * Math.log10(Math.max(eps, rms));
          const peakDb = 20 * Math.log10(Math.max(eps, peak));

          // Normalize MIN_DB..0dB into 0..1. (Below MIN_DB is treated as silence.)
          // Using a wider window makes quiet-but-real voice energy more visible.
          const MIN_DB = -72;
          const normDb = (db: number) => clamp01((db - MIN_DB) / (0 - MIN_DB));
          const rmsNorm = normDb(rmsDb);
          const peakNorm = normDb(peakDb);

          // Prefer RMS (steady voice energy) but let peaks show punch.
          let level = Math.max(rmsNorm * 1.25, peakNorm);
          // Visual gain + curve so typical speaking doesn't look near-flat.
          level = Math.min(1, level * 1.6);
          level = Math.pow(level, 0.72);
          if (!Number.isFinite(level) || level < 0.003) level = 0;

          hasFrameRef.current = true;
          levelRef.current = level;
        }
      );
    };

    setup();
    return () => {
      unlisten?.();
    };
  }, [isVisible]);

  useEffect(() => {
    if (!isVisible) return;

    if (animationRef.current) {
      cancelAnimationFrame(animationRef.current);
      animationRef.current = null;
    }

    let mounted = true;

    const applyEdgeTaper = (v: number, i: number, n: number) => {
      const last = n - 1;
      if (last <= 0) return 0;
      const edgePoints = Math.max(8, Math.floor(n * 0.1));
      const d = Math.min(i, last - i);
      if (d >= edgePoints) return v;
      const t = d / edgePoints;
      const w = 0.5 - 0.5 * Math.cos(Math.PI * t);
      return v * w;
    };

    const draw = () => {
      if (!mounted) return;
      const canvas = canvasRef.current;
      const ctx = canvas?.getContext("2d") ?? null;
      if (!canvas || !ctx) return;

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

      const { r, g, b } = getAccentRgb();
      const colorMid = `rgba(${r},${g},${b},0.90)`;
      const colorEdge = `rgba(${r},${g},${b},0.55)`;
      const grad = ctx.createLinearGradient(0, 0, logicalW, 0);
      grad.addColorStop(0, colorEdge);
      grad.addColorStop(0.5, colorMid);
      grad.addColorStop(1, colorEdge);

      const midY = logicalH / 2;
      const maxAmp = Math.max(1, logicalH / 2 - 1);
      const amp = maxAmp * 0.95;

      const drawTrueWaveform = (_mins: Float32Array, _maxes: Float32Array) => {
        const n = waveHistBinsRef.current;
        const hist = waveHistBufRef.current;
        if (!hist || n <= 1) return;

        const xPad = 2;
        const xSpan = Math.max(1, logicalW - xPad * 2);

        const clamp11 = (v: number) => Math.max(-1, Math.min(1, v));

        // Auto-gain based on the last published bucket max abs.
        // Use a perceptual curve so quiet speech becomes visible quickly.
        const rawAbs = waveMaxAbsRef.current;

        // Separate curves:
        // - gain curve is more aggressive to avoid a long "ramp-up" period on quiet mics
        // - render curve is slightly gentler so peaks still stand out
        const CURVE_GAIN = 0.45;
        const CURVE_RENDER = 0.58;
        const effectiveAbs = Math.pow(Math.max(1e-6, rawAbs), CURVE_GAIN);

        // Auto-gain target is deliberately < 1 so we keep headroom and avoid
        // living at full-scale all the time.
        const HEADROOM = 0.78;

        // Higher target + higher max gain makes normal speaking move the waveform
        // more, especially when the capture path yields small bucket values.
        const targetAbs = 0.62;
        const MAX_GAIN = 140;
        let desiredGain = (targetAbs * HEADROOM) / effectiveAbs;
        // Extra boost for very quiet-but-nonzero signals.
        if (rawAbs > 1e-6 && rawAbs < 0.02) desiredGain *= 1.6;
        if (rawAbs > 1e-6 && rawAbs < 0.006) desiredGain *= 1.4;
        desiredGain = Math.min(MAX_GAIN, Math.max(1, desiredGain));

        // Smooth + slew-limit to avoid gain "pumping" frame-to-frame.
        const prevG = waveGainRef.current;
        const smoothedDesired = prevG * 0.78 + desiredGain * 0.22;
        // Faster ramp-up reduces perceived "startup lag" when the capture path
        // yields small-but-real bucket values.
        const maxUp = Math.max(2.0, prevG * 0.22);
        const maxDown = Math.max(2.0, prevG * 0.18);
        const delta = smoothedDesired - prevG;
        const nextG = prevG + Math.max(-maxDown, Math.min(maxUp, delta));
        waveGainRef.current = Math.max(1, Math.min(MAX_GAIN, nextG));
        const g = waveGainRef.current;

        const trueAmp = amp * 1.0;

        const softLimit = (v: number) => {
          // Soft-knee limiter in [-1, 1]. Prevents constant hard clipping.
          // Slightly less aggressive so peaks read as peaks.
          const k = 1.6;
          const t = Math.tanh(k);
          return t === 0 ? clamp11(v) : Math.tanh(k * v) / t;
        };

        // DAW-style: render as vertical bars. Use a medium horizontal resolution
        // (denser than before), but keep gaps so it doesn't turn into a filled worm.
        // Roughly ~1 bar per 2px, clamped.
        const barCount = Math.max(48, Math.min(140, Math.round(xSpan / 2)));
        const spacing = barCount <= 1 ? xSpan : xSpan / (barCount - 1);
        const barW = Math.max(1, Math.min(2, spacing * 0.65));

        const drawBars = (alpha: number, blur: number) => {
          ctx.save();
          ctx.globalAlpha = alpha;
          ctx.fillStyle = grad;
          ctx.shadowColor = `rgba(${r},${g},${b},0.35)`;
          ctx.shadowBlur = blur;

          // Oldest frame is the next-to-write (ring buffer).
          const write = waveHistWriteRef.current;
          const total = WAVE_HISTORY_FRAMES * n;
          for (let bi = 0; bi < barCount; bi++) {
            const u = barCount <= 1 ? 0 : bi / (barCount - 1);

            // Map x across the entire history window, left=oldest, right=newest.
            const tAll = u * (total - 1);
            const gi0 = Math.floor(tAll);
            const gi1 = Math.min(total - 1, gi0 + 1);
            const tt = tAll - gi0;

            // Convert global indices into ring-buffered frame/bin indices.
            const f0 = Math.floor(gi0 / n);
            const b0 = gi0 % n;
            const f1 = Math.floor(gi1 / n);
            const b1 = gi1 % n;
            const rf0 = (write + f0) % WAVE_HISTORY_FRAMES;
            const rf1 = (write + f1) % WAVE_HISTORY_FRAMES;
            const v0 = hist[rf0 * n + b0] ?? 0;
            const v1 = hist[rf1 * n + b1] ?? 0;
            const half = v0 * (1 - tt) + v1 * tt;

            const curved =
              Math.sign(half) * Math.pow(Math.abs(half), CURVE_RENDER);
            const v = softLimit(curved * g);
            const yTop = midY - clamp11(v) * trueAmp;
            const yBot = midY + clamp11(v) * trueAmp;
            const y = Math.min(yTop, yBot);
            const h = Math.max(1, Math.abs(yBot - yTop));
            // Even spacing avoids occasional larger gaps from rounding.
            const xCenter = xPad + bi * spacing;
            ctx.fillRect(xCenter - barW / 2, y, barW, h);
          }

          ctx.restore();
        };

        drawBars(0.4, 6);
        drawBars(0.82, 0);
      };

      // Prefer true waveform buckets if present.
      const mins = waveMinRef.current;
      const maxes = waveMaxRef.current;
      if (isActive && hasWaveformRef.current && mins && maxes) {
        drawTrueWaveform(mins, maxes);
        animationRef.current = requestAnimationFrame(draw);
        return;
      }

      // Stationary waveform: instead of scrolling history right-to-left, keep a
      // single smoothed level and modulate it across X. This makes the whole wave
      // "go up and down" together.
      const hist = historyRef.current;
      const target = isActive && hasFrameRef.current ? levelRef.current : 0;
      const prev = smoothLevelRef.current;
      const next = prev * 0.68 + target * 0.32;
      smoothLevelRef.current = next;

      // Generate a full-width envelope from the single level using a standing-wave
      // style profile (no horizontal flow).
      const t = performance.now() / 1000;
      const n = hist.length;
      for (let i = 0; i < n; i++) {
        const x = n <= 1 ? 0 : i / (n - 1);
        // Edge taper (quiet at the ends, louder near center).
        const edge = Math.sin(Math.PI * x); // 0..1..0
        // Gentle shape variation so it doesn't look like a flat slab.
        // This is a standing pattern (depends on x) that "breathes" (depends on t)
        // without translating left/right.
        const wobbleA = 0.65 + 0.35 * Math.sin(t * 3.4);
        const wobbleB = 0.6 + 0.4 * Math.sin(t * 2.1 + 0.8);
        const spatial =
          0.55 +
          0.25 * Math.sin(x * Math.PI * 2) * wobbleA +
          0.2 * Math.sin(x * Math.PI * 4) * wobbleB;
        const lvl = Math.max(0, Math.min(1, next * edge * spatial));
        hist[i] = lvl;
      }

      // Draw a mirrored, filled envelope waveform (DAW-style), which better matches
      // the user's mental model and avoids the "always goes up then down" wobble.
      const drawFilledEnvelope = (alpha: number, blur: number) => {
        ctx.save();
        ctx.globalAlpha = alpha;
        ctx.fillStyle = grad;
        ctx.shadowColor = `rgba(${r},${g},${b},0.35)`;
        ctx.shadowBlur = blur;

        const xPad = 2;
        const xSpan = Math.max(1, logicalW - xPad * 2);

        // Top edge
        ctx.beginPath();
        for (let i = 0; i < hist.length; i++) {
          const x = xPad + (i / (hist.length - 1)) * xSpan;
          const lvl = applyEdgeTaper(hist[i] ?? 0, i, hist.length);
          const yTop = midY - lvl * amp;
          if (i === 0) ctx.moveTo(x, yTop);
          else ctx.lineTo(x, yTop);
        }

        // Bottom edge (reverse)
        for (let i = hist.length - 1; i >= 0; i--) {
          const x = xPad + (i / (hist.length - 1)) * xSpan;
          const lvl = applyEdgeTaper(hist[i] ?? 0, i, hist.length);
          const yBot = midY + lvl * amp;
          ctx.lineTo(x, yBot);
        }

        ctx.closePath();
        ctx.fill();
        ctx.restore();
      };

      const drawOutline = (lineWidth: number, alpha: number) => {
        ctx.save();
        ctx.globalAlpha = alpha;
        ctx.lineWidth = lineWidth;
        ctx.strokeStyle = grad;
        ctx.lineCap = "round";
        ctx.lineJoin = "round";

        const xPad = Math.max(2, lineWidth * 0.75);
        const xSpan = Math.max(1, logicalW - xPad * 2);

        // Top outline
        ctx.beginPath();
        for (let i = 0; i < hist.length; i++) {
          const x = xPad + (i / (hist.length - 1)) * xSpan;
          const lvl = applyEdgeTaper(hist[i] ?? 0, i, hist.length);
          const yTop = midY - lvl * amp;
          if (i === 0) ctx.moveTo(x, yTop);
          else ctx.lineTo(x, yTop);
        }
        ctx.stroke();

        // Bottom outline
        ctx.beginPath();
        for (let i = 0; i < hist.length; i++) {
          const x = xPad + (i / (hist.length - 1)) * xSpan;
          const lvl = applyEdgeTaper(hist[i] ?? 0, i, hist.length);
          const yBot = midY + lvl * amp;
          if (i === 0) ctx.moveTo(x, yBot);
          else ctx.lineTo(x, yBot);
        }
        ctx.stroke();
        ctx.restore();
      };

      drawFilledEnvelope(0.55, 10);
      drawFilledEnvelope(0.9, 0);
      drawOutline(1.5, 0.85);

      animationRef.current = requestAnimationFrame(draw);
    };

    animationRef.current = requestAnimationFrame(draw);

    return () => {
      mounted = false;
      if (animationRef.current) {
        cancelAnimationFrame(animationRef.current);
        animationRef.current = null;
      }
    };
  }, [getAccentRgb, isActive, isVisible]);

  return (
    <canvas
      ref={canvasRef}
      width={168}
      height={24}
      className={["overlay-wave", className].filter(Boolean).join(" ")}
      style={{ display: "block" }}
    />
  );
}

function AudioWave({
  isActive,
  isVisible = true,
  selectedMicId,
  className,
}: AudioWaveProps) {
  const WARM_MIC_IDLE_TIMEOUT_MS = 10 * 60 * 1000;

  const canvasRef = useRef<HTMLCanvasElement>(null);
  const animationRef = useRef<number | null>(null);
  const analyserRef = useRef<AnalyserNode | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const audioContextRef = useRef<AudioContext | null>(null);
  const micWarmTimerRef = useRef<number | null>(null);
  const activeMicIdRef = useRef<string | null>(null);
  const lastPointsRef = useRef<Float32Array | null>(null);
  const drawPointsRef = useRef<Float32Array | null>(null);
  const smoothedPeakRef = useRef(0);
  const noiseFloorRef = useRef(0);
  const voiceEnergyRef = useRef(0);

  const getAccentRgb = useCallback(() => {
    // Read from CSS so the overlay waveform matches the app brand accent.
    const raw =
      getComputedStyle(document.documentElement)
        .getPropertyValue("--accent-primary")
        .trim() || "#f97316";

    // Minimal color parsing: supports #rgb/#rrggbb and rgb()/rgba().
    const hex = raw.startsWith("#") ? raw.slice(1) : null;
    if (hex) {
      const h =
        hex.length === 3
          ? hex
              .split("")
              .map((c) => c + c)
              .join("")
          : hex;
      if (h.length === 6) {
        const r = Number.parseInt(h.slice(0, 2), 16);
        const g = Number.parseInt(h.slice(2, 4), 16);
        const b = Number.parseInt(h.slice(4, 6), 16);
        if (Number.isFinite(r) && Number.isFinite(g) && Number.isFinite(b)) {
          return { r, g, b };
        }
      }
    }

    const m = raw
      .replace(/\s+/g, "")
      .match(/^rgba?\((\d+),(\d+),(\d+)(?:,(\d*\.?\d+))?\)$/i);
    if (m) {
      return {
        r: Number.parseInt(m[1] ?? "0", 10),
        g: Number.parseInt(m[2] ?? "0", 10),
        b: Number.parseInt(m[3] ?? "0", 10),
      };
    }

    return { r: 249, g: 115, b: 22 };
  }, []);

  const stopAnimation = useCallback(() => {
    if (animationRef.current) {
      cancelAnimationFrame(animationRef.current);
      animationRef.current = null;
    }
  }, []);

  const cleanupMic = useCallback(() => {
    if (micWarmTimerRef.current) {
      window.clearTimeout(micWarmTimerRef.current);
      micWarmTimerRef.current = null;
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
    activeMicIdRef.current = null;

    lastPointsRef.current = null;
    drawPointsRef.current = null;
    smoothedPeakRef.current = 0;
    noiseFloorRef.current = 0;
    voiceEnergyRef.current = 0;
  }, []);

  const cleanupAudio = useCallback(() => {
    stopAnimation();
    cleanupMic();
  }, [cleanupMic, stopAnimation]);

  // If the overlay webview is actually hidden (window hidden / minimized / etc.),
  // release the mic so we don't keep the OS "mic in use" indicator on forever.
  // When visible again, the next active cycle will re-acquire as needed.
  useEffect(() => {
    const onVisibility = () => {
      if (document.hidden) {
        cleanupMic();
      }
    };

    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [cleanupMic]);

  useEffect(() => {
    if (!isVisible) {
      cleanupAudio();
      return;
    }

    // If we are actively recording/arming, don't let the warm-mic timer tear down the analyser.
    if (isActive && micWarmTimerRef.current) {
      window.clearTimeout(micWarmTimerRef.current);
      micWarmTimerRef.current = null;
    }

    if (!isActive) {
      // Idle animation (no mic permissions required)
      let mounted = true;
      const canvas = canvasRef.current;
      const ctx = canvas?.getContext("2d") ?? null;
      if (!canvas || !ctx) return;

      // Keep the mic/analyser warm for a short grace period after returning to idle.
      // This avoids the next recording having to pay getUserMedia/AudioContext startup latency.
      if (!micWarmTimerRef.current && analyserRef.current) {
        micWarmTimerRef.current = window.setTimeout(() => {
          micWarmTimerRef.current = null;
          cleanupMic();
        }, WARM_MIC_IDLE_TIMEOUT_MS);
      }

      // Stop any previous active/warmup draw loop, but do NOT stop the mic stream.
      stopAnimation();

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

        const { r, g, b } = getAccentRgb();
        const grad = ctx.createLinearGradient(0, 0, logicalW, 0);
        grad.addColorStop(0, `rgba(${r},${g},${b},0.18)`);
        grad.addColorStop(0.5, `rgba(${r},${g},${b},0.32)`);
        grad.addColorStop(1, `rgba(${r},${g},${b},0.18)`);

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
        // Only stop the idle animation loop for this effect instance.
        stopAnimation();
      };
    }

    // Active visualization: stop any idle loop.
    stopAnimation();

    let mounted = true;

    // Warmup animation: start drawing immediately while waiting for mic permissions
    // + AudioContext/analyser setup. This fixes the "waveform starts late" feel,
    // even though the backend may already be recording.
    const startWarmupAnimation = () => {
      const canvas = canvasRef.current;
      const ctx = canvas?.getContext("2d") ?? null;
      if (!canvas || !ctx) return;

      const points = 64;
      const wave = new Float32Array(points);

      const applyEdgeTaper = (v: number, i: number, n: number) => {
        const last = n - 1;
        if (last <= 0) return 0;
        const edgePoints = Math.max(8, Math.floor(n * 0.1));
        const d = Math.min(i, last - i);
        if (d >= edgePoints) return v;
        const t = d / edgePoints;
        const w = 0.5 - 0.5 * Math.cos(Math.PI * t);
        return v * w;
      };

      const drawWarmup = (t: number) => {
        if (!mounted) return;
        // If the analyser is ready, the real draw loop takes over.
        if (analyserRef.current) return;

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
        // Slightly more alive than idle: communicates "arming/listening".
        const amp = logicalH * 0.16;
        const phase = t / 210;
        const wobble = 0.72 + 0.28 * Math.sin(t / 330);
        for (let i = 0; i < points; i++) {
          const x = i / (points - 1);
          wave[i] =
            Math.sin(phase + x * Math.PI * 2) *
            wobble *
            (0.85 + 0.15 * Math.sin(phase * 0.7 + x * Math.PI * 4));
        }

        const { r, g, b } = getAccentRgb();
        const grad = ctx.createLinearGradient(0, 0, logicalW, 0);
        grad.addColorStop(0, `rgba(${r},${g},${b},0.28)`);
        grad.addColorStop(0.5, `rgba(${r},${g},${b},0.62)`);
        grad.addColorStop(1, `rgba(${r},${g},${b},0.28)`);

        ctx.save();
        ctx.lineWidth = 2;
        ctx.strokeStyle = grad;
        ctx.lineCap = "round";
        ctx.lineJoin = "round";
        ctx.beginPath();
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

        animationRef.current = requestAnimationFrame(drawWarmup);
      };

      animationRef.current = requestAnimationFrame(drawWarmup);
    };

    startWarmupAnimation();

    const startAnalyserDraw = (
      analyserNode: AnalyserNode,
      audioContext: AudioContext
    ) => {
      const canvas = canvasRef.current;
      if (!canvas) return;
      const ctx = canvas.getContext("2d");
      if (!ctx) return;

      const timeData = new Uint8Array(analyserNode.fftSize);
      const freqData = new Uint8Array(analyserNode.frequencyBinCount);

      const draw = () => {
        if (!mounted) return;
        if (!analyserRef.current) return;

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
        for (let i = 0; i < points; i++) {
          const a = Math.abs(next[i] ?? 0);
          if (a > peak) peak = a;
        }

        // Voice energy (0..1) based on *speech-band* frequency content.
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

        const totalRms = totalCount > 0 ? Math.sqrt(totalSum / totalCount) : 0;
        const speechRms =
          speechCount > 0 ? Math.sqrt(speechSum / speechCount) : 0;
        const speechRatio = totalRms > 1e-6 ? speechRms / totalRms : 0;

        // Adaptive floor - only rises when speechRatio is LOW (noise-like)
        const prevNoiseFloor = noiseFloorRef.current;
        const isNoiseLike = speechRatio < 1.02; // Broadband = noise
        const riseRate = isNoiseLike ? 0.08 : 0.002;
        const fallRate = 0.01;
        const nfRate = speechRms > prevNoiseFloor ? riseRate : fallRate;
        const noiseFloor =
          prevNoiseFloor + (speechRms - prevNoiseFloor) * nfRate;
        noiseFloorRef.current = noiseFloor;

        const aboveFloor = Math.max(0, speechRms - noiseFloor * 0.8);
        const voiceLikelihood = Math.max(
          0,
          Math.min(1, (speechRatio - 1.0) * 8)
        );
        const rawEnergy = Math.pow(Math.min(1, aboveFloor / 0.015), 0.6);
        const voiceEnergyInstant = rawEnergy * (0.2 + voiceLikelihood * 0.8);

        const prevVE = voiceEnergyRef.current;
        const attack = 0.55;
        const decay = 0.12;
        const ve =
          voiceEnergyInstant > prevVE
            ? prevVE * (1 - attack) + voiceEnergyInstant * attack
            : prevVE * (1 - decay) + voiceEnergyInstant * decay;
        voiceEnergyRef.current = ve;

        const prevPeak = smoothedPeakRef.current;
        const decayed = prevPeak * 0.9;
        smoothedPeakRef.current = Math.max(peak, decayed);

        const effectivePeak = Math.max(0.02, smoothedPeakRef.current);
        const targetPeak = 0.9 + ve * 0.8;
        const maxGain = 3.0 + ve * 50;
        const gain = Math.min(maxGain, Math.max(1, targetPeak / effectivePeak));

        for (let i = 0; i < points; i++) {
          const v = (next[i] ?? 0) * gain;
          next[i] = Math.tanh(v * 0.5) * 2.0;
        }

        const prev = lastPointsRef.current;
        if (!prev || prev.length !== points) {
          lastPointsRef.current = next;
        } else {
          const responsiveness = 0.25;
          const hold = 1 - responsiveness;
          for (let i = 0; i < points; i++) {
            const prevVal = prev[i] ?? 0;
            const nextVal = next[i] ?? 0;
            prev[i] = prevVal * hold + nextVal * responsiveness;
          }
          lastPointsRef.current = prev;
        }

        const drawWave = lastPointsRef.current ?? next;

        const applyEdgeTaper = (v: number, i: number, n: number) => {
          const last = n - 1;
          if (last <= 0) return 0;
          const edgePoints = Math.max(8, Math.floor(n * 0.1));
          const d = Math.min(i, last - i);
          if (d >= edgePoints) return v;
          const t = d / edgePoints;
          const w = 0.5 - 0.5 * Math.cos(Math.PI * t);
          return v * w;
        };

        const midY = logicalH / 2;
        const maxAmp = Math.max(1, logicalH / 2 - 1);
        const amp = maxAmp * 0.95;

        const { r, g, b } = getAccentRgb();
        const colorMid = `rgba(${r},${g},${b},0.90)`;
        const colorEdge = `rgba(${r},${g},${b},0.55)`;

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
          ctx.shadowColor = `rgba(${r},${g},${b},0.35)`;
          ctx.shadowBlur = blur;

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

        drawPath(4, 0.28, 8);
        drawPath(2, 0.92, 0);

        animationRef.current = requestAnimationFrame(draw);
      };

      draw();
    };

    // If we already have a warm analyser for the selected mic, start drawing immediately.
    if (analyserRef.current && audioContextRef.current) {
      const currentMic = activeMicIdRef.current;
      const desiredMic = selectedMicId ?? null;
      const micMatches = (currentMic ?? null) === (desiredMic ?? null);
      if (micMatches) {
        // Stop warmup loop now that the real waveform can run.
        stopAnimation();
        startAnalyserDraw(analyserRef.current, audioContextRef.current);
        return () => {
          mounted = false;
          stopAnimation();
        };
      }

      // Mic changed: tear down the warm mic so setupAudio can acquire the new device.
      cleanupMic();
    }

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
        activeMicIdRef.current = selectedMicId ?? null;
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

        // Stop the warmup loop now that we have a real analyser-driven loop.
        stopAnimation();
        startAnalyserDraw(analyser, audioContext);
      } catch (error) {
        console.error("[AudioWave] Failed to setup audio:", error);
      }
    };

    setupAudio();

    return () => {
      mounted = false;
      stopAnimation();
    };
  }, [
    cleanupAudio,
    cleanupMic,
    getAccentRgb,
    isActive,
    isVisible,
    selectedMicId,
    stopAnimation,
  ]);

  return (
    <canvas
      ref={canvasRef}
      width={168}
      height={24}
      className={["overlay-wave", className].filter(Boolean).join(" ")}
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

  useEffect(() => {
    applyAccentColor(settings?.accent_color);
  }, [settings?.accent_color]);

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
      // During an active capture cycle, prioritize responsiveness over avoiding a
      // one-frame clipped border: render immediately so the waveform can warm up.
      if (pipelineState !== "idle") {
        setRenderExpanded(true);
      } else {
        setRenderExpanded(false);
      }
      tauriAPI.resizeOverlay(264, 56);
      return;
    }

    // Collapse: hide expanded immediately, then shrink window.
    setRenderExpanded(false);
    tauriAPI.resizeOverlay(56, 56);
  }, [expanded, pipelineState]);

  useEffect(() => {
    if (!expanded) return;
    // If we're active, we already rendered immediately above.
    if (pipelineState !== "idle") return;

    if (rect.width >= 260) {
      setRenderExpanded(true);
    }
  }, [expanded, pipelineState, rect.width]);

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
      pipelineState === "arming" ||
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
      pipelineState === "arming" ||
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

    // Optimistic UX: the backend begins capturing before the UI can receive events.
    // Show "REC" immediately so the overlay matches when the user can start talking.
    setPipelineState("recording");

    try {
      await invoke("pipeline_start_recording");

      // If recording is already active, reflect it immediately.
      // This reduces the confusing "Arm" state when the backend is already capturing.
      try {
        const state = await invoke<string>("pipeline_get_state");
        if (isPipelineState(state)) {
          setPipelineState(state);
        }
      } catch {
        // If polling fails, we'll still rely on event listeners / interval polling.
      }
    } catch (error) {
      console.error("[Pipeline] Failed to start recording:", error);
      const errorInfo = parseError(error);
      setLastError(errorInfo);
      setLastErrorDetail(String(error));

      setPipelineState("error");
    }
  }, [pipelineState]);

  // Stop recording and transcribe
  const onStopRecording = useCallback(async () => {
    if (pipelineState !== "recording") return;

    try {
      // UX: once the user stops, always show "transcribing" (even if the backend
      // ends up short-circuiting due to quiet-audio gating).
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
        // Hotkey events can arrive slightly before the overlay receives any other
        // pipeline events; still show "REC" immediately to match actual capture.
        setLastError(null);
        setLastErrorDetail(null);
        setLastFailedRequestId(null);
        setPipelineState("recording");
      });
      unlistenStop = await tauriAPI.onStopRecording(() => {
        // UX: once the user stops, always show "transcribing".
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
      unlisten = await tauriAPI.onSettingsChanged(async (payload) => {
        // Apply accent immediately (without waiting on any disk reload).
        try {
          const maybeObj = payload as unknown;
          if (maybeObj && typeof maybeObj === "object") {
            const accent = (maybeObj as Record<string, unknown>).accent_color;
            if (accent === null || typeof accent === "string") {
              applyAccentColor(accent);
            }
          }
        } catch (error) {
          console.error("[Overlay] Failed to apply accent payload:", error);
        }

        // In the overlay window, force a disk reload so *all* settings fields reflect
        // the latest changes made by the main window.
        try {
          await tauriAPI.reloadSettingsFromDisk();
        } catch (error) {
          console.error(
            "[Overlay] Failed to reload settings from disk:",
            error
          );
        }

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

    if (pipelineState === "arming") {
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
  const isArming = pipelineState === "arming";
  const isRecording = pipelineState === "recording";
  const isWaveActive = isArming || isRecording;
  const isBusy = isArming || isLoading;
  const isError = pipelineState === "error";
  const centerPhaseText =
    pipelineState === "rewriting"
      ? "rewriting..."
      : pipelineState === "transcribing"
      ? "transcribing..."
      : null;

  const renderIcon = () => {
    if (isLoading) {
      return <Loader size="xs" style={{ color: "var(--accent-primary)" }} />;
    }
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
            disabled={isBusy}
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
            disabled={isBusy}
            className="overlay-button overlay-button--expanded"
            style={
              isError ? { background: "rgba(127, 29, 29, 0.92)" } : undefined
            }
          >
            <div className="overlay-icon">{renderIcon()}</div>
            <div
              className={`overlay-center${
                isError && lastError ? " overlay-center--error" : ""
              }`}
            >
              {isError && lastError ? (
                <div
                  style={{ display: "flex", flexDirection: "column", gap: 4 }}
                >
                  <div
                    className="overlay-error-text"
                    title={lastError.message}
                    tabIndex={0}
                    onMouseEnter={(e) => {
                      // Ensure we show the beginning of the message (not a scrolled midpoint).
                      e.currentTarget.scrollLeft = 0;
                    }}
                    onFocus={(e) => {
                      e.currentTarget.scrollLeft = 0;
                    }}
                  >
                    {lastError.message}
                  </div>
                </div>
              ) : centerPhaseText ? (
                <div className="overlay-phase-text" aria-live="polite">
                  {centerPhaseText}
                </div>
              ) : (
                <>
                  {/* Backend-driven waveform (no getUserMedia startup lag).
                      While "arming" (UI-only), keep an idle animation so the overlay
                      doesn't look dead before recording actually starts. */}
                  {isWaveActive ? (
                    <BackendAudioWave
                      isActive={true}
                      isVisible={true}
                      className={isArming ? "overlay-wave--arming" : undefined}
                    />
                  ) : (
                    <AudioWave
                      isActive={false}
                      isVisible={true}
                      selectedMicId={settings?.selected_mic_id ?? null}
                      className={isArming ? "overlay-wave--arming" : undefined}
                    />
                  )}
                </>
              )}
            </div>
            <div className="overlay-meta">
              <div
                className="overlay-pill"
                data-variant={
                  isError
                    ? "dim"
                    : isRecording
                    ? "rec"
                    : isArming
                    ? "arming"
                    : "dim"
                }
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
                {isError
                  ? "Retry"
                  : isRecording
                  ? "REC"
                  : isLoading
                  ? "..."
                  : "Tap"}
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
        <Loader size="xs" color="orange" />
      </div>
    );
  }

  return <RecordingControl />;
}
