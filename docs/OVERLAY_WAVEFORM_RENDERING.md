# Overlay waveform rendering (UI)

This document explains how the **overlay waveform** is rendered in the app UI.

Source of truth: `app/src/OverlayApp.tsx` (`AudioWave` component).

---

## At a glance (plain English)

When the overlay is expanded, it shows a small waveform line.

- If we’re **not actively recording** (`isActive = false`), we draw a **gentle idle animation** (no mic required).
- If we **are actively recording** (`isActive = true`), we:
  1. open the selected microphone,
  2. filter the audio to focus on speech frequencies,
  3. analyze it in both time-domain (for the shape) and frequency-domain (for “is it voice?”),
  4. suppress background noise so the line doesn’t wiggle constantly,
  5. smooth the waveform (in time and across the line),
  6. draw it with a soft glow.

---

## Files and responsibilities

- `app/src/OverlayApp.tsx`

  - `AudioWave`: owns the audio setup, analysis, and canvas drawing.
  - The waveform is drawn on a `<canvas class="overlay-wave" />`.

- `app/src/app.css`
  - `.overlay-wave`: sets the logical size (168×24) used by the drawing loop.

---

## Component structure

### `AudioWave` props

- `isActive: boolean`

  - `false`: idle (fake) animation.
  - `true`: active mic visualization.

- `isVisible?: boolean`

  - When `false`, we tear down audio + animation.

- `selectedMicId?: string | null`
  - The device id for `getUserMedia`.

### Important refs (state held outside React renders)

- `canvasRef`: the canvas element.
- `animationRef`: requestAnimationFrame id (so we can cancel it).
- `analyserRef`: WebAudio `AnalyserNode` for signal inspection.
- `streamRef`: the `MediaStream` so we can stop tracks.
- `audioContextRef`: `AudioContext` so we can close it.
- `lastPointsRef`: the last displayed waveform points (temporal smoothing buffer).
- `drawPointsRef`: a second buffer used for spatial smoothing before drawing.
- `smoothedPeakRef`: tracks peak amplitude over time (used for gain control).
- `noiseFloorRef`: tracks an adaptive “speech-band floor” (used to suppress noise).

---

## Rendering modes

## 1) Idle animation mode (no mic)

### What it does (simple)

- Generates a sine-like wave that slowly moves.
- Keeps the amplitude small so it looks subtle.

### How it works (technical)

- A fixed number of points are computed each frame.
- A gradient stroke is used.
- The line is drawn as a polyline:
  - `moveTo` for the first point, `lineTo` for the rest.

### Endpoint behavior

- Before drawing, values are tapered near the ends using a raised-cosine window.
- X coordinates are inset a little (`xPad`) so stroke caps aren’t clipped.

---

## 2) Active mic visualization mode

This is the main pipeline. Think of it in layers:

1. **Acquire audio**
2. **Filter**
3. **Analyze** (time + frequency)
4. **Decide: voice vs noise**
5. **Normalize / shape** the waveform
6. **Smooth** (over time + across points)
7. **Draw**

### 2.1 Acquire audio (getUserMedia)

#### Simple

- We request microphone input.
- If a specific mic device id fails, we fall back to default mic.

#### Technical

- Uses `navigator.mediaDevices.getUserMedia(...)` with constraints aiming to disable browser “helpful” processing:
  - `echoCancellation: false`
  - `noiseSuppression: false`
  - `autoGainControl: false` (typed via a cast since TS DOM types can lag)

### 2.2 AudioContext + resume

#### Simple

- Creates an `AudioContext` and ensures it’s running.

#### Technical

- Some browsers can start the context suspended if not created in a user gesture.
- We call `audioContext.resume()` defensively.

### 2.3 Filtering (speech-focused)

#### Simple

- Removes low rumble (bus/handling noise) and extreme highs.

#### Technical

Two `BiquadFilterNode`s are inserted before analysis:

- High-pass filter:

  - type: `highpass`
  - cutoff: ~180 Hz

- Low-pass filter:
  - type: `lowpass`
  - cutoff: ~3800 Hz

Connection:

`MediaStreamSource -> highpass -> lowpass -> analyser`

This ensures both the waveform shape and the FFT analysis are driven by the filtered signal.

### 2.4 Analyser configuration

#### Simple

- The analyser provides both the waveform shape and frequency energy.

#### Technical

- `analyser.fftSize = 2048`
- `analyser.smoothingTimeConstant` is set lower than the earlier iterations to avoid long “hang”
  - We do our own smoothing as well.

Buffers:

- `timeData = new Uint8Array(analyser.fftSize)`
- `freqData = new Uint8Array(analyser.frequencyBinCount)`

Each frame:

- `getByteTimeDomainData(timeData)`
- `getByteFrequencyData(freqData)`

### 2.5 Downsample time-domain waveform (shape)

#### Simple

- We pick a small number of points from the waveform so it draws smoothly.

#### Technical

- We resample `timeData` into `next: Float32Array(points)`.
- Conversion:
  - time-domain bytes are centered at 128
  - normalized to approximately `[-1, 1]`

Currently:

- `points = 64` (reduced from earlier values to look smoother/less spiky)

