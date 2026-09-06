//! Audio capture module using cpal for cross-platform audio input.
//!
//! This module provides functionality to capture audio from the system's
//! default input device and encode it to WAV format for STT processing.
//!
//! Supports optional Voice Activity Detection (VAD) for auto-stop functionality.

use crate::vad::{VadConfig, VadEvent, VadFrameProcessor};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use crossbeam_queue::ArrayQueue;
use hound::{WavSpec, WavWriter};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex as StdMutex};
use std::thread::{self, JoinHandle};

pub mod computer_audio;
mod device_selection;
pub mod journal;
mod meters;
mod preprocessing;

pub use device_selection::{
    get_default_input_device_info, list_input_devices, list_input_devices_v2, AudioInputDeviceInfo,
};
pub use meters::{
    AudioLevelSnapshot, AudioLevelStats, AudioWaveformSnapshot, SharedAudioLevelMeter,
    SharedAudioWaveformMeter,
};

/// Number of min/max buckets sent to the overlay for waveform rendering.
///
/// Re-exported from the meter Module to preserve the existing `audio_capture` API.
#[allow(dead_code)]
pub const WAVEFORM_BINS: usize = meters::WAVEFORM_BINS;

use device_selection::select_input_device_from_host;
use meters::{compute_rms_peak, detect_speech_presence, AudioLevelMeter, AudioWaveformMeter};
use preprocessing::{
    apply_agc, apply_highpass_dc_block, apply_light_noise_suppression,
    apply_noise_gate_interleaved, noise_gate_threshold_dbfs_from_strength,
};

use crate::audio_normalization::{
    downmix_interleaved_chunk_to_mono_into, downmix_interleaved_to_mono,
};

fn estimate_callback_interleaved_capacity(config: &cpal::StreamConfig, channels: usize) -> usize {
    // CPAL often uses a stable callback size, but it can vary.
    // We preallocate a reasonable default to avoid per-callback growth.
    let frames = match config.buffer_size {
        cpal::BufferSize::Fixed(n) => n as usize,
        cpal::BufferSize::Default => 2048,
    };
    frames.saturating_mul(channels.max(1))
}

// Keep these runtime-policy helpers local to `audio_capture.rs` on purpose.
// Phase 6 is about characterizing the CPAL/VAD lifecycle before we invent a broader seam,
// so centralizing the policy math here reduces drift without pretending the runtime has been
// cleanly extracted yet.
const MAX_PRE_ROLL_MS: u32 = 5000;
const MAX_PRE_ROLL_SAMPLES: usize = 10_000_000;
const STOP_JOIN_TIMEOUT_MS: u64 = 2500;
const WATCHDOG_CHECK_EVERY_MS: u64 = 100;
const WATCHDOG_STALL_MS: u64 = 2000;

fn clamped_pre_roll_ms(pre_roll_ms: u32) -> u32 {
    pre_roll_ms.min(MAX_PRE_ROLL_MS)
}

fn pre_roll_capacity_samples(sample_rate: u32, channels: usize, pre_roll_ms: u32) -> usize {
    let sample_rate = sample_rate.max(1) as f32;
    let channels = channels.max(1) as f32;
    let pre_roll_secs = clamped_pre_roll_ms(pre_roll_ms) as f32 / 1000.0;
    ((sample_rate * pre_roll_secs * channels) as usize).min(MAX_PRE_ROLL_SAMPLES)
}

