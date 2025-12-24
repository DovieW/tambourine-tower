//! Audio capture module using cpal for cross-platform audio input.
//!
//! This module provides functionality to capture audio from the system's
//! default input device and encode it to WAV format for STT processing.
//!
//! Supports optional Voice Activity Detection (VAD) for auto-stop functionality.

use crate::vad::{VadConfig, VadEvent, VadFrameProcessor};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use hound::{WavSpec, WavWriter};
use std::io::Cursor;
use std::sync::mpsc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::thread::{self, JoinHandle};

fn clamp_u8_0_100(v: u8) -> u8 {
    v.min(100)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn db_to_amp(db: f32) -> f32 {
    // db is dBFS; 0 dBFS == full-scale amplitude (1.0).
    10.0_f32.powf(db / 20.0)
}

/// Apply a simple noise gate to interleaved samples.
///
/// - `samples` are interleaved f32 in [-1, 1]
/// - `channels` is the interleaving width
/// - `strength` is 0..=100 (0 => bypass)
///
/// This is intentionally lightweight and runs at stop-time (offline), not in the
/// real-time capture callback.
fn apply_noise_gate_interleaved(
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    strength: u8,
) -> Vec<f32> {
    let strength = clamp_u8_0_100(strength);
    if strength == 0 || samples.is_empty() {
        return samples.to_vec();
    }

    let channels_usize = channels.max(1) as usize;
    let frames = samples.len() / channels_usize;

    // Map 0..100 -> threshold dBFS range. Higher threshold => more aggressive gating.
    // Tuned to be conservative at low strengths.
    let t = strength as f32 / 100.0;
    let threshold_dbfs = lerp(-75.0, -30.0, t);
    let threshold_amp = db_to_amp(threshold_dbfs);

    // Hysteresis reduces "chattering" around the threshold.
    let close_threshold_amp = threshold_amp * 0.85;

    // Attack/release smoothing on the *gain* to avoid clicks.
    let fs = sample_rate.max(1) as f32;
    let attack_s = 0.005_f32;
    let release_s = 0.120_f32;

    let attack_alpha = (-1.0 / (attack_s * fs)).exp();
    let release_alpha = (-1.0 / (release_s * fs)).exp();

    let mut out = Vec::with_capacity(samples.len());
    let mut gate_open = false;
    let mut gain: f32 = 0.0;

    for frame_idx in 0..frames {
        let base = frame_idx * channels_usize;

        // Envelope: max abs across channels for this frame.
        let mut env = 0.0_f32;
        for c in 0..channels_usize {
            env = env.max(samples[base + c].abs());
        }

        if gate_open {
            if env < close_threshold_amp {
                gate_open = false;
            }
        } else if env > threshold_amp {
            gate_open = true;
        }

        let target = if gate_open { 1.0_f32 } else { 0.0_f32 };
        let alpha = if target > gain { attack_alpha } else { release_alpha };
        gain = target + alpha * (gain - target);

        for c in 0..channels_usize {
            out.push(samples[base + c] * gain);
        }
    }

    // If there were trailing samples not forming a whole frame, just copy them.
    // (Shouldn't happen in normal capture, but keep it safe.)
    let consumed = frames * channels_usize;
    if consumed < samples.len() {
        out.extend_from_slice(&samples[consumed..]);
    }

    out
}

/// Errors that can occur during audio capture
#[derive(Debug, thiserror::Error)]
pub enum AudioCaptureError {
    #[error("No input device available")]
    NoInputDevice,

    #[error("Failed to get device config: {0}")]
    DeviceConfig(String),

    #[error("Failed to build audio stream: {0}")]
    StreamBuild(String),

    #[error("Failed to start audio stream: {0}")]
    StreamStart(String),

    #[error("Failed to encode audio: {0}")]
    Encoding(String),

    #[error("Audio capture not active")]
    #[cfg_attr(not(test), allow(dead_code))]
    NotActive,

    #[error("Capture thread error: {0}")]
    #[cfg_attr(not(test), allow(dead_code))]
    ThreadError(String),
}

/// Audio buffer that accumulates samples during recording
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    samples: Vec<f32>,
    sample_rate: u32,
    channels: u16,
    max_duration_secs: f32,
}