### 2.6 Compute raw peak (for gain)

#### Simple

- Finds how “big” the waveform is so we can scale it.

#### Technical

- `peak = max(abs(next[i]))` across points
- `smoothedPeakRef` stores a decaying version so gain doesn’t jump wildly.

### 2.7 Voice detection / “voiceEnergy” (speech-band)

#### Simple

- We estimate “how voice-like and how loud” the audio is.
- If it looks like background noise, we keep the waveform flat.

#### Technical

We compute a speech metric from FFT bins:

- Convert bin index to Hz:

  - `binHz = sampleRate / fftSize`

- Bands:
  - speech band: 300–3400 Hz
  - total band: 60–8000 Hz

We compute RMS-like energy in those bands using squared magnitudes:

- `speechRms = sqrt(mean(v^2))` over speech bins
- `totalRms = sqrt(mean(v^2))` over total bins

We compute a ratio:

- `speechRatio = speechRms / totalRms`

And combine:

- `speechMetric = speechRms * clamp(speechRatio, 0..1.6)`

### 2.8 Adaptive noise floor (speech band)

#### Simple

- The app learns the “background level” so constant noise doesn’t animate.

#### Technical

- `noiseFloorRef` tracks the floor of `speechMetric`.
- It uses different rise/fall rates:
  - rises faster for small sustained increases
  - rises slower for large spikes
  - falls moderately so it can recover

This is meant to reduce “bus noise wiggle” while not immediately learning your voice as “background.”

### 2.9 Gate + deadzone + soft clipping

#### Simple

- If it’s not voice, we flatten the waveform.
- If it is voice, we still remove tiny wiggles.

#### Technical

- `isSilent = voiceEnergy < threshold` then:

  - `next.fill(0)`
  - and we also snap the display buffer to 0 (see smoothing section)

- When not silent:
  - apply `gain`
  - apply a `deadzone` (small values become 0)
  - apply `tanh` soft-clipping so big peaks don’t become harsh flat tops

### 2.10 Temporal smoothing (frame-to-frame)

#### Simple

- The waveform moves gradually rather than jittering.

#### Technical

- `lastPointsRef` is a buffer that is blended toward the new frame.

Formula:

- `prev[i] = prev[i] * (1 - r) + next[i] * r`

Where `r` (“responsiveness”) is chosen to be smaller now so motion is more gradual.

Important: when `isSilent` we **snap**:

- `prev.fill(0)`
- reset peak tracking

This prevents multi-second decay tails.

### 2.11 Spatial smoothing (across points)

#### Simple

- Removes jagged “teeth” and tiny spikes along the line.

#### Technical

- `drawPointsRef` holds the post-smoothed line.
- Each point is a weighted blend of neighbors (5-tap kernel):
  - `[0.15, 0.20, 0.30, 0.20, 0.15]`

This is essentially a small low-pass filter across the polyline.

### 2.12 Endpoint taper + padding

#### Simple

- The waveform fades to the baseline at both ends.
- Prevents the ends from looking chopped/clipped.

#### Technical

Two parts:

1. **Edge taper** (raised cosine window) applied to the values near ends.
2. **X padding** (`xPad`) so stroke caps and glow aren’t clipped by canvas bounds.

### 2.13 Drawing (canvas)

#### Simple

- Draws the line twice: a soft glow and a crisp line.

#### Technical

- Uses HiDPI scaling:

  - backing store scaled by `devicePixelRatio` and then `ctx.setTransform(dpr, ...)`

- Gradient stroke:

  - brighter in the center, slightly dimmer toward edges

- Two passes:
  1. glow: larger lineWidth, lower alpha, shadow blur
  2. crisp: thinner lineWidth, higher alpha, no blur

Line styling:

- `lineCap = "round"`
- `lineJoin = "round"`

---

## How to tune behavior (practical knobs)

If you want to adjust how it feels, these are the main levers inside `AudioWave`:

- Voice gating aggressiveness:

  - `noiseMargin`
  - mapping from `above -> voiceEnergy`
  - `isSilent` threshold

- Smoothness:

  - number of plotted `points`
  - temporal smoothing responsiveness (`r`)
  - spatial smoothing kernel weights

- Voice “pop” / visual intensity:

  - `maxGain`, `targetPeak`, effective peak floor
  - `amp` scaling vs `voiceEnergy`

- Noise rejection:
  - highpass cutoff frequency
  - speech/total band choices (Hz ranges)
  - noise-floor rise/fall rates

---

## Notes / known limitations

- Voice-vs-noise detection is heuristic; non-voice sounds with strong energy in 300–3400 Hz (e.g., loud speech around you, some machinery) can still animate.
- The waveform is a visualization, not a VAD (voice activity detection) used for recording.
- The UI shows only a tiny canvas (24px tall), so small parameter changes can appear subtle.

---

## Pointers for further improvements (optional)

- Replace the polyline with a spline (Catmull–Rom / quadratic curves) for an even smoother look.
- Add an optional “aggressive noise suppression” toggle for travel scenarios.
- Expose a single “Waveform sensitivity” slider that adjusts a bundle of parameters together.
