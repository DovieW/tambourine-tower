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
use std::sync::{Arc, Mutex as StdMutex};
use std::thread::{self, JoinHandle};

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
    NotActive,

    #[error("Capture thread error: {0}")]
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
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Get the number of samples in the buffer
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Check if the buffer is empty
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

            for &sample in &self.samples {
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
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Get the number of channels
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
        }
    }

    /// Update VAD configuration
    pub fn set_vad_config(&mut self, config: VadAutoStopConfig) {
        self.vad_config = config;
    }

    /// Get the current VAD configuration
    pub fn vad_config(&self) -> &VadAutoStopConfig {
        &self.vad_config
    }

    /// Start recording audio from the default input device
    ///
    /// # Arguments
    /// * `max_duration_secs` - Maximum recording duration in seconds (for buffer sizing)
    pub fn start(&mut self, max_duration_secs: f32) -> Result<(), AudioCaptureError> {
        // Stop any existing recording
        self.stop();

        // Get device info first (on main thread)
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or(AudioCaptureError::NoInputDevice)?;

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
    pub fn stop_and_get_wav(&mut self) -> Result<Vec<u8>, AudioCaptureError> {
        let (wav_bytes, _stats) = self.stop_and_get_wav_with_stats()?;
        Ok(wav_bytes)
    }

    /// Stop recording and return the captured audio as WAV bytes along with level stats.
    pub fn stop_and_get_wav_with_stats(
        &mut self,
    ) -> Result<(Vec<u8>, AudioLevelStats), AudioCaptureError> {
        self.stop();

        let buffer = self.buffer.lock().map_err(|_| {
            AudioCaptureError::Encoding("Failed to lock buffer".to_string())
        })?;

        let stats = buffer.level_stats();
        let wav_bytes = buffer.to_wav_bytes()?;

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
    pub fn is_recording(&self) -> bool {
        self.capture_handle.is_some()
    }

    /// Poll for VAD events (non-blocking)
    ///
    /// Returns the next VAD event if one is available, or None if no events are pending.
    /// This should be called periodically to check for speech start/end events.
    pub fn poll_vad_event(&self) -> Option<AudioCaptureEvent> {
        if let Some(ref handle) = self.capture_handle {
            handle.event_rx.try_recv().ok()
        } else {
            None
        }
    }

    /// Check if VAD auto-stop is enabled
    pub fn is_vad_auto_stop_enabled(&self) -> bool {
        self.vad_config.enabled && self.vad_config.auto_stop
    }

    /// Get the duration of recorded audio in seconds
    pub fn duration_secs(&self) -> f32 {
        self.buffer
            .lock()
            .map(|b| b.duration_secs())
            .unwrap_or(0.0)
    }

    /// Get the sample rate
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Get the number of channels
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
            let vad_tx = if vad_config.enabled { Some(vad_samples_tx.clone()) } else { None };
            device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
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
            let vad_tx = if vad_config.enabled { Some(vad_samples_tx.clone()) } else { None };
            device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let samples: Vec<f32> = data.iter().map(|&s| s.to_float_sample()).collect();

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
            let vad_tx = if vad_config.enabled { Some(vad_samples_tx.clone()) } else { None };
            device.build_input_stream(
                &config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    let samples: Vec<f32> = data.iter().map(|&s| s.to_float_sample()).collect();

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