impl AudioBuffer {
    /// Create a new audio buffer with the specified parameters
    pub fn new(sample_rate: u32, channels: u16, max_duration_secs: f32) -> Self {
        let capacity = (sample_rate as f32 * max_duration_secs * channels as f32) as usize;
        Self {
            samples: Vec::with_capacity(capacity),
            sample_rate,
            channels,
            max_duration_secs,
        }
    }

    /// Append samples to the buffer
    pub fn append(&mut self, new_samples: &[f32]) {
        self.samples.extend_from_slice(new_samples);

        // Trim if exceeds max duration
        let max_samples =
            (self.sample_rate as f32 * self.max_duration_secs * self.channels as f32) as usize;
        if self.samples.len() > max_samples {
            let drain_count = self.samples.len() - max_samples;
            self.samples.drain(0..drain_count);
        }
    }

    /// Clear all samples from the buffer
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Get the number of samples in the buffer
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Check if the buffer is empty
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Get the duration of audio in the buffer in seconds
    pub fn duration_secs(&self) -> f32 {
        self.samples.len() as f32 / (self.sample_rate as f32 * self.channels as f32)
    }

    /// Compute simple signal level statistics over the captured samples.
    ///
    /// Samples are expected to be normalized floats in [-1.0, 1.0].
    pub fn level_stats(&self) -> AudioLevelStats {
        let mut peak: f32 = 0.0;
        let mut sum_sq: f64 = 0.0;
        let mut n: u64 = 0;

        for &s in &self.samples {
            let a = s.abs();
            if a > peak {
                peak = a;
            }

            // Promote to f64 for numerical stability on long recordings.
            sum_sq += (s as f64) * (s as f64);
            n += 1;
        }

        let rms = if n == 0 {
            0.0
        } else {
            (sum_sq / n as f64).sqrt() as f32
        };

        AudioLevelStats {
            duration_secs: self.duration_secs(),
            rms,
            peak,
        }
    }

    /// Convert the buffer contents to WAV bytes
    pub fn to_wav_bytes(&self) -> Result<Vec<u8>, AudioCaptureError> {
        self.to_wav_bytes_with_noise_gate(0)
    }

    /// Convert the buffer contents to WAV bytes, optionally applying an experimental noise gate.
    ///
    /// `noise_gate_strength` is 0..=100 where 0 disables the noise gate.
    pub fn to_wav_bytes_with_noise_gate(
        &self,
        noise_gate_strength: u8,
    ) -> Result<Vec<u8>, AudioCaptureError> {
        let processed = apply_noise_gate_interleaved(
            &self.samples,
            self.sample_rate,
            self.channels,
            noise_gate_strength,
        );

        let spec = WavSpec {
            channels: self.channels,
            sample_rate: self.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = WavWriter::new(&mut cursor, spec)
                .map_err(|e| AudioCaptureError::Encoding(e.to_string()))?;

            for &sample in &processed {
                // Convert f32 [-1.0, 1.0] to i16
                let sample_i16 = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                writer
                    .write_sample(sample_i16)
                    .map_err(|e| AudioCaptureError::Encoding(e.to_string()))?;
            }

            writer
                .finalize()
                .map_err(|e| AudioCaptureError::Encoding(e.to_string()))?;
        }

        Ok(cursor.into_inner())
    }

    /// Get the sample rate
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Get the number of channels
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn channels(&self) -> u16 {
        self.channels
    }
}

/// Basic audio level metrics for gating/diagnostics.
#[derive(Debug, Clone, Copy)]
pub struct AudioLevelStats {
    pub duration_secs: f32,
    /// Root-mean-square amplitude in [0, 1].
    pub rms: f32,
    /// Peak (max absolute) amplitude in [0, 1].
    pub peak: f32,
}