fn watchdog_restart_backoff_ms(consecutive_restart_failures: u32) -> u64 {
    match consecutive_restart_failures {
        0 | 1 => 200,
        2 => 500,
        3 => 1000,
        _ => 2000,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AudioEncodeConfig {
    /// If set, apply a noise gate with the given threshold.
    pub noise_gate_threshold_dbfs: Option<f32>,
    /// Convert the captured audio to mono before WAV encoding.
    pub downmix_to_mono: bool,
    /// Resample to 16kHz before WAV encoding.
    pub resample_to_16khz: bool,
    /// Apply a lightweight high-pass (DC/rumble) filter.
    pub highpass_enabled: bool,
    /// Apply a lightweight gain normalization.
    pub agc_enabled: bool,
    /// Apply a lightweight noise suppression.
    pub noise_suppression_enabled: bool,
    /// If enabled, compute a best-effort speech presence boolean using WebRTC VAD.
    pub detect_speech_presence: bool,
}

impl Default for AudioEncodeConfig {
    fn default() -> Self {
        Self {
            noise_gate_threshold_dbfs: None,
            downmix_to_mono: true,
            resample_to_16khz: false,
            highpass_enabled: true,
            agc_enabled: false,
            noise_suppression_enabled: false,
            detect_speech_presence: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
pub struct AudioCaptureDiagnostics {
    pub stats: AudioLevelStats,
    pub speech_detected: Option<bool>,
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
    #[allow(dead_code)]
    NotActive,

    #[error("Capture thread error: {0}")]
    #[allow(dead_code)]
    ThreadError(String),
}

/// Audio buffer that accumulates samples during recording
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    recovery_path: Option<std::path::PathBuf>,
    journal: Option<Arc<StdMutex<journal::Journal>>>,
    journal_failed: bool,
    captured_samples: u64,
    data: Vec<f32>,
    write_pos: usize,
    filled: usize,
    sample_rate: u32,
    channels: u16,
    max_duration_secs: f32,
}

impl AudioBuffer {
    fn max_samples_for(sample_rate: u32, channels: u16, max_duration_secs: f32) -> usize {
        let sr = sample_rate.max(1) as f32;
        let ch = channels.max(1) as f32;
        let secs = max_duration_secs.max(0.0);
        (sr * secs * ch) as usize
    }

    fn capacity(&self) -> usize {
        self.data.len()
    }

    fn snapshot(&self) -> Vec<f32> {
        let cap = self.capacity();
        if self.filled == 0 || cap == 0 {
            return Vec::new();
        }

        if self.filled < cap {
            // Not wrapped yet; oldest is at 0.
            return self.data[..self.filled].to_vec();
        }

        // Wrapped / full: oldest is at write_pos.
        let mut out = Vec::with_capacity(cap);
        out.extend_from_slice(&self.data[self.write_pos..]);
        if self.write_pos > 0 {
            out.extend_from_slice(&self.data[..self.write_pos]);
        }
        out
    }

    /// Create a new audio buffer with the specified parameters
    pub fn new(sample_rate: u32, channels: u16, max_duration_secs: f32) -> Self {
        let capacity = Self::max_samples_for(sample_rate, channels, max_duration_secs);
        Self {
            data: vec![0.0; capacity],
            recovery_path: None,
            journal: None,
            journal_failed: false,
            captured_samples: 0,
            write_pos: 0,
            filled: 0,
            sample_rate,
            channels: channels.max(1),
            max_duration_secs,
        }
    }

    /// Update the buffer's format (sample rate + channels) without clearing samples.
    ///
    /// This is primarily used by long-running streams (Hot Mic) when the underlying
    /// device is restarted.
    pub fn set_format(&mut self, sample_rate: u32, channels: u16) {
        self.sample_rate = sample_rate;
        self.channels = channels.max(1);

        // Keep the most recent samples, but resize capacity to reflect the new format.
        // This can happen on device restarts; it's not on a realtime hot path.
        let new_cap =
            Self::max_samples_for(self.sample_rate, self.channels, self.max_duration_secs);
        if new_cap == self.capacity() {
            return;
        }

        if new_cap == 0 {
            self.data.clear();
            self.write_pos = 0;
            self.filled = 0;
            return;
        }

        let snapshot = self.snapshot();
        let keep = if snapshot.len() > new_cap {
            snapshot[snapshot.len() - new_cap..].to_vec()
        } else {
            snapshot
        };

        let mut data = vec![0.0; new_cap];
        let n = keep.len().min(new_cap);
        data[..n].copy_from_slice(&keep[..n]);

        self.data = data;
        self.filled = n;
        self.write_pos = if self.filled < self.capacity() {
            self.filled
        } else {
            0
        };
    }

    /// Reset the buffer for a new recording session.
    ///
    /// Clears samples and sets the max duration (used for trimming during capture).
    pub fn reset_for_recording(&mut self, max_duration_secs: f32) {
        self.captured_samples = 0;
        self.max_duration_secs = max_duration_secs.max(0.0);

        let cap = Self::max_samples_for(self.sample_rate, self.channels, self.max_duration_secs);
        if cap == 0 {
            self.data.clear();
        } else {
            self.data = vec![0.0; cap];
        }
        self.write_pos = 0;
        self.filled = 0;
    }

    /// Append samples to the buffer
    pub fn append(&mut self, new_samples: &[f32]) {
        if self.journal_failed {
            return;
        }
        if self.journal.is_none() {
            if let Some(path) = self.recovery_path.as_ref() {
                match journal::Journal::create(path, self.sample_rate, self.channels) {
                    Ok(writer) => self.journal = Some(Arc::new(StdMutex::new(writer))),
                    Err(_) => {
                        self.journal_failed = true;
                        return;
                    }
                }
            }
        }
        if let Some(journal) = &self.journal {
            match journal.lock() {
                Ok(mut writer) => {
                    if writer
                        .append(new_samples, self.sample_rate, self.channels)
                        .is_err()
                    {
                        self.journal_failed = true;
                        return;
                    }
                }
                Err(_) => {
                    self.journal_failed = true;
                    return;
                }
            }
        }
        self.captured_samples = self
            .captured_samples
            .saturating_add(new_samples.len() as u64);
        let cap = self.capacity();
        if cap == 0 || new_samples.is_empty() {
            return;
        }

        // Fast path: if input is larger than the whole buffer, keep only the tail.
        if new_samples.len() >= cap {
            let tail = &new_samples[new_samples.len() - cap..];
            self.data.copy_from_slice(tail);
            self.write_pos = 0;
            self.filled = cap;
            return;
        }

        // Write in up to two contiguous segments.
        let first = (cap - self.write_pos).min(new_samples.len());
        self.data[self.write_pos..self.write_pos + first].copy_from_slice(&new_samples[..first]);
        self.write_pos = (self.write_pos + first) % cap;

        let rest = &new_samples[first..];
        if !rest.is_empty() {
            self.data[..rest.len()].copy_from_slice(rest);
            self.write_pos = rest.len();
        }

        self.filled = (self.filled + new_samples.len()).min(cap);
    }

    /// Clear all samples from the buffer
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn clear(&mut self) {
        self.write_pos = 0;
        self.filled = 0;
    }

    /// Get the number of samples in the buffer
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.filled
    }

    /// Check if the buffer is empty
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.filled == 0
    }

    /// Get the duration of audio in the buffer in seconds
    pub fn duration_secs(&self) -> f32 {
        self.len() as f32 / (self.sample_rate as f32 * self.channels as f32)
    }

    /// Compute simple signal level statistics over the captured samples.
    ///
    /// Samples are expected to be normalized floats in [-1.0, 1.0].
    pub fn level_stats(&self) -> AudioLevelStats {
        let mut peak: f32 = 0.0;
        let mut sum_sq: f64 = 0.0;
        let mut n: u64 = 0;

        let cap = self.capacity();
        if self.filled > 0 && cap > 0 {
            if self.filled < cap {
                for &s in &self.data[..self.filled] {
                    let a = s.abs();
                    if a > peak {
                        peak = a;
                    }
                    sum_sq += (s as f64) * (s as f64);
                    n += 1;
                }
            } else {
                for &s in &self.data[self.write_pos..] {
                    let a = s.abs();
                    if a > peak {
                        peak = a;
                    }
                    sum_sq += (s as f64) * (s as f64);
                    n += 1;
                }
                if self.write_pos > 0 {
                    for &s in &self.data[..self.write_pos] {
                        let a = s.abs();
                        if a > peak {
                            peak = a;
                        }
                        sum_sq += (s as f64) * (s as f64);
                        n += 1;
                    }
                }
            }
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
    #[cfg_attr(not(test), allow(dead_code))]
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
        let (wav_bytes, _diagnostics) = self.to_wav_bytes_with_config(AudioEncodeConfig {
            noise_gate_threshold_dbfs: noise_gate_threshold_dbfs_from_strength(noise_gate_strength),
            ..Default::default()
        })?;
        Ok(wav_bytes)
    }

    pub fn to_wav_bytes_with_config(
        &self,
        cfg: AudioEncodeConfig,
    ) -> Result<(Vec<u8>, AudioCaptureDiagnostics), AudioCaptureError> {
        if self.journal_failed {
            return Err(AudioCaptureError::Encoding(
                "Recovery storage failed; recording may be incomplete".into(),
            ));
        }
        if let Some(writer) = &self.journal {
            writer
                .lock()
                .map_err(|_| AudioCaptureError::Encoding("Recovery storage unavailable".into()))?
                .finish()
                .map_err(|_| {
                    AudioCaptureError::Encoding("Recovery audio could not be synchronized".into())
                })?;
        }
        // Allocate once at stop-time to obtain a contiguous, chronological snapshot.
        let raw_samples = self.snapshot();

        let diagnostics = if cfg.detect_speech_presence {
            Some(detect_speech_presence(
                &raw_samples,
                self.sample_rate,
                self.channels,
            ))
        } else {
            None
        };

        let mut processed_samples = if cfg.downmix_to_mono {
            downmix_interleaved_to_mono(&raw_samples, self.channels as usize)
        } else {
            raw_samples
        };

        let mut out_sample_rate = self.sample_rate;
        let out_channels: u16 = if cfg.downmix_to_mono {
            1
        } else {
            self.channels.max(1)
        };

        // If we didn't downmix, most processing is skipped (keeps code simple and predictable).
        if cfg.downmix_to_mono {
            if cfg.noise_suppression_enabled {
                apply_light_noise_suppression(&mut processed_samples, out_sample_rate);
            }
            if cfg.highpass_enabled {
                apply_highpass_dc_block(&mut processed_samples, out_sample_rate);
            }
            if cfg.agc_enabled {
                apply_agc(&mut processed_samples);
            }

            // Optional resample after filtering/gain.
            if cfg.resample_to_16khz && out_sample_rate != 16000 {
                processed_samples = crate::audio_normalization::resample_to_16khz_vad_quality(
                    &processed_samples,
                    out_sample_rate,
                );
                out_sample_rate = 16000;
            }

            // Noise gate (mono)
            processed_samples = apply_noise_gate_interleaved(
                &processed_samples,
                out_sample_rate,
                1,
                cfg.noise_gate_threshold_dbfs,
            );
        } else {
            // Noise gate (interleaved)
            processed_samples = apply_noise_gate_interleaved(
                &processed_samples,
                out_sample_rate,
                out_channels,
                cfg.noise_gate_threshold_dbfs,
            );
        }

        let spec = WavSpec {
            channels: out_channels,
            sample_rate: out_sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = WavWriter::new(&mut cursor, spec)
                .map_err(|e| AudioCaptureError::Encoding(e.to_string()))?;

            for &sample in &processed_samples {
                let sample_i16 = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                writer
                    .write_sample(sample_i16)
                    .map_err(|e| AudioCaptureError::Encoding(e.to_string()))?;
            }

            writer
                .finalize()
                .map_err(|e| AudioCaptureError::Encoding(e.to_string()))?;
        }

        Ok((
            cursor.into_inner(),
            AudioCaptureDiagnostics {
                stats: self.level_stats(),
                speech_detected: diagnostics,
            },
        ))
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

/// A fixed-capacity rolling buffer for pre-roll audio.
///
/// Stores the last N *samples* (interleaved f32 PCM). This is designed to be
/// cheap to push to from the CPAL input callback.
#[derive(Debug, Clone)]
struct RollingBuffer {
    data: Vec<f32>,
    write_pos: usize,
    filled: usize,
}

impl RollingBuffer {
    fn new(capacity_samples: usize) -> Self {
        let cap = capacity_samples;
        Self {
            data: vec![0.0; cap],
            write_pos: 0,
            filled: 0,
        }
    }

    fn capacity(&self) -> usize {
        self.data.len()
    }

    fn clear(&mut self) {
        self.write_pos = 0;
        self.filled = 0;
    }

    fn set_capacity(&mut self, capacity_samples: usize) {
        if capacity_samples == self.capacity() {
            return;
        }

        if capacity_samples == 0 {
            self.data.clear();
            self.write_pos = 0;
            self.filled = 0;
            return;
        }

        let snapshot = self.snapshot();
        let keep = if snapshot.len() > capacity_samples {
            snapshot[snapshot.len() - capacity_samples..].to_vec()
        } else {
            snapshot
        };

        let mut data = vec![0.0; capacity_samples];
        let n = keep.len().min(capacity_samples);
        data[..n].copy_from_slice(&keep[..n]);

        self.data = data;
        self.filled = n;
        self.write_pos = if self.filled < self.capacity() {
            self.filled
        } else {
            0
        };
    }

    fn push(&mut self, samples: &[f32]) {
        let cap = self.capacity();
        if cap == 0 || samples.is_empty() {
            return;
        }

        // Fast path: if input is larger than the whole buffer, keep only the tail.
        if samples.len() >= cap {
            let tail = &samples[samples.len() - cap..];
            self.data.copy_from_slice(tail);
            self.write_pos = 0;
            self.filled = cap;
            return;
        }

        for &s in samples {
            self.data[self.write_pos] = s;
            self.write_pos += 1;
            if self.write_pos >= cap {
                self.write_pos = 0;
            }
            if self.filled < cap {
                self.filled += 1;
            }
        }
    }

    fn snapshot(&self) -> Vec<f32> {
        let cap = self.capacity();
        if self.filled == 0 || cap == 0 {
            return Vec::new();
        }

        if self.filled < cap {
            // Not wrapped yet; oldest is at 0.
            return self.data[..self.filled].to_vec();
        }

        // Wrapped / full: oldest is at write_pos.
        let mut out = Vec::with_capacity(cap);
        out.extend_from_slice(&self.data[self.write_pos..]);
        if self.write_pos > 0 {
            out.extend_from_slice(&self.data[..self.write_pos]);
        }
        out
    }
}

/// Minimal interface used by the pipeline so we can unit test state transitions
/// without requiring a real CPAL device.
///
/// Real implementation: `AudioCapture`.
pub trait AudioCaptureBackend: Send {
    fn recording_progress(&self) -> (f64, bool) {
        (0.0, false)
    }
    fn set_computer_audio(&mut self, enabled: bool) -> Result<(), AudioCaptureError> {
        if enabled {
            Err(AudioCaptureError::NoInputDevice)
        } else {
            Ok(())
        }
    }
    fn recovery_path(&self) -> Option<std::path::PathBuf> {
        None
    }
    fn discard_recovery(&mut self) -> Result<(), AudioCaptureError> {
        Ok(())
    }
    fn set_recovery_path(
        &mut self,
        _path: Option<std::path::PathBuf>,
    ) -> Result<(), AudioCaptureError> {
        Ok(())
    }
    fn set_paused(&mut self, _paused: bool) -> Result<(), AudioCaptureError> {
        Err(AudioCaptureError::NotActive)
    }
    fn shared_level_meter(&self) -> SharedAudioLevelMeter;
    fn shared_waveform_meter(&self) -> SharedAudioWaveformMeter;
    fn level_snapshot(&self) -> AudioLevelSnapshot;

    fn set_vad_config(&mut self, config: VadAutoStopConfig);
    fn set_capture_behavior(
        &mut self,
        hot_mic_enabled: bool,
        hot_mic_pre_roll_ms: u32,
        mic_auto_recover_enabled: bool,
        input_device_name: Option<&str>,
    ) -> Result<(), AudioCaptureError>;

    fn start_recording_session(
        &mut self,
        max_duration_secs: f32,
        input_device_name: Option<&str>,
    ) -> Result<(), AudioCaptureError>;

    fn stop_and_get_wav_with_diagnostics(
        &mut self,
        cfg: AudioEncodeConfig,
    ) -> Result<(Vec<u8>, AudioCaptureDiagnostics), AudioCaptureError>;

    fn stop_and_get_wav_before_after(
        &mut self,
        after_cfg: AudioEncodeConfig,
    ) -> Result<(Vec<u8>, Vec<u8>, AudioCaptureDiagnostics), AudioCaptureError>;

    fn stop_recording(&mut self);
    fn stop(&mut self);

    fn poll_vad_event(&self) -> Option<AudioCaptureEvent>;
    fn is_vad_auto_stop_enabled(&self) -> bool;

    /// Set (or clear) the live audio sender for concurrent STT streaming.
    fn set_live_audio_tx(&mut self, tx: Option<tokio::sync::mpsc::Sender<Vec<f32>>>);

    /// Current sample rate of the active capture session (or the default).
    fn capture_sample_rate(&self) -> u32;
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
#[derive(Debug, Clone, Default)]
pub struct VadAutoStopConfig {
    /// Enable VAD processing
    pub enabled: bool,
    /// Automatically stop recording when speech ends
    #[cfg_attr(not(test), allow(dead_code))]
    pub auto_stop: bool,
    /// VAD configuration
    pub vad_config: VadConfig,
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
    computer_audio_enabled: bool,
    computer_capture: Option<computer_audio::Capture>,
    buffer: Arc<StdMutex<AudioBuffer>>,
    pre_roll: Arc<StdMutex<RollingBuffer>>,
    capture_handle: Option<CaptureHandle>,
    sample_rate: u32,
    channels: u16,
    vad_config: VadAutoStopConfig,

    // Capture behavior settings (synced from PipelineConfig)
    hot_mic_enabled: bool,
    pre_roll_ms: Arc<AtomicU32>,
    mic_auto_recover_enabled: Arc<AtomicBool>,
    desired_device_name: Arc<StdMutex<Option<String>>>,

    // Recording session state shared with the capture thread.
    recording_active: Arc<AtomicBool>,

    // Most recent realtime level stats (for UI metering / overlay waveform).
    level_meter: Arc<AudioLevelMeter>,

    // Most recent realtime waveform buckets (for true waveform rendering).
    waveform_meter: Arc<AudioWaveformMeter>,

    /// Optional sender for live mono f32 audio chunks (used by concurrent STT streaming).
    ///
    /// When set, the worker thread downmixes captured audio to mono and sends chunks
    /// through this sender. The pipeline sets this at recording start when the STT
    /// provider supports concurrent streaming, and clears it when recording stops.
    live_audio_tx: Arc<StdMutex<Option<tokio::sync::mpsc::Sender<Vec<f32>>>>>,
}

impl AudioCapture {
    /// Create a new audio capture instance
    pub fn new() -> Self {
        Self {
            buffer: Arc::new(StdMutex::new(AudioBuffer::new(44100, 1, 300.0))),
            pre_roll: Arc::new(StdMutex::new(RollingBuffer::new(0))),
            capture_handle: None,
            computer_audio_enabled: false,
            computer_capture: None,
            sample_rate: 44100,
            channels: 1,
            vad_config: VadAutoStopConfig::default(),

            hot_mic_enabled: false,
            pre_roll_ms: Arc::new(AtomicU32::new(1500)),
            mic_auto_recover_enabled: Arc::new(AtomicBool::new(false)),
            desired_device_name: Arc::new(StdMutex::new(None)),
            recording_active: Arc::new(AtomicBool::new(false)),
            level_meter: Arc::new(AudioLevelMeter::default()),
            waveform_meter: Arc::new(AudioWaveformMeter::default()),
            live_audio_tx: Arc::new(StdMutex::new(None)),
        }
    }

    /// Create a new audio capture instance with VAD configuration
    pub fn with_vad_config(vad_config: VadAutoStopConfig) -> Self {
        Self {
            buffer: Arc::new(StdMutex::new(AudioBuffer::new(44100, 1, 300.0))),
            pre_roll: Arc::new(StdMutex::new(RollingBuffer::new(0))),
            capture_handle: None,
            computer_audio_enabled: false,
            computer_capture: None,
            sample_rate: 44100,
            channels: 1,
            vad_config,

            hot_mic_enabled: false,
            pre_roll_ms: Arc::new(AtomicU32::new(1500)),
            mic_auto_recover_enabled: Arc::new(AtomicBool::new(false)),
            desired_device_name: Arc::new(StdMutex::new(None)),
            recording_active: Arc::new(AtomicBool::new(false)),
            level_meter: Arc::new(AudioLevelMeter::default()),
            waveform_meter: Arc::new(AudioWaveformMeter::default()),
            live_audio_tx: Arc::new(StdMutex::new(None)),
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
        SharedAudioLevelMeter::from_meter(self.level_meter.clone())
    }

    pub fn shared_waveform_meter(&self) -> SharedAudioWaveformMeter {
        SharedAudioWaveformMeter::from_meter(self.waveform_meter.clone())
    }

    /// Update VAD configuration
    pub fn set_vad_config(&mut self, config: VadAutoStopConfig) {
        self.vad_config = config;
    }

    /// Update capture behavior settings (Hot Mic + auto-recovery) and apply them.
    ///
    /// - When `hot_mic_enabled` is true, the input stream is kept open while idle.
    /// - When false, the stream is only opened on an explicit recording start.
    pub fn set_capture_behavior(
        &mut self,
        hot_mic_enabled: bool,
        hot_mic_pre_roll_ms: u32,
        mic_auto_recover_enabled: bool,
        input_device_name: Option<&str>,
    ) -> Result<(), AudioCaptureError> {
        self.hot_mic_enabled = hot_mic_enabled;
        self.pre_roll_ms
            .store(clamped_pre_roll_ms(hot_mic_pre_roll_ms), Ordering::Relaxed);
        self.mic_auto_recover_enabled
            .store(mic_auto_recover_enabled, Ordering::Relaxed);

        // Update desired device name for potential restarts.
        let desired_name = input_device_name
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "default")
            .map(|s| s.to_string());
        if let Ok(mut lock) = self.desired_device_name.lock() {
            *lock = desired_name;
        }

        if self.computer_capture.is_some() {
            return Ok(());
        }
        if hot_mic_enabled {
            // Keep the stream open while idle.
            self.ensure_stream_running(input_device_name)?;
        } else {
            // If we are not recording, close the stream to match push-to-talk behavior.
            if !self.recording_active.load(Ordering::Relaxed) {
                self.stop();
            }
        }

        Ok(())
    }

    /// Start a recording session.
    ///
    /// - In Hot Mic mode, this reuses the always-on stream and prepends the rolling pre-roll.
    /// - In push-to-talk mode, this starts a new stream and records normally.
    pub fn start_recording_session(
        &mut self,
        max_duration_secs: f32,
        input_device_name: Option<&str>,
    ) -> Result<(), AudioCaptureError> {
        if self.computer_audio_enabled {
            self.stop();
            self.sample_rate = 16000;
            self.channels = 1;
            if let Ok(mut buffer) = self.buffer.lock() {
                buffer.set_format(16000, 1);
                buffer.reset_for_recording(max_duration_secs);
            }
            self.recording_active.store(true, Ordering::Relaxed);
            match computer_audio::Capture::start(self.buffer.clone(), self.recording_active.clone())
            {
                Ok(capture) => self.computer_capture = Some(capture),
                Err(error) => {
                    self.recording_active.store(false, Ordering::Relaxed);
                    return Err(error);
                }
            }
            return Ok(());
        }
        if self.hot_mic_enabled {
            self.ensure_stream_running(input_device_name)?;
            self.begin_recording_with_pre_roll(max_duration_secs);
            return Ok(());
        }

        // Push-to-talk: start a new capture stream and record.
        self.start_with_device_name(max_duration_secs, input_device_name)
    }

    fn begin_recording_with_pre_roll(&mut self, max_duration_secs: f32) {
        // Snapshot pre-roll first (avoid holding lock while mutating recording buffer).
        let pre_roll = self
            .pre_roll
            .lock()
            .map(|b| b.snapshot())
            .unwrap_or_default();

        if let Ok(mut buf) = self.buffer.lock() {
            buf.reset_for_recording(max_duration_secs);
            if !pre_roll.is_empty() {
                buf.append(&pre_roll);
            }
        }

        self.recording_active.store(true, Ordering::Relaxed);
    }

    /// Stop recording. In Hot Mic mode, keeps the stream open.
    pub fn stop_recording(&mut self) {
        self.computer_capture = None;
        self.recording_active.store(false, Ordering::Relaxed);
        // NOTE: We intentionally do NOT clear live_audio_tx here.
        // The worker thread must be allowed to flush any remaining queued chunks
        // (which have was_recording=true) through the live audio channel before
        // it is closed. The pipeline takes ownership of the StreamingSttSession
        // and its audio_tx is dropped during finalize(), which naturally signals
        // end-of-audio. The pipeline clears live_audio_tx after the session is taken.
        if !self.hot_mic_enabled {
            self.stop();
        }
    }

    /// Set (or clear) the live audio sender for concurrent STT streaming.
    ///
    /// While set, the capture worker thread will send mono f32 chunks through
    /// this sender for each audio callback during recording. Call with `None`
    /// to stop streaming (also done automatically on `stop_recording`).
    pub fn set_live_audio_tx(&mut self, tx: Option<tokio::sync::mpsc::Sender<Vec<f32>>>) {
        if let Ok(mut slot) = self.live_audio_tx.lock() {
            *slot = tx;
        }
    }

    /// Current sample rate of the active capture session (or the default).
    pub fn capture_sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn ensure_stream_running(
        &mut self,
        input_device_name: Option<&str>,
    ) -> Result<(), AudioCaptureError> {
        if self.capture_handle.is_some() {
            // Stream already running; ensure pre-roll buffer capacity matches current config.
            let (sr, ch) = self
                .buffer
                .lock()
                .map(|b| (b.sample_rate(), b.channels()))
                .unwrap_or((self.sample_rate, self.channels));
            let cap_samples = pre_roll_capacity_samples(
                sr,
                ch as usize,
                self.pre_roll_ms.load(Ordering::Relaxed),
            );
            if let Ok(mut pr) = self.pre_roll.lock() {
                pr.set_capacity(cap_samples);
            }
            return Ok(());
        }

        // Start a stream in "armed" mode (recording_active=false).
        // Reuse the existing device selection logic in start_with_device_name but
        // do not mark recording as active.
        self.start_stream_only(input_device_name)
    }

    fn start_stream_only(
        &mut self,
        input_device_name: Option<&str>,
    ) -> Result<(), AudioCaptureError> {
        // Stop any existing stream (defensive)
        self.stop();

        let host = cpal::default_host();

        let device = select_input_device_from_host(&host, input_device_name)
            .or_else(|| host.default_input_device())
            .ok_or(AudioCaptureError::NoInputDevice)?;

        let config = device
            .default_input_config()
            .map_err(|e| AudioCaptureError::DeviceConfig(e.to_string()))?;

        self.sample_rate = config.sample_rate();
        self.channels = config.channels().max(1);

        // Update pre-roll buffer capacity based on current stream format.
        let cap_samples = pre_roll_capacity_samples(
            self.sample_rate,
            self.channels as usize,
            self.pre_roll_ms.load(Ordering::Relaxed),
        );
        if let Ok(mut pr) = self.pre_roll.lock() {
            pr.set_capacity(cap_samples);
            pr.clear();
        }

        // Ensure the recording buffer format matches. Keep max_duration as-is for now.
        if let Ok(mut buf) = self.buffer.lock() {
            buf.set_format(self.sample_rate, self.channels);
        }

        // Prepare shared state for the capture thread.
        self.recording_active.store(false, Ordering::Relaxed);

        let buffer_clone = self.buffer.clone();
        let pre_roll_clone = self.pre_roll.clone();
        let recording_active = self.recording_active.clone();
        let meter = self.level_meter.clone();
        let waveform_meter = self.waveform_meter.clone();

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let sample_format = config.sample_format();
        let stream_config: cpal::StreamConfig = config.into();
        let vad_config = self.vad_config.clone();
        let pre_roll_ms = self.pre_roll_ms.clone();
        let auto_recover = self.mic_auto_recover_enabled.clone();
        let desired_device_name = self.desired_device_name.clone();
        let live_audio_tx = self.live_audio_tx.clone();

        // Spawn capture thread
        let thread_handle = thread::spawn(move || {
            run_capture_thread(CaptureThreadArgs {
                device,
                config: stream_config,
                sample_format,
                buffer: buffer_clone,
                pre_roll: pre_roll_clone,
                recording_active,
                pre_roll_ms,
                meter,
                waveform_meter,
                command_rx,
                event_tx,
                vad_config,
                auto_recover_enabled: auto_recover,
                desired_device_name,
                live_audio_tx,
            })
        });

        self.capture_handle = Some(CaptureHandle {
            command_tx,
            event_rx,
            thread_handle,
        });

        Ok(())
    }

    /// Get the current VAD configuration
    #[allow(dead_code)]
    pub fn vad_config(&self) -> &VadAutoStopConfig {
        &self.vad_config
    }

    /// Start recording audio from the default input device.
    ///
    /// Prefer `start_with_device_name` when you need to honor a user-selected mic.
    ///
    /// # Arguments
    /// * `max_duration_secs` - Maximum recording duration in seconds (for buffer sizing)
    #[allow(dead_code)]
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
        // Reuse the stream-only path and then enable recording.
        self.start_stream_only(input_device_name)?;

        if let Ok(mut buf) = self.buffer.lock() {
            buf.reset_for_recording(max_duration_secs);
        }

        self.recording_active.store(true, Ordering::Relaxed);
        log::info!("Audio capture started");
        Ok(())
    }

    /// Stop recording and return the captured audio as WAV bytes
    #[allow(dead_code)]
    pub fn stop_and_get_wav(&mut self) -> Result<Vec<u8>, AudioCaptureError> {
        self.stop_and_get_wav_with_noise_gate(0)
    }

    /// Stop recording and return the captured audio as WAV bytes, applying an optional noise gate.
    ///
    /// `noise_gate_strength` is 0..=100 where 0 disables the noise gate.
    #[allow(dead_code)]
    pub fn stop_and_get_wav_with_noise_gate(
        &mut self,
        noise_gate_strength: u8,
    ) -> Result<Vec<u8>, AudioCaptureError> {
        let (wav_bytes, _diag) =
            self.stop_and_get_wav_with_stats_with_noise_gate(noise_gate_strength)?;
        Ok(wav_bytes)
    }

    /// Stop recording and return the captured audio as WAV bytes along with level stats.
    #[allow(dead_code)]
    pub fn stop_and_get_wav_with_stats(
        &mut self,
    ) -> Result<(Vec<u8>, AudioLevelStats), AudioCaptureError> {
        self.stop_and_get_wav_with_stats_with_noise_gate(0)
    }

    /// Stop recording and return WAV bytes + level stats, optionally applying an experimental noise gate.
    ///
    /// Note: stats are computed on the *raw* (pre-gate) samples.
    #[allow(dead_code)]
    pub fn stop_and_get_wav_with_stats_with_noise_gate(
        &mut self,
        noise_gate_strength: u8,
    ) -> Result<(Vec<u8>, AudioLevelStats), AudioCaptureError> {
        self.stop_recording();

        let buffer = self
            .buffer
            .lock()
            .map_err(|_| AudioCaptureError::Encoding("Failed to lock buffer".to_string()))?;

        let stats = buffer.level_stats();
        let (wav_bytes, _diag) = buffer.to_wav_bytes_with_config(AudioEncodeConfig {
            noise_gate_threshold_dbfs: noise_gate_threshold_dbfs_from_strength(noise_gate_strength),
            ..Default::default()
        })?;

        log::info!(
            "Audio capture stopped, {} bytes captured (duration {:.2}s, rms {:.6}, peak {:.6})",
            wav_bytes.len(),
            stats.duration_secs,
            stats.rms,
            stats.peak
        );

        Ok((wav_bytes, stats))
    }

    /// Stop recording and return WAV bytes + diagnostics, applying preprocessing.
    pub fn stop_and_get_wav_with_diagnostics(
        &mut self,
        cfg: AudioEncodeConfig,
    ) -> Result<(Vec<u8>, AudioCaptureDiagnostics), AudioCaptureError> {
        self.stop_recording();

        let buffer = self
            .buffer
            .lock()
            .map_err(|_| AudioCaptureError::Encoding("Failed to lock buffer".to_string()))?;

        buffer.to_wav_bytes_with_config(cfg)
    }

    /// Stop recording and return two WAV encodes of the same captured audio:
    /// - "before": raw, with no preprocessing/gates
    /// - "after": encoded with the provided config
    ///
    /// This is intended for UI A/B testing of audio settings.
    pub fn stop_and_get_wav_before_after(
        &mut self,
        after_cfg: AudioEncodeConfig,
    ) -> Result<(Vec<u8>, Vec<u8>, AudioCaptureDiagnostics), AudioCaptureError> {
        self.stop_recording();

        let buffer = self
            .buffer
            .lock()
            .map_err(|_| AudioCaptureError::Encoding("Failed to lock buffer".to_string()))?;

        // "Before": as-captured (no downmix/resample/filters/gates).
        let (before_wav, _before_diag) = buffer.to_wav_bytes_with_config(AudioEncodeConfig {
            noise_gate_threshold_dbfs: None,
            downmix_to_mono: false,
            resample_to_16khz: false,
            highpass_enabled: false,
            agc_enabled: false,
            noise_suppression_enabled: false,
            detect_speech_presence: false,
        })?;

        // "After": apply current user settings.
        let (after_wav, after_diag) = buffer.to_wav_bytes_with_config(after_cfg)?;

        Ok((before_wav, after_wav, after_diag))
    }

    /// Stop recording without returning audio data
    pub fn stop(&mut self) {
        self.computer_capture = None;
        self.recording_active.store(false, Ordering::Relaxed);
        if let Some(handle) = self.capture_handle.take() {
            log::info!("Stopping audio capture");

            // Send stop command (ignore error if thread already stopped).
            let _ = handle.command_tx.send(CaptureCommand::Stop);

            // Join can block forever if the capture thread is stuck (e.g. callback stalls).
            // We use a helper joiner thread + recv_timeout so stop() itself is bounded.
            let (join_tx, join_rx) = mpsc::channel();
            thread::spawn(move || {
                let res = handle.thread_handle.join();
                let _ = join_tx.send(res);
            });

            match join_rx.recv_timeout(std::time::Duration::from_millis(STOP_JOIN_TIMEOUT_MS)) {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(e))) => {
                    log::warn!("Audio capture thread stopped with error: {}", e);
                }
                Ok(Err(_panic)) => {
                    log::warn!("Audio capture thread panicked while stopping");
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    log::warn!(
                        "AudioCapture::stop() timed out after {}ms; continuing without blocking",
                        STOP_JOIN_TIMEOUT_MS
                    );
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    log::warn!("AudioCapture::stop(): join waiter disconnected unexpectedly");
                }
            }
        }
    }

    /// Check if currently recording
    #[allow(dead_code)]
    pub fn is_recording(&self) -> bool {
        self.recording_active.load(Ordering::Relaxed)
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
    #[allow(dead_code)]
    pub fn duration_secs(&self) -> f32 {
        self.buffer.lock().map(|b| b.duration_secs()).unwrap_or(0.0)
    }

    /// Get the sample rate
    #[allow(dead_code)]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Get the number of channels
    #[allow(dead_code)]
    pub fn channels(&self) -> u16 {
        self.channels
    }
}

impl AudioCaptureBackend for AudioCapture {
    fn recording_progress(&self) -> (f64, bool) {
        self.buffer
            .lock()
            .map(|buffer| {
                (
                    buffer.captured_samples as f64
                        / buffer.sample_rate.max(1) as f64
                        / buffer.channels.max(1) as f64,
                    buffer.journal_failed,
                )
            })
            .unwrap_or((0.0, true))
    }
    fn set_computer_audio(&mut self, enabled: bool) -> Result<(), AudioCaptureError> {
        if enabled && !computer_audio::available() {
            return Err(AudioCaptureError::StreamStart(
                "Computer audio is not supported on this installation".into(),
            ));
        }
        self.computer_audio_enabled = enabled;
        Ok(())
    }
    fn recovery_path(&self) -> Option<std::path::PathBuf> {
        self.buffer
            .lock()
            .ok()
            .and_then(|buffer| buffer.recovery_path.clone())
    }
    fn discard_recovery(&mut self) -> Result<(), AudioCaptureError> {
        let mut buffer = self
            .buffer
            .lock()
            .map_err(|_| AudioCaptureError::ThreadError("Recovery buffer unavailable".into()))?;
        buffer.journal = None;
        if let Some(path) = buffer.recovery_path.take() {
            if journal::discard(&path).is_err() {
                buffer.recovery_path = Some(path);
                return Err(AudioCaptureError::ThreadError(
                    "Could not discard recovery audio".into(),
                ));
            }
        }
        Ok(())
    }
    fn set_recovery_path(
        &mut self,
        path: Option<std::path::PathBuf>,
    ) -> Result<(), AudioCaptureError> {
        let mut buffer = self
            .buffer
            .lock()
            .map_err(|_| AudioCaptureError::ThreadError("Recovery buffer unavailable".into()))?;
        buffer.journal = None;
        buffer.journal_failed = false;
        buffer.recovery_path = path;
        Ok(())
    }
    fn set_paused(&mut self, paused: bool) -> Result<(), AudioCaptureError> {
        if self.capture_handle.is_none() && self.computer_capture.is_none() {
            return Err(AudioCaptureError::NotActive);
        }
        self.recording_active.store(!paused, Ordering::Relaxed);
        Ok(())
    }
    fn shared_level_meter(&self) -> SharedAudioLevelMeter {
        self.shared_level_meter()
    }

    fn shared_waveform_meter(&self) -> SharedAudioWaveformMeter {
        self.shared_waveform_meter()
    }

    fn level_snapshot(&self) -> AudioLevelSnapshot {
        self.level_snapshot()
    }

    fn set_vad_config(&mut self, config: VadAutoStopConfig) {
        self.set_vad_config(config);
    }

    fn set_capture_behavior(
        &mut self,
        hot_mic_enabled: bool,
        hot_mic_pre_roll_ms: u32,
        mic_auto_recover_enabled: bool,
        input_device_name: Option<&str>,
    ) -> Result<(), AudioCaptureError> {
        self.set_capture_behavior(
            hot_mic_enabled,
            hot_mic_pre_roll_ms,
            mic_auto_recover_enabled,
            input_device_name,
        )
    }

    fn start_recording_session(
        &mut self,
        max_duration_secs: f32,
        input_device_name: Option<&str>,
    ) -> Result<(), AudioCaptureError> {
        self.start_recording_session(max_duration_secs, input_device_name)
    }

    fn stop_and_get_wav_with_diagnostics(
        &mut self,
        cfg: AudioEncodeConfig,
    ) -> Result<(Vec<u8>, AudioCaptureDiagnostics), AudioCaptureError> {
        self.stop_and_get_wav_with_diagnostics(cfg)
    }

    fn stop_and_get_wav_before_after(
        &mut self,
        after_cfg: AudioEncodeConfig,
    ) -> Result<(Vec<u8>, Vec<u8>, AudioCaptureDiagnostics), AudioCaptureError> {
        self.stop_and_get_wav_before_after(after_cfg)
    }

    fn stop_recording(&mut self) {
        self.stop_recording();
    }

    fn stop(&mut self) {
        self.stop();
    }

    fn poll_vad_event(&self) -> Option<AudioCaptureEvent> {
        self.poll_vad_event()
    }

    fn is_vad_auto_stop_enabled(&self) -> bool {
        self.is_vad_auto_stop_enabled()
    }

    fn set_live_audio_tx(&mut self, tx: Option<tokio::sync::mpsc::Sender<Vec<f32>>>) {
        self.set_live_audio_tx(tx);
    }

    fn capture_sample_rate(&self) -> u32 {
        self.capture_sample_rate()
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

struct CaptureThreadArgs {
    device: cpal::Device,
    config: cpal::StreamConfig,
    sample_format: SampleFormat,
    buffer: Arc<StdMutex<AudioBuffer>>,
    pre_roll: Arc<StdMutex<RollingBuffer>>,
    recording_active: Arc<AtomicBool>,
    pre_roll_ms: Arc<AtomicU32>,
    meter: Arc<AudioLevelMeter>,
    waveform_meter: Arc<AudioWaveformMeter>,
    command_rx: mpsc::Receiver<CaptureCommand>,
    event_tx: mpsc::Sender<AudioCaptureEvent>,
    vad_config: VadAutoStopConfig,
    auto_recover_enabled: Arc<AtomicBool>,
    desired_device_name: Arc<StdMutex<Option<String>>>,
    live_audio_tx: Arc<StdMutex<Option<tokio::sync::mpsc::Sender<Vec<f32>>>>>,
}

/// Run the audio capture in a dedicated thread
fn run_capture_thread(args: CaptureThreadArgs) -> Result<(), AudioCaptureError> {
    use std::time::{Duration, Instant};

    let CaptureThreadArgs {
        mut device,
        config,
        sample_format,
        buffer,
        pre_roll,
        recording_active,
        pre_roll_ms,
        meter,
        waveform_meter,
        command_rx,
        event_tx,
        vad_config,
        auto_recover_enabled,
        desired_device_name,
        live_audio_tx,
    } = args;
    let sample_rate = config.sample_rate;

    // Used for watchdog timing (monotonic).
    let start = Instant::now();
    let last_callback_ms = Arc::new(AtomicU64::new(0));

    fn stream_err(err: cpal::StreamError) {
        log::error!("Audio stream error: {}", err);
    }

    #[derive(Debug)]
    struct CapturedChunk {
        samples: Vec<f32>,
        was_recording: bool,
    }

    // Channel + pool for passing samples to the VAD processing thread.
    // The pool avoids per-callback allocations; the callback takes a Vec from the pool,
    // fills it with mono f32, and the VAD thread returns it to the pool after processing.
    let (vad_samples_tx, vad_samples_rx): (mpsc::Sender<Vec<f32>>, mpsc::Receiver<Vec<f32>>) =
        mpsc::channel();
    let vad_pool: Option<Arc<ArrayQueue<Vec<f32>>>> = if vad_config.enabled {
        const VAD_POOL_SIZE: usize = 8;

        // VAD input is mono, but we provision capacity based on the interleaved callback size
        // to conservatively avoid resizing when channels > 1.
        let cap = estimate_callback_interleaved_capacity(&config, config.channels as usize);
        let pool = Arc::new(ArrayQueue::new(VAD_POOL_SIZE));
        for _ in 0..VAD_POOL_SIZE {
            // Preallocate buffers; actual length is set in the callback.
            let _ = pool.push(Vec::with_capacity(cap));
        }
        Some(pool)
    } else {
        None
    };

    // Queue + pool for passing *interleaved f32* samples from the realtime callback
    // to a non-realtime worker thread.
    //
    // Goal: keep the CPAL callback fast (no mutex locks, no VAD work, no waveform binning).
    const CAPTURE_CHUNK_QUEUE_CAPACITY: usize = 48;
    const CAPTURE_CHUNK_POOL_SIZE: usize = 48;
    let callback_capacity =
        estimate_callback_interleaved_capacity(&config, config.channels.max(1) as usize);

    let chunk_queue: Arc<ArrayQueue<CapturedChunk>> =
        Arc::new(ArrayQueue::new(CAPTURE_CHUNK_QUEUE_CAPACITY));
    let chunk_pool: Arc<ArrayQueue<Vec<f32>>> = Arc::new(ArrayQueue::new(CAPTURE_CHUNK_POOL_SIZE));
    for _ in 0..CAPTURE_CHUNK_POOL_SIZE {
        // Preallocate buffers; actual length is set by the callback.
        let _ = chunk_pool.push(Vec::with_capacity(callback_capacity));
    }

    let worker_stop = Arc::new(AtomicBool::new(false));
    let worker_handle = {
        let buffer = buffer.clone();
        let pre_roll = pre_roll.clone();
        let recording_active = recording_active.clone();
        let pre_roll_ms = pre_roll_ms.clone();
        let meter = meter.clone();
        let waveform_meter = waveform_meter.clone();
        let queue = chunk_queue.clone();
        let pool = chunk_pool.clone();
        let stop = worker_stop.clone();
        let vad_tx = if vad_config.enabled {
            Some(vad_samples_tx.clone())
        } else {
            None
        };
        let vad_pool = vad_pool.clone();
        let channels = config.channels.max(1) as usize;
        let sample_rate = config.sample_rate;
        let live_audio_tx = live_audio_tx.clone();

        thread::spawn(move || {
            let mut last_pre_roll_capacity: usize = 0;

            let mut process_one = |chunk: CapturedChunk| {
                // Keep level/waveform meters up-to-date even when not recording.
                let (rms, peak) = compute_rms_peak(&chunk.samples);
                meter.update(rms, peak);
                waveform_meter.update_from_f32_interleaved(chunk.samples.as_slice(), channels);

                // Ensure pre-roll capacity matches the current setting (best effort).
                // This is safe to do here because we're off the realtime callback.
                let desired_cap = pre_roll_capacity_samples(
                    sample_rate,
                    channels,
                    pre_roll_ms.load(Ordering::Relaxed),
                );
                if desired_cap != last_pre_roll_capacity {
                    if let Ok(mut pr) = pre_roll.lock() {
                        pr.set_capacity(desired_cap);
                    }
                    last_pre_roll_capacity = desired_cap;
                }

                // Always maintain rolling pre-roll.
                if let Ok(mut pr) = pre_roll.lock() {
                    pr.push(chunk.samples.as_slice());
                }

                // Recording buffer + VAD should reflect the state at callback-time,
                // not "now" (avoids dropping the tail when recording_active flips).
                if chunk.was_recording {
                    if let Ok(mut buf) = buffer.lock() {
                        buf.append(chunk.samples.as_slice());
                    }

                    if let Some(ref tx) = vad_tx {
                        if let Some(ref pool) = vad_pool {
                            if let Some(mut mono) = pool.pop() {
                                downmix_interleaved_chunk_to_mono_into(
                                    chunk.samples.as_slice(),
                                    channels,
                                    &mut mono,
                                );
                                if let Err(mpsc::SendError(mut mono)) = tx.send(mono) {
                                    mono.clear();
                                    let _ = pool.push(mono);
                                }
                            }
                        }
                    }

                    // Live audio streaming: send mono f32 chunks for concurrent STT.
                    if let Ok(guard) = live_audio_tx.lock() {
                        if let Some(ref sender) = *guard {
                            let mono =
                                downmix_interleaved_to_mono(chunk.samples.as_slice(), channels);
                            // Use try_send to avoid blocking the capture worker.
                            // If the channel is full the chunk is silently dropped;
                            // the STT server still gets enough audio from other chunks.
                            let _ = sender.try_send(mono);
                        }
                    }
                }

                // Return the backing buffer to the pool.
                let mut samples = chunk.samples;
                samples.clear();
                let _ = pool.push(samples);
            };

            loop {
                // Drain the queue quickly when there is work.
                if let Some(chunk) = queue.pop() {
                    process_one(chunk);
                    continue;
                }

                if stop.load(Ordering::Relaxed) {
                    break;
                }

                // Idle: avoid a tight spin.
                std::thread::sleep(std::time::Duration::from_millis(1));
            }

            // Best-effort flush on stop.
            while let Some(chunk) = queue.pop() {
                process_one(chunk);
            }

            // Keep the compiler honest: this worker intentionally does not read
            // recording_active (state is captured per-chunk).
            let _ = recording_active;
        })
    };

    // Spawn a separate thread for VAD processing (since webrtc-vad is not Send)
    let vad_handle = if vad_config.enabled {
        let event_tx_clone = event_tx.clone();
        let vad_cfg = vad_config.vad_config.clone();
        let pool = vad_pool.clone();
        Some(thread::spawn(move || {
            let mut processor = VadFrameProcessor::new(vad_cfg, sample_rate);
            log::info!(
                "VAD processor initialized for {} Hz audio in dedicated thread",
                sample_rate
            );

            loop {
                match vad_samples_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(mut samples) => {
                        for event in processor.process(&samples) {
                            let capture_event = match event {
                                VadEvent::SpeechStart { .. } => AudioCaptureEvent::SpeechStart,
                                VadEvent::SpeechEnd => AudioCaptureEvent::SpeechEnd,
                                VadEvent::None => continue,
                            };
                            let _ = event_tx_clone.send(capture_event);
                        }

                        // Return the buffer to the pool for reuse.
                        samples.clear();
                        if let Some(ref pool) = pool {
                            let _ = pool.push(samples);
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

    #[allow(clippy::too_many_arguments)]
    fn build_stream(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        sample_format: SampleFormat,
        start: Instant,
        last_callback_ms: Arc<AtomicU64>,
        recording_active: Arc<AtomicBool>,
        pre_roll_ms: Arc<AtomicU32>,
        chunk_queue: Arc<ArrayQueue<CapturedChunk>>,
        chunk_pool: Arc<ArrayQueue<Vec<f32>>>,
    ) -> Result<cpal::Stream, AudioCaptureError> {
        use std::cell::RefCell;

        let channels = config.channels.max(1) as usize;
        let scratch_capacity = estimate_callback_interleaved_capacity(config, channels);

        match sample_format {
            SampleFormat::F32 => {
                let queue = chunk_queue.clone();
                let pool = chunk_pool.clone();
                let last_callback_ms = last_callback_ms.clone();
                let recording_active = recording_active.clone();
                let pre_roll_ms = pre_roll_ms.clone();

                device
                    .build_input_stream(
                        config,
                        move |data: &[f32], _: &cpal::InputCallbackInfo| {
                            last_callback_ms
                                .store(start.elapsed().as_millis() as u64, Ordering::Relaxed);

                            let was_recording = recording_active.load(Ordering::Relaxed);

                            // Fast path: try to reuse a preallocated Vec.
                            let mut samples = match pool.pop() {
                                Some(v) => v,
                                None => return,
                            };

                            samples.clear();
                            samples.extend_from_slice(data);

                            let mut chunk = CapturedChunk {
                                samples,
                                was_recording,
                            };

                            // If the queue is full, drop the oldest chunk to make space, then retry.
                            if let Err(returned) = queue.push(chunk) {
                                chunk = returned;

                                if let Some(oldest) = queue.pop() {
                                    let mut old = oldest.samples;
                                    old.clear();
                                    let _ = pool.push(old);
                                }

                                if let Err(returned_again) = queue.push(chunk) {
                                    let mut samples = returned_again.samples;
                                    samples.clear();
                                    let _ = pool.push(samples);
                                }
                            }

                            // Keep capacity in sync if pre-roll ms changes drastically.
                            let _ = pre_roll_ms.load(Ordering::Relaxed);
                        },
                        stream_err,
                        None,
                    )
                    .map_err(|e| AudioCaptureError::StreamBuild(e.to_string()))
            }
            SampleFormat::I16 => {
                let scratch = RefCell::new(Vec::<f32>::with_capacity(scratch_capacity));
                let queue = chunk_queue.clone();
                let pool = chunk_pool.clone();
                let last_callback_ms = last_callback_ms.clone();
                let recording_active = recording_active.clone();
                let pre_roll_ms = pre_roll_ms.clone();

                device
                    .build_input_stream(
                        config,
                        move |data: &[i16], _: &cpal::InputCallbackInfo| {
                            last_callback_ms
                                .store(start.elapsed().as_millis() as u64, Ordering::Relaxed);

                            let was_recording = recording_active.load(Ordering::Relaxed);

                            let mut out = match pool.pop() {
                                Some(v) => v,
                                None => return,
                            };
                            out.clear();

                            let mut tmp = scratch.borrow_mut();
                            tmp.clear();
                            for &s in data {
                                // Normalize PCM i16 to [-1.0, 1.0]
                                let v = (s as f32) / (i16::MAX as f32);
                                tmp.push(v.clamp(-1.0, 1.0));
                            }
                            out.extend_from_slice(tmp.as_slice());

                            let mut chunk = CapturedChunk {
                                samples: out,
                                was_recording,
                            };

                            if let Err(returned) = queue.push(chunk) {
                                chunk = returned;
                                if let Some(oldest) = queue.pop() {
                                    let mut old = oldest.samples;
                                    old.clear();
                                    let _ = pool.push(old);
                                }
                                if let Err(returned_again) = queue.push(chunk) {
                                    let mut samples = returned_again.samples;
                                    samples.clear();
                                    let _ = pool.push(samples);
                                }
                            }

                            let _ = pre_roll_ms.load(Ordering::Relaxed);
                        },
                        stream_err,
                        None,
                    )
                    .map_err(|e| AudioCaptureError::StreamBuild(e.to_string()))
            }
            SampleFormat::U16 => {
                let scratch = RefCell::new(Vec::<f32>::with_capacity(scratch_capacity));
                let queue = chunk_queue.clone();
                let pool = chunk_pool.clone();
                let last_callback_ms = last_callback_ms.clone();
                let recording_active = recording_active.clone();
                let pre_roll_ms = pre_roll_ms.clone();

                device
                    .build_input_stream(
                        config,
                        move |data: &[u16], _: &cpal::InputCallbackInfo| {
                            last_callback_ms
                                .store(start.elapsed().as_millis() as u64, Ordering::Relaxed);

                            let was_recording = recording_active.load(Ordering::Relaxed);

                            let mut out = match pool.pop() {
                                Some(v) => v,
                                None => return,
                            };
                            out.clear();

                            let mut tmp = scratch.borrow_mut();
                            tmp.clear();
                            for &s in data {
                                // Normalize PCM u16 (0..=65535) to [-1.0, 1.0]
                                let v = (s as f32) / (u16::MAX as f32);
                                tmp.push((v * 2.0 - 1.0).clamp(-1.0, 1.0));
                            }
                            out.extend_from_slice(tmp.as_slice());

                            let mut chunk = CapturedChunk {
                                samples: out,
                                was_recording,
                            };

                            if let Err(returned) = queue.push(chunk) {
                                chunk = returned;
                                if let Some(oldest) = queue.pop() {
                                    let mut old = oldest.samples;
                                    old.clear();
                                    let _ = pool.push(old);
                                }
                                if let Err(returned_again) = queue.push(chunk) {
                                    let mut samples = returned_again.samples;
                                    samples.clear();
                                    let _ = pool.push(samples);
                                }
                            }

                            let _ = pre_roll_ms.load(Ordering::Relaxed);
                        },
                        stream_err,
                        None,
                    )
                    .map_err(|e| AudioCaptureError::StreamBuild(e.to_string()))
            }
            _ => Err(AudioCaptureError::DeviceConfig(format!(
                "Unsupported sample format: {:?}",
                sample_format
            ))),
        }
    }

    let mut stream = build_stream(
        &device,
        &config,
        sample_format,
        start,
        last_callback_ms.clone(),
        recording_active.clone(),
        pre_roll_ms.clone(),
        chunk_queue.clone(),
        chunk_pool.clone(),
    )?;

    stream
        .play()
        .map_err(|e| AudioCaptureError::StreamStart(e.to_string()))?;

    // Wait for stop command, with optional watchdog restart.
    let mut consecutive_restart_failures: u32 = 0;

    loop {
        match command_rx.recv_timeout(Duration::from_millis(WATCHDOG_CHECK_EVERY_MS)) {
            Ok(CaptureCommand::Stop) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Watchdog: if we're idle and callbacks have stalled, attempt to restart.
                if !auto_recover_enabled.load(Ordering::Relaxed) {
                    continue;
                }
                if recording_active.load(Ordering::Relaxed) {
                    continue;
                }

                let now_ms = start.elapsed().as_millis() as u64;
                let last_ms = last_callback_ms.load(Ordering::Relaxed);
                if last_ms == 0 {
                    // Stream may not have delivered a callback yet.
                    continue;
                }

                if now_ms.saturating_sub(last_ms) < WATCHDOG_STALL_MS {
                    continue;
                }

                log::warn!(
                    "Audio capture watchdog: no callback for {}ms; attempting stream restart",
                    now_ms.saturating_sub(last_ms)
                );

                // Clear pre-roll to avoid carrying stale audio across a restart.
                if let Ok(mut pr) = pre_roll.lock() {
                    pr.clear();
                }

                // Try rebuilding the stream on the current device.
                match build_stream(
                    &device,
                    &config,
                    sample_format,
                    start,
                    last_callback_ms.clone(),
                    recording_active.clone(),
                    pre_roll_ms.clone(),
                    chunk_queue.clone(),
                    chunk_pool.clone(),
                )
                .and_then(|s| {
                    s.play()
                        .map_err(|e| AudioCaptureError::StreamStart(e.to_string()))
                        .map(|_| s)
                }) {
                    Ok(new_stream) => {
                        let _previous_stream = std::mem::replace(&mut stream, new_stream);
                        consecutive_restart_failures = 0;
                        last_callback_ms.store(now_ms, Ordering::Relaxed);
                        continue;
                    }
                    Err(e) => {
                        consecutive_restart_failures =
                            consecutive_restart_failures.saturating_add(1);
                        log::warn!("Audio capture watchdog: restart failed: {}", e);
                    }
                }

                // Optional: try rebinding to the desired/default device using the same config.
                let maybe_name = desired_device_name.lock().ok().and_then(|n| n.clone());
                let host = cpal::default_host();
                let rebound = select_input_device_from_host(&host, maybe_name.as_deref())
                    .or_else(|| host.default_input_device());
                if let Some(new_device) = rebound {
                    device = new_device;
                    if let Ok(mut buf) = buffer.lock() {
                        buf.set_format(config.sample_rate, config.channels);
                    }
                    // Resize pre-roll based on current config and configured ms.
                    let cap_samples = pre_roll_capacity_samples(
                        config.sample_rate,
                        config.channels as usize,
                        pre_roll_ms.load(Ordering::Relaxed),
                    );
                    if let Ok(mut pr) = pre_roll.lock() {
                        pr.set_capacity(cap_samples);
                    }

                    match build_stream(
                        &device,
                        &config,
                        sample_format,
                        start,
                        last_callback_ms.clone(),
                        recording_active.clone(),
                        pre_roll_ms.clone(),
                        chunk_queue.clone(),
                        chunk_pool.clone(),
                    )
                    .and_then(|s| {
                        s.play()
                            .map_err(|e| AudioCaptureError::StreamStart(e.to_string()))
                            .map(|_| s)
                    }) {
                        Ok(new_stream) => {
                            let _previous_stream = std::mem::replace(&mut stream, new_stream);
                            consecutive_restart_failures = 0;
                            last_callback_ms.store(now_ms, Ordering::Relaxed);
                            continue;
                        }
                        Err(e) => {
                            consecutive_restart_failures =
                                consecutive_restart_failures.saturating_add(1);
                            log::warn!("Audio capture watchdog: rebind restart failed: {}", e);
                        }
                    }
                }

                // Backoff to avoid tight restart loops.
                let backoff_ms = watchdog_restart_backoff_ms(consecutive_restart_failures);
                std::thread::sleep(Duration::from_millis(backoff_ms));
            }
        }
    }

    // Stop the worker and flush any pending chunks into the buffers before we shut down.
    worker_stop.store(true, Ordering::Relaxed);
    let _ = worker_handle.join();

    // Drop the VAD sender to signal the VAD thread to stop
    drop(vad_samples_tx);

    // Wait for VAD thread to finish
    if let Some(handle) = vad_handle {
        let _ = handle.join();
    }

    // Stream is dropped here, stopping capture
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn recovery_keeps_audio_beyond_the_memory_ring_and_explicit_discard_removes_it() {
        use super::*;
        let path =
            std::env::temp_dir().join(format!("kolboo-ring-recovery-{}.pcm", uuid::Uuid::new_v4()));
        let mut capture = AudioCapture::new();
        AudioCaptureBackend::set_recovery_path(&mut capture, Some(path.clone())).unwrap();
        {
            let mut buffer = capture.buffer.lock().unwrap();
            buffer.set_format(16000, 1);
            buffer.reset_for_recording(0.01);
            buffer.append(&vec![0.25; 16000]);
            assert_eq!(buffer.snapshot().len(), 160);
            assert_eq!(buffer.captured_samples, 16000);
            buffer.to_wav_bytes().unwrap();
        }
        assert_eq!(journal::read_chunk(&path, 0, 16000).unwrap().2.len(), 16000);
        crate::recordings::options::RecordingPreferences::default()
            .save_journal(&path)
            .unwrap();
        AudioCaptureBackend::discard_recovery(&mut capture).unwrap();
        assert!(!path.exists());
        assert!(!path.with_extension("options.json").exists());
        assert!(capture.buffer.lock().unwrap().recovery_path.is_none());
    }

    use super::*;
    use std::sync::atomic::AtomicBool;

    fn spawn_test_capture_handle(
        on_stop: Option<Arc<AtomicBool>>,
    ) -> (CaptureHandle, mpsc::Sender<AudioCaptureEvent>) {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let thread_handle = thread::spawn(move || {
            let _ = command_rx.recv();
            if let Some(flag) = on_stop {
                flag.store(true, Ordering::Relaxed);
            }
            Ok(())
        });

        (
            CaptureHandle {
                command_tx,
                event_rx,
                thread_handle,
            },
            event_tx,
        )
    }

    fn attach_running_capture(
        capture: &mut AudioCapture,
    ) -> (mpsc::Sender<AudioCaptureEvent>, Arc<AtomicBool>) {
        let stopped = Arc::new(AtomicBool::new(false));
        let (handle, event_tx) = spawn_test_capture_handle(Some(stopped.clone()));
        capture.capture_handle = Some(handle);
        (event_tx, stopped)
    }

    fn set_capture_format_for_tests(capture: &mut AudioCapture, sample_rate: u32, channels: u16) {
        capture.sample_rate = sample_rate;
        capture.channels = channels.max(1);
        if let Ok(mut buffer) = capture.buffer.lock() {
            buffer.set_format(sample_rate, channels);
        }
    }

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

    #[test]
    fn test_audio_buffer_set_format_keeps_newest_samples_when_capacity_changes() {
        let mut buffer = AudioBuffer::new(4, 1, 1.0);
        buffer.append(&[1.0, 2.0, 3.0, 4.0]);

        buffer.set_format(2, 1);

        assert_eq!(buffer.sample_rate(), 2);
        assert_eq!(buffer.channels(), 1);
        assert_eq!(buffer.snapshot(), vec![3.0, 4.0]);
    }

    #[test]
    fn test_audio_buffer_reset_for_recording_clears_existing_audio() {
        let mut buffer = AudioBuffer::new(16000, 1, 2.0);
        buffer.append(&[1.0, 2.0, 3.0, 4.0]);

        buffer.reset_for_recording(0.5);

        assert!(buffer.is_empty());
        assert_eq!(
            buffer.capacity(),
            AudioBuffer::max_samples_for(16000, 1, 0.5)
        );
    }

    #[test]
    fn test_rolling_buffer_set_capacity_keeps_newest_tail() {
        let mut buffer = RollingBuffer::new(6);
        buffer.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);

        buffer.set_capacity(4);

        assert_eq!(buffer.capacity(), 4);
        assert_eq!(buffer.snapshot(), vec![3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn test_set_capture_behavior_enabling_hot_mic_reuses_running_stream_and_resizes_pre_roll() {
        let mut capture = AudioCapture::new();
        let (_event_tx, _stopped) = attach_running_capture(&mut capture);
        set_capture_format_for_tests(&mut capture, 1000, 2);
        if let Ok(mut pre_roll) = capture.pre_roll.lock() {
            pre_roll.set_capacity(10);
            pre_roll.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        }

        capture
            .set_capture_behavior(true, 3, true, Some("Built-in Mic"))
            .expect("enabling hot mic on an already-running stream should reuse it");

        assert!(capture.capture_handle.is_some());
        assert_eq!(capture.pre_roll_ms.load(Ordering::Relaxed), 3);
        assert!(capture.mic_auto_recover_enabled.load(Ordering::Relaxed));
        assert_eq!(
            capture
                .desired_device_name
                .lock()
                .ok()
                .and_then(|name| name.clone()),
            Some("Built-in Mic".to_string())
        );

        let pre_roll = capture
            .pre_roll
            .lock()
            .expect("pre-roll lock should remain available in tests");
        assert_eq!(pre_roll.capacity(), pre_roll_capacity_samples(1000, 2, 3));
        assert_eq!(pre_roll.snapshot(), vec![3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
    }

    #[test]
    fn test_set_capture_behavior_disabling_hot_mic_stops_idle_stream_and_normalizes_default_device()
    {
        let mut capture = AudioCapture::new();
        let (_event_tx, stopped) = attach_running_capture(&mut capture);
        capture.recording_active.store(false, Ordering::Relaxed);

        capture
            .set_capture_behavior(false, 9000, false, Some(" default "))
            .expect("disabling hot mic while idle should succeed");

        assert!(stopped.load(Ordering::Relaxed));
        assert!(capture.capture_handle.is_none());
        assert_eq!(capture.pre_roll_ms.load(Ordering::Relaxed), MAX_PRE_ROLL_MS);
        assert!(!capture.mic_auto_recover_enabled.load(Ordering::Relaxed));
        assert_eq!(
            capture
                .desired_device_name
                .lock()
                .ok()
                .and_then(|name| name.clone()),
            None
        );
    }

    #[test]
    fn test_disabling_hot_mic_while_recording_keeps_stream_until_recording_stops() {
        let mut capture = AudioCapture::new();
        let (_event_tx, stopped) = attach_running_capture(&mut capture);
        capture.hot_mic_enabled = true;
        capture.recording_active.store(true, Ordering::Relaxed);

        capture
            .set_capture_behavior(false, 250, false, Some("Desk Mic"))
            .expect("switching to push-to-talk mid-recording should not tear down the stream");

        assert!(!stopped.load(Ordering::Relaxed));
        assert!(capture.capture_handle.is_some());
        assert!(!capture.hot_mic_enabled);

        capture.stop_recording();

        assert!(stopped.load(Ordering::Relaxed));
        assert!(capture.capture_handle.is_none());
        assert!(!capture.recording_active.load(Ordering::Relaxed));
    }

    #[test]
    fn test_start_recording_session_hot_mic_prepends_pre_roll_audio() {
        let mut capture = AudioCapture::new();
        let (_event_tx, _stopped) = attach_running_capture(&mut capture);
        set_capture_format_for_tests(&mut capture, 1000, 1);
        capture.hot_mic_enabled = true;
        capture.pre_roll_ms.store(4, Ordering::Relaxed);
        if let Ok(mut pre_roll) = capture.pre_roll.lock() {
            pre_roll.set_capacity(4);
            pre_roll.push(&[10.0, 20.0, 30.0]);
        }

        capture
            .start_recording_session(1.0, None)
            .expect("hot mic start should reuse the armed stream");

        assert!(capture.recording_active.load(Ordering::Relaxed));
        assert!(capture.capture_handle.is_some());
        let buffer = capture
            .buffer
            .lock()
            .expect("recording buffer lock should remain available in tests");
        assert_eq!(buffer.snapshot(), vec![10.0, 20.0, 30.0]);
        assert_eq!(
            buffer.capacity(),
            AudioBuffer::max_samples_for(1000, 1, 1.0)
        );
    }

    #[test]
    fn test_stop_recording_keeps_hot_mic_stream_running() {
        let mut capture = AudioCapture::new();
        let (_event_tx, stopped) = attach_running_capture(&mut capture);
        capture.hot_mic_enabled = true;
        capture.recording_active.store(true, Ordering::Relaxed);

        capture.stop_recording();

        assert!(!stopped.load(Ordering::Relaxed));
        assert!(capture.capture_handle.is_some());
        assert!(!capture.recording_active.load(Ordering::Relaxed));

        capture.stop();
        assert!(stopped.load(Ordering::Relaxed));
    }

    #[test]
    fn test_poll_vad_event_returns_buffered_events_in_order() {
        let mut capture = AudioCapture::new();
        let (event_tx, _stopped) = attach_running_capture(&mut capture);
        event_tx
            .send(AudioCaptureEvent::SpeechStart)
            .expect("test event send should succeed");
        event_tx
            .send(AudioCaptureEvent::SpeechEnd)
            .expect("test event send should succeed");

        assert!(matches!(
            capture.poll_vad_event(),
            Some(AudioCaptureEvent::SpeechStart)
        ));
        assert!(matches!(
            capture.poll_vad_event(),
            Some(AudioCaptureEvent::SpeechEnd)
        ));
        assert!(capture.poll_vad_event().is_none());
    }

    #[test]
    fn test_drop_stops_running_capture_thread() {
        let stopped = Arc::new(AtomicBool::new(false));

        {
            let mut capture = AudioCapture::new();
            let (handle, _event_tx) = spawn_test_capture_handle(Some(stopped.clone()));
            capture.capture_handle = Some(handle);
        }

        assert!(stopped.load(Ordering::Relaxed));
    }

    #[test]
    fn test_watchdog_restart_backoff_steps_up_with_failures() {
        assert_eq!(watchdog_restart_backoff_ms(0), 200);
        assert_eq!(watchdog_restart_backoff_ms(1), 200);
        assert_eq!(watchdog_restart_backoff_ms(2), 500);
        assert_eq!(watchdog_restart_backoff_ms(3), 1000);
        assert_eq!(watchdog_restart_backoff_ms(4), 2000);
    }
}