/// Realtime-safe snapshot of the most recent input level.
///
/// Updated by the CPAL input callback using atomics (no allocations, no event emission).
#[derive(Debug, Clone, Copy)]
pub struct AudioLevelSnapshot {
    pub seq: u64,
    /// Root-mean-square amplitude in [0, 1] for the most recent callback chunk.
    pub rms: f32,
    /// Peak (max abs) amplitude in [0, 1] for the most recent callback chunk.
    pub peak: f32,
}

/// Number of min/max buckets sent to the overlay for waveform rendering.
///
/// Keep this modest: payload size is 2 * N floats per frame.
pub const WAVEFORM_BINS: usize = 64;

/// Realtime-safe snapshot of the most recent min/max waveform buckets.
///
/// `mins[i]` and `maxes[i]` are in [-1, 1] representing the min/max sample value
/// for that bucket.
#[derive(Debug, Clone)]
pub struct AudioWaveformSnapshot {
    pub seq: u64,
    pub mins: Vec<f32>,
    pub maxes: Vec<f32>,
}

/// A cheap-to-clone handle for reading realtime waveform buckets without needing
/// to borrow the full `AudioCapture`.
#[derive(Clone)]
pub struct SharedAudioWaveformMeter {
    inner: Arc<AudioWaveformMeter>,
}

impl SharedAudioWaveformMeter {
    pub fn snapshot(&self) -> AudioWaveformSnapshot {
        self.inner.snapshot()
    }
}

/// A cheap-to-clone handle for reading realtime audio levels without needing to
/// borrow the full `AudioCapture`.
///
/// This wrapper avoids exposing the internal `AudioLevelMeter` implementation.
#[derive(Clone)]
pub struct SharedAudioLevelMeter {
    inner: Arc<AudioLevelMeter>,
}

impl SharedAudioLevelMeter {
    pub fn snapshot(&self) -> AudioLevelSnapshot {
        self.inner.snapshot()
    }
}

#[derive(Debug)]
struct AudioWaveformMeter {
    seq: AtomicU64,
    min_bits: [AtomicU32; WAVEFORM_BINS],
    max_bits: [AtomicU32; WAVEFORM_BINS],
}

impl Default for AudioWaveformMeter {
    fn default() -> Self {
        Self {
            seq: AtomicU64::new(0),
            min_bits: std::array::from_fn(|_| AtomicU32::new(0f32.to_bits())),
            max_bits: std::array::from_fn(|_| AtomicU32::new(0f32.to_bits())),
        }
    }
}

impl AudioWaveformMeter {
    fn snapshot(&self) -> AudioWaveformSnapshot {
        let seq = self.seq.load(Ordering::Relaxed);
        let mut mins = Vec::with_capacity(WAVEFORM_BINS);
        let mut maxes = Vec::with_capacity(WAVEFORM_BINS);
        for i in 0..WAVEFORM_BINS {
            mins.push(f32::from_bits(self.min_bits[i].load(Ordering::Relaxed)));
            maxes.push(f32::from_bits(self.max_bits[i].load(Ordering::Relaxed)));
        }
        AudioWaveformSnapshot { seq, mins, maxes }
    }

    fn update_from_f32_interleaved(&self, data: &[f32], channels: usize) {
        let channels = channels.max(1);
        let frames = data.len() / channels;
        if frames == 0 {
            return;
        }

        for bin in 0..WAVEFORM_BINS {
            let start = (bin * frames) / WAVEFORM_BINS;
            let end = ((bin + 1) * frames) / WAVEFORM_BINS;
            if start >= end {
                self.min_bits[bin].store(0f32.to_bits(), Ordering::Relaxed);
                self.max_bits[bin].store(0f32.to_bits(), Ordering::Relaxed);
                continue;
            }

            let mut min_v: f32 = 1.0;
            let mut max_v: f32 = -1.0;

            for frame in start..end {
                let base = frame * channels;
                let mut acc: f32 = 0.0;
                for c in 0..channels {
                    acc += data.get(base + c).copied().unwrap_or(0.0);
                }
                let s = (acc / channels as f32).clamp(-1.0, 1.0);
                if s < min_v {
                    min_v = s;
                }
                if s > max_v {
                    max_v = s;
                }
            }

            self.min_bits[bin].store(min_v.to_bits(), Ordering::Relaxed);
            self.max_bits[bin].store(max_v.to_bits(), Ordering::Relaxed);
        }

        self.seq.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug, Default)]
struct AudioLevelMeter {
    seq: AtomicU64,
    rms_bits: AtomicU32,
    peak_bits: AtomicU32,
}

impl AudioLevelMeter {
    fn snapshot(&self) -> AudioLevelSnapshot {
        let seq = self.seq.load(Ordering::Relaxed);
        let rms = f32::from_bits(self.rms_bits.load(Ordering::Relaxed));
        let peak = f32::from_bits(self.peak_bits.load(Ordering::Relaxed));
        AudioLevelSnapshot { seq, rms, peak }
    }

    fn update(&self, rms: f32, peak: f32) {
        // Clamp to sane range and avoid NaNs propagating into the UI.
        let rms = if rms.is_finite() { rms.clamp(0.0, 1.0) } else { 0.0 };
        let peak = if peak.is_finite() { peak.clamp(0.0, 1.0) } else { 0.0 };

        self.rms_bits.store(rms.to_bits(), Ordering::Relaxed);
        self.peak_bits.store(peak.to_bits(), Ordering::Relaxed);
        self.seq.fetch_add(1, Ordering::Relaxed);
    }
}

/// Commands sent to the audio capture thread
enum CaptureCommand {
    Stop,
}

/// VAD events sent from the capture thread
#[derive(Debug, Clone)]
pub enum AudioCaptureEvent {
    /// Speech detected (with pre-roll audio)
    SpeechStart,
    /// Speech ended after hangover period
    SpeechEnd,
}

/// Configuration for VAD-based auto-stop
#[derive(Debug, Clone)]
pub struct VadAutoStopConfig {
    /// Enable VAD processing
    pub enabled: bool,
    /// Automatically stop recording when speech ends
    #[cfg_attr(not(test), allow(dead_code))]
    pub auto_stop: bool,
    /// VAD configuration
    pub vad_config: VadConfig,
}

impl Default for VadAutoStopConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_stop: false,
            vad_config: VadConfig::default(),
        }
    }
}

/// Handle to a running audio capture session
struct CaptureHandle {
    command_tx: mpsc::Sender<CaptureCommand>,
    #[cfg_attr(not(test), allow(dead_code))]
    event_rx: mpsc::Receiver<AudioCaptureEvent>,
    thread_handle: JoinHandle<Result<(), AudioCaptureError>>,
}

/// Thread-safe audio capture manager
///
/// This runs audio capture in a separate thread to avoid Send/Sync issues
/// with cpal::Stream. The captured audio is stored in a shared buffer.
pub struct AudioCapture {
    buffer: Arc<StdMutex<AudioBuffer>>,
    capture_handle: Option<CaptureHandle>,
    sample_rate: u32,
    channels: u16,
    vad_config: VadAutoStopConfig,

    // Most recent realtime level stats (for UI metering / overlay waveform).
    level_meter: Arc<AudioLevelMeter>,

    // Most recent realtime waveform buckets (for true waveform rendering).
    waveform_meter: Arc<AudioWaveformMeter>,
}

impl AudioCapture {
    /// Create a new audio capture instance
    pub fn new() -> Self {
        Self {
            buffer: Arc::new(StdMutex::new(AudioBuffer::new(44100, 1, 300.0))),
            capture_handle: None,
            sample_rate: 44100,
            channels: 1,
            vad_config: VadAutoStopConfig::default(),
            level_meter: Arc::new(AudioLevelMeter::default()),
            waveform_meter: Arc::new(AudioWaveformMeter::default()),
        }
    }

    /// Create a new audio capture instance with VAD configuration
    pub fn with_vad_config(vad_config: VadAutoStopConfig) -> Self {
        Self {
            buffer: Arc::new(StdMutex::new(AudioBuffer::new(44100, 1, 300.0))),
            capture_handle: None,
            sample_rate: 44100,
            channels: 1,
            vad_config,
            level_meter: Arc::new(AudioLevelMeter::default()),
            waveform_meter: Arc::new(AudioWaveformMeter::default()),
        }
    }

    /// Get the most recent realtime input level snapshot.
    ///
    /// This is updated continuously while recording (per CPAL callback). When not recording,
    /// it returns the last observed values.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn level_snapshot(&self) -> AudioLevelSnapshot {
        self.level_meter.snapshot()
    }

    pub fn shared_level_meter(&self) -> SharedAudioLevelMeter {
        SharedAudioLevelMeter {
            inner: self.level_meter.clone(),
        }
    }

    pub fn shared_waveform_meter(&self) -> SharedAudioWaveformMeter {
        SharedAudioWaveformMeter {
            inner: self.waveform_meter.clone(),
        }
    }

    /// Update VAD configuration
    pub fn set_vad_config(&mut self, config: VadAutoStopConfig) {
        self.vad_config = config;
    }

    /// Get the current VAD configuration
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn vad_config(&self) -> &VadAutoStopConfig {
        &self.vad_config
    }

    /// Start recording audio from the default input device.
    ///
    /// Prefer `start_with_device_name` when you need to honor a user-selected mic.
    ///
    /// # Arguments
    /// * `max_duration_secs` - Maximum recording duration in seconds (for buffer sizing)
    pub fn start(&mut self, max_duration_secs: f32) -> Result<(), AudioCaptureError> {
        self.start_with_device_name(max_duration_secs, None)
    }

    /// Start recording audio from a specific input device (by CPAL device name),
    /// falling back to the system default if not found.
    pub fn start_with_device_name(
        &mut self,
        max_duration_secs: f32,
        input_device_name: Option<&str>,
    ) -> Result<(), AudioCaptureError> {
        // Stop any existing recording
        self.stop();

        // Get device info first (on main thread)
        let host = cpal::default_host();

        let desired_name = input_device_name
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "default");

        let mut selected: Option<cpal::Device> = None;
        if let Some(name) = desired_name {
            if let Ok(devices) = host.input_devices() {
                for d in devices {
                    let Ok(n) = d.name() else { continue };
                    if n == name {
                        selected = Some(d);
                        break;
                    }
                }
            }
        }

        let device = match selected {
            Some(d) => {
                log::info!("Using selected input device: {}", desired_name.unwrap_or("<unknown>"));
                d
            }
            None => {
                if let Some(name) = desired_name {
                    log::warn!(
                        "Selected input device '{}' not found; falling back to default input device",
                        name
                    );
                }
                host.default_input_device()
                    .ok_or(AudioCaptureError::NoInputDevice)?
            }
        };

        let config = device
            .default_input_config()
            .map_err(|e| AudioCaptureError::DeviceConfig(e.to_string()))?;

        self.sample_rate = config.sample_rate().0;
        self.channels = config.channels();

        log::info!(
            "Audio config: {} Hz, {} channels, {:?}",
            self.sample_rate,
            self.channels,
            config.sample_format()
        );

        // Create new buffer with correct params
        self.buffer = Arc::new(StdMutex::new(AudioBuffer::new(
            self.sample_rate,
            self.channels,
            max_duration_secs,
        )));

        let buffer_clone = self.buffer.clone();
        let meter = self.level_meter.clone();
        let waveform_meter = self.waveform_meter.clone();
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let sample_format = config.sample_format();
        let stream_config: cpal::StreamConfig = config.into();
        let vad_config = self.vad_config.clone();
        let sample_rate = self.sample_rate;

        // Spawn capture thread
        let thread_handle = thread::spawn(move || {
            run_capture_thread(
                device,
                stream_config,
                sample_format,
                buffer_clone,
                meter,
                waveform_meter,
                command_rx,
                event_tx,
                vad_config,
                sample_rate,
            )
        });

        self.capture_handle = Some(CaptureHandle {
            command_tx,
            event_rx,
            thread_handle,
        });

        log::info!("Audio capture started");
        Ok(())
    }

    /// Stop recording and return the captured audio as WAV bytes
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn stop_and_get_wav(&mut self) -> Result<Vec<u8>, AudioCaptureError> {
        self.stop_and_get_wav_with_noise_gate(0)
    }

    /// Stop recording and return the captured audio as WAV bytes, applying an optional noise gate.
    ///
    /// `noise_gate_strength` is 0..=100 where 0 disables the noise gate.
    pub fn stop_and_get_wav_with_noise_gate(
        &mut self,
        noise_gate_strength: u8,
    ) -> Result<Vec<u8>, AudioCaptureError> {
        let (wav_bytes, _stats) = self.stop_and_get_wav_with_stats_with_noise_gate(noise_gate_strength)?;
        Ok(wav_bytes)
    }

    /// Stop recording and return the captured audio as WAV bytes along with level stats.
    pub fn stop_and_get_wav_with_stats(
        &mut self,
    ) -> Result<(Vec<u8>, AudioLevelStats), AudioCaptureError> {
        self.stop_and_get_wav_with_stats_with_noise_gate(0)
    }

    /// Stop recording and return WAV bytes + level stats, optionally applying an experimental noise gate.
    ///
    /// Note: stats are computed on the *raw* (pre-gate) samples.
    pub fn stop_and_get_wav_with_stats_with_noise_gate(
        &mut self,
        noise_gate_strength: u8,
    ) -> Result<(Vec<u8>, AudioLevelStats), AudioCaptureError> {
        self.stop();

        let buffer = self
            .buffer
            .lock()
            .map_err(|_| AudioCaptureError::Encoding("Failed to lock buffer".to_string()))?;

        let stats = buffer.level_stats();
        let wav_bytes = buffer.to_wav_bytes_with_noise_gate(noise_gate_strength)?;

        log::info!(
            "Audio capture stopped, {} bytes captured (duration {:.2}s, rms {:.6}, peak {:.6})",
            wav_bytes.len(),
            stats.duration_secs,
            stats.rms,
            stats.peak
        );

        Ok((wav_bytes, stats))
    }

    /// Stop recording without returning audio data
    pub fn stop(&mut self) {
        if let Some(handle) = self.capture_handle.take() {
            log::info!("Stopping audio capture");
            // Send stop command (ignore error if thread already stopped)
            let _ = handle.command_tx.send(CaptureCommand::Stop);
            // Wait for thread to finish (with timeout in case of issues)
            let _ = handle.thread_handle.join();
        }
    }

    /// Check if currently recording
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_recording(&self) -> bool {
        self.capture_handle.is_some()
    }

    /// Poll for VAD events (non-blocking)
    ///
    /// Returns the next VAD event if one is available, or None if no events are pending.
    /// This should be called periodically to check for speech start/end events.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn poll_vad_event(&self) -> Option<AudioCaptureEvent> {
        if let Some(ref handle) = self.capture_handle {
            handle.event_rx.try_recv().ok()
        } else {
            None
        }
    }

    /// Check if VAD auto-stop is enabled
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_vad_auto_stop_enabled(&self) -> bool {
        self.vad_config.enabled && self.vad_config.auto_stop
    }

    /// Get the duration of recorded audio in seconds
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn duration_secs(&self) -> f32 {
        self.buffer
            .lock()
            .map(|b| b.duration_secs())
            .unwrap_or(0.0)
    }

    /// Get the sample rate
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Get the number of channels
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn channels(&self) -> u16 {
        self.channels
    }
}

impl Default for AudioCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Run the audio capture in a dedicated thread
fn run_capture_thread(
    device: cpal::Device,
    config: cpal::StreamConfig,
    sample_format: SampleFormat,
    buffer: Arc<StdMutex<AudioBuffer>>,
    meter: Arc<AudioLevelMeter>,
    waveform_meter: Arc<AudioWaveformMeter>,
    command_rx: mpsc::Receiver<CaptureCommand>,
    event_tx: mpsc::Sender<AudioCaptureEvent>,
    vad_config: VadAutoStopConfig,
    sample_rate: u32,
) -> Result<(), AudioCaptureError> {
    use cpal::Sample;

    let err_fn = |err| {
        log::error!("Audio stream error: {}", err);
    };

    // Create a channel for passing samples to the VAD processing thread
    let (vad_samples_tx, vad_samples_rx): (mpsc::Sender<Vec<f32>>, mpsc::Receiver<Vec<f32>>) =
        mpsc::channel();

    // Spawn a separate thread for VAD processing (since webrtc-vad is not Send)
    let vad_handle = if vad_config.enabled {
        let event_tx_clone = event_tx.clone();
        let vad_cfg = vad_config.vad_config.clone();
        Some(thread::spawn(move || {
            let mut processor = VadFrameProcessor::new(vad_cfg, sample_rate);
            log::info!("VAD processor initialized for {} Hz audio in dedicated thread", sample_rate);

            loop {
                match vad_samples_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(samples) => {
                        for event in processor.process(&samples) {
                            let capture_event = match event {
                                VadEvent::SpeechStart { .. } => AudioCaptureEvent::SpeechStart,
                                VadEvent::SpeechEnd => AudioCaptureEvent::SpeechEnd,
                                VadEvent::None => continue,
                            };
                            let _ = event_tx_clone.send(capture_event);
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        }))
    } else {
        None
    };

    let stream = match sample_format {
        SampleFormat::F32 => {
            let buffer = buffer.clone();
            let meter = meter.clone();
            let waveform_meter = waveform_meter.clone();
            let vad_tx = if vad_config.enabled { Some(vad_samples_tx.clone()) } else { None };
            let channels = config.channels as usize;
            device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    // Realtime meter (cheap math, no allocations).
                    let mut peak: f32 = 0.0;
                    let mut sum_sq: f64 = 0.0;
                    let mut n: u64 = 0;
                    for &s in data {
                        let a = s.abs();
                        if a > peak {
                            peak = a;
                        }
                        sum_sq += (s as f64) * (s as f64);
                        n += 1;
                    }
                    let rms = if n == 0 { 0.0 } else { (sum_sq / n as f64).sqrt() as f32 };
                    meter.update(rms, peak);

                    // True waveform buckets for UI.
                    waveform_meter.update_from_f32_interleaved(data, channels);

                    // Store audio in buffer
                    if let Ok(mut buf) = buffer.lock() {
                        buf.append(data);
                    }

                    // Send samples to VAD thread if enabled
                    if let Some(ref tx) = vad_tx {
                        let _ = tx.send(data.to_vec());
                    }
                },
                err_fn,
                None,
            )
        }
        SampleFormat::I16 => {
            let buffer = buffer.clone();
            let meter = meter.clone();
            let waveform_meter = waveform_meter.clone();
            let vad_tx = if vad_config.enabled { Some(vad_samples_tx.clone()) } else { None };
            let channels = config.channels as usize;
            device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let mut peak: f32 = 0.0;
                    let mut sum_sq: f64 = 0.0;
                    let samples: Vec<f32> = data
                        .iter()
                        .map(|&s| {
                            let f = s.to_float_sample();
                            let a = f.abs();
                            if a > peak {
                                peak = a;
                            }
                            sum_sq += (f as f64) * (f as f64);
                            f
                        })
                        .collect();
                    let n = samples.len() as u64;
                    let rms = if n == 0 { 0.0 } else { (sum_sq / n as f64).sqrt() as f32 };
                    meter.update(rms, peak);

                    // True waveform buckets for UI.
                    waveform_meter.update_from_f32_interleaved(&samples, channels);

                    // Store audio in buffer
                    if let Ok(mut buf) = buffer.lock() {
                        buf.append(&samples);
                    }

                    // Send samples to VAD thread if enabled
                    if let Some(ref tx) = vad_tx {
                        let _ = tx.send(samples);
                    }
                },
                err_fn,
                None,
            )
        }
        SampleFormat::U16 => {
            let buffer = buffer.clone();
            let meter = meter.clone();
            let waveform_meter = waveform_meter.clone();
            let vad_tx = if vad_config.enabled { Some(vad_samples_tx.clone()) } else { None };
            let channels = config.channels as usize;
            device.build_input_stream(
                &config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    let mut peak: f32 = 0.0;
                    let mut sum_sq: f64 = 0.0;
                    let samples: Vec<f32> = data
                        .iter()
                        .map(|&s| {
                            let f = s.to_float_sample();
                            let a = f.abs();
                            if a > peak {
                                peak = a;
                            }
                            sum_sq += (f as f64) * (f as f64);
                            f
                        })
                        .collect();
                    let n = samples.len() as u64;
                    let rms = if n == 0 { 0.0 } else { (sum_sq / n as f64).sqrt() as f32 };
                    meter.update(rms, peak);

                    // True waveform buckets for UI.
                    waveform_meter.update_from_f32_interleaved(&samples, channels);

                    // Store audio in buffer
                    if let Ok(mut buf) = buffer.lock() {
                        buf.append(&samples);
                    }

                    // Send samples to VAD thread if enabled
                    if let Some(ref tx) = vad_tx {
                        let _ = tx.send(samples);
                    }
                },
                err_fn,
                None,
            )
        }
        _ => {
            return Err(AudioCaptureError::DeviceConfig(format!(
                "Unsupported sample format: {:?}",
                sample_format
            )));
        }
    }
    .map_err(|e| AudioCaptureError::StreamBuild(e.to_string()))?;

    stream
        .play()
        .map_err(|e| AudioCaptureError::StreamStart(e.to_string()))?;

    // Wait for stop command
    loop {
        match command_rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(CaptureCommand::Stop) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // Drop the VAD sender to signal the VAD thread to stop
    drop(vad_samples_tx);

    // Wait for VAD thread to finish
    if let Some(handle) = vad_handle {
        let _ = handle.join();
    }

    // Stream is dropped here, stopping capture
    Ok(())
}

/// Get the list of available input devices
#[cfg_attr(not(test), allow(dead_code))]
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devices| {
            devices
                .filter_map(|d| d.name().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Get information about the default input device
#[cfg_attr(not(test), allow(dead_code))]
pub fn get_default_input_device_info() -> Option<(String, u32, u16)> {
    let host = cpal::default_host();
    let device = host.default_input_device()?;
    let name = device.name().ok()?;
    let config = device.default_input_config().ok()?;
    Some((name, config.sample_rate().0, config.channels()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_buffer_creation() {
        let buffer = AudioBuffer::new(16000, 1, 60.0);
        assert!(buffer.is_empty());
        assert_eq!(buffer.sample_rate(), 16000);
        assert_eq!(buffer.channels(), 1);
    }

    #[test]
    fn test_audio_buffer_append() {
        let mut buffer = AudioBuffer::new(16000, 1, 60.0);
        buffer.append(&[0.5, -0.5, 0.0]);
        assert_eq!(buffer.len(), 3);
    }

    #[test]
    fn test_audio_buffer_clear() {
        let mut buffer = AudioBuffer::new(16000, 1, 60.0);
        buffer.append(&[0.5, -0.5, 0.0]);
        buffer.clear();
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_audio_buffer_to_wav() {
        let mut buffer = AudioBuffer::new(16000, 1, 60.0);
        // Add some test samples
        buffer.append(&[0.0; 1600]); // 0.1 seconds of silence
        let wav_bytes = buffer.to_wav_bytes().expect("Failed to encode WAV");

        // WAV header is 44 bytes, plus samples
        assert!(wav_bytes.len() > 44);
        // Check WAV magic bytes "RIFF"
        assert_eq!(&wav_bytes[0..4], b"RIFF");
    }

    #[test]
    fn test_audio_buffer_max_duration() {
        let mut buffer = AudioBuffer::new(1000, 1, 1.0); // 1 second max
        // Add 2 seconds worth of samples
        buffer.append(&[0.0; 2000]);
        // Should be trimmed to 1 second
        assert_eq!(buffer.len(), 1000);
    }
}
