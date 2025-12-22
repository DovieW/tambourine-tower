# Option B: Remove Pipecat & Server — True "All-in-App" Architecture

This document outlines the implementation plan for removing the Python/Pipecat server and building a fully self-contained Tauri desktop application.

---

## ⚠️ Architecture Decision Points (Answer Before Building)

These questions create architecture forks—answer them first:

| Question | Options | Impact |
|----------|---------|--------|
| **Recording mode?** | Push-to-talk / Toggle / VAD auto-stop | Push-to-talk = no VAD needed for MVP |
| **Partial transcripts?** | Yes / No | Yes = streaming STT (much harder) |
| **Continuous dictation?** | One transcript per stop / Multi-utterance | Continuous = robust segmentation needed |

**Recommended MVP answers:** Push-to-talk (or toggle), no partials, one transcript per stop. This dramatically simplifies the initial implementation.

---

## Checklist

### Phase 1: End-to-End Manual Path (Ship First! 🚀)
> Goal: Prove capture → encoding → STT → typing works before adding complexity

- [x] Audio capture in Rust (already partially done via `cpal`)
- [x] Encode captured audio to WAV (use native device sample rate—let remote STT resample)
- [x] Hotkey start → record → hotkey stop → send WAV to remote STT
- [x] Receive transcript → invoke `type_text`
- [x] Basic error handling (provider failure → user feedback)
- [ ] **Milestone: Manual start/stop dictation works end-to-end**

### Phase 2: Remote STT Providers
> Ship with remote APIs first—lower complexity, smaller bundle, fewer platform issues

- [x] Implement STT provider trait/abstraction
- [x] OpenAI Whisper API provider
- [x] Groq provider
- [x] Deepgram provider (if needed)
- [x] Provider switching via settings
- [x] Timeouts, retries, rate-limit backoff
- [ ] **Milestone: All current remote STT providers working**

### Phase 3: Settings & Configuration Migration
- [x] Move settings API from Python server to Tauri commands
- [x] Migrate provider configuration + API keys to Rust/store
- [x] Update frontend to use Tauri invoke instead of HTTP
- [ ] **Milestone: No HTTP calls to localhost:8765**

### Phase 4: VAD Enhancement (Optional—Add After Core Works)
> Only add VAD if you want auto-stop or utterance splitting

- [x] Implement VAD with proper buffering:
  - [x] Ring buffer for **pre-roll** (~200-500ms before speech start)
  - [x] **Hangover period** (keep recording X ms after speech ends)
  - [x] Configurable silence threshold
- [ ] Handle edge cases:
  - [ ] Keyboard clicks / breathing / background noise
  - [ ] Multi-utterance sessions (if continuous dictation)
- [ ] **Debug mode**: visualize VAD decisions + energy level in UI
- [x] VAD options: `webrtc-vad` (simple) or Silero ONNX (accurate)
- [x] **Milestone: VAD-driven auto-stop works reliably**

### Phase 5: Pipeline Hardening ✅
> Address the "hard details" that break real-world usage

- [x] **Backpressure**: Handle STT calls slower than audio arrival
- [x] **Cancellation**: Stop/cancel must abort in-flight tasks cleanly (CancellationToken)
- [x] **Memory caps**: Bound buffer size for long sessions (MAX_WAV_SIZE_BYTES = 50MB)
- [x] **Error recovery**: Provider failure shouldn't wedge the app (PipelineState::Error recoverable)
- [x] **Concurrency**: Audio callback threads vs async runtime ownership
- [x] Model pipeline as explicit state machine with bounded queues (PipelineState enum with guards)
- [x] **Milestone: Robust under stress (long recordings, slow network, errors)**

### Phase 6: LLM Formatting (Optional) ✅
- [x] Implement direct LLM API calls from Rust
  - [x] OpenAI API client (src/llm/openai.rs)
  - [x] Anthropic API client (src/llm/anthropic.rs)
  - [x] Local Ollama support (src/llm/ollama.rs)
- [x] Port prompt templates from Python to Rust (src/llm/prompts.rs)
- [x] Add formatting pipeline step (integrated into pipeline.rs stop_and_transcribe)
- [x] **Milestone: Transcript → formatted text working**

### Phase 7: Local Whisper (Optional Power-User Feature)
> Treat as enhancement, not default—ships complexity
> **Build Requirements**: Requires `libclang` development libraries installed on the system to compile with this feature (`apt install libclang-dev` on Ubuntu/Debian, `brew install llvm` on macOS)

- [x] Integrate `whisper-rs` (whisper.cpp bindings) - Optional feature flag `local-whisper`
- [x] Implement resampling to 16kHz mono (uses existing `vad::resample_to_16khz()` from rubato)
- [x] Model distribution strategy (download on demand from Hugging Face)
- [x] Model selection UX - `WhisperModel` enum with all variants (Tiny→LargeV3Turbo)
- [x] Commands: `is_local_whisper_available`, `get_whisper_models`, `get_whisper_models_dir`, `is_whisper_model_downloaded`, `get_whisper_model_url`, `delete_whisper_model`, `validate_whisper_model`
- [ ] CPU/RAM impact management
- [ ] Consider GPU acceleration (platform-specific)
- [ ] **Milestone: Offline STT works for power users**

### Phase 8: Cleanup & Migration ✅
- [x] Remove `server/` directory
- [x] Remove Pipecat client dependencies from frontend (removed @pipecat-ai/*, @daily-co/daily-js, ky, zustand)
- [x] Update `OverlayApp.tsx` to use new Rust-based pipeline
- [x] Remove WebRTC/signaling code (removed recordingStore, config response handling)
- [x] Update documentation

### Phase 9: Testing & Polish ✅
- [x] Test all STT providers (unit tests + integration test framework)
- [x] Test LLM formatting providers (unit tests + integration test framework)
- [x] Edge case tests (state transitions, VAD, concurrent access)
- [x] Error handling and user feedback (visual error state in overlay, error parsing)
- [x] Performance benchmarking (latency, CPU usage) - 14 benchmarks covering VAD, resampling, pipeline, memory
- [ ] Cross-platform testing (Windows, macOS, Linux) - Requires manual testing on each platform

---

## Previous Architecture (Now Replaced)

```
┌─────────────────────────────────────────────────────────────────┐
│                        Previous Architecture                     │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────┐         WebRTC          ┌──────────────────┐  │
│  │  Tauri App   │ ◄─────────────────────► │  Python Server   │  │
│  │              │      /api/offer          │                  │  │
│  │  - UI        │                          │  - Pipecat       │  │
│  │  - Hotkeys   │   HTTP (config/settings) │  - VAD (Silero)  │  │
│  │  - Overlay   │ ◄─────────────────────► │  - STT calls     │  │
│  │  - Typing    │      :8765               │  - LLM calls     │  │
│  └──────────────┘                          └──────────────────┘  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

## Current Architecture (Option B - Implemented ✅)

```
┌─────────────────────────────────────────────────────────────────┐
│                        Current Architecture                      │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                      Tauri App (Single Process)            │ │
│  │                                                            │ │
│  │  ┌─────────────────────────────────────────────────────┐  │ │
│  │  │                    Rust Backend                      │  │ │
│  │  │                                                      │  │ │
│  │  │  Audio Capture ──► VAD ──► STT ──► LLM ──► Events   │  │ │
│  │  │      (cpal)       (vad)  (whisper/  (reqwest)        │  │ │
│  │  │                           API)                       │  │ │
│  │  │                                                      │  │ │
│  │  │  Settings Store (tauri-plugin-store)                │  │ │
│  │  └─────────────────────────────────────────────────────┘  │ │
│  │                           │                                │ │
│  │                     Tauri Events                           │ │
│  │                           ▼                                │ │
│  │  ┌─────────────────────────────────────────────────────┐  │ │
│  │  │                  React Frontend                      │  │ │
│  │  │  - Main Window (Settings)                           │  │ │
│  │  │  - Overlay Window (Recording UI)                    │  │ │
│  │  └─────────────────────────────────────────────────────┘  │ │
│  │                                                            │ │
│  └────────────────────────────────────────────────────────────┘ │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Detailed Implementation Guide

### Phase 1: Audio Pipeline in Rust

#### 1.1 Audio Capture (Already Exists)

The app already uses `cpal` for audio capture in [audio.rs](../app/src-tauri/src/audio.rs). Review and ensure it outputs raw PCM samples.

#### 1.2 Audio Resampling

> ⚠️ **Note for MVP**: Skip resampling initially! Most remote STT APIs accept various sample rates and resample server-side. Only add resampling when you integrate local whisper or if quality demands it.

When you do need resampling (for local whisper):

```rust
// Cargo.toml addition
rubato = "0.15"  # High-quality resampler

// IMPORTANT: Naive "resample each chunk independently" causes artifacts.
// You need overlap handling and correct chunk sizing.

fn resample_to_16khz(samples: &[f32], input_sample_rate: u32) -> Vec<f32> {
    if input_sample_rate == 16000 {
        return samples.to_vec();
    }

    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: rubato::WindowFunction::BlackmanHarris2,
    };

    let mut resampler = SincFixedIn::<f32>::new(
        16000.0 / input_sample_rate as f64,
        2.0,
        params,
        samples.len(),
        1,  // mono
    ).unwrap();

    let waves_in = vec![samples.to_vec()];
    let waves_out = resampler.process(&waves_in, None).unwrap();
    waves_out[0].clone()
}
```

#### 1.3 Voice Activity Detection (VAD)

> ⚠️ **Note**: VAD is optional for MVP. If using push-to-talk or toggle mode, skip this until Phase 4.

VAD is deceptively complex. Getting it wrong causes chopped words, laggy stops, or merged sentences. Key requirements:

- **Pre-roll buffer**: Ring buffer storing ~200-500ms *before* speech starts (so you don't clip the first syllable)
- **Hangover period**: Continue recording for X ms after speech ends (catches trailing consonants)
- **Debug visualization**: Show energy levels and VAD decisions in UI during development

**Option A: Use `webrtc-vad` crate (simpler)**

```rust
// Cargo.toml
webrtc-vad = "0.4"

// Usage - NOTE: requires specific frame sizes (10/20/30ms) and PCM16
use webrtc_vad::{Vad, SampleRate, VadMode};

struct VoiceActivityDetector {
    vad: Vad,
    is_speaking: bool,
    silence_frames: u32,
    speech_frames: u32,
    // Pre-roll ring buffer
    pre_roll_buffer: VecDeque<Vec<i16>>,
    pre_roll_frames: usize,  // ~20-50 frames for 200-500ms
    // Hangover
    hangover_frames: u32,    // ~30 frames for 300ms at 10ms/frame
}

impl VoiceActivityDetector {
    fn new() -> Self {
        let mut vad = Vad::new();
        vad.set_mode(VadMode::Aggressive);  // Less false positives
        Self {
            vad,
            is_speaking: false,
            silence_frames: 0,
            speech_frames: 0,
            pre_roll_buffer: VecDeque::with_capacity(50),
            pre_roll_frames: 30,  // ~300ms pre-roll
            hangover_frames: 30,  // ~300ms hangover
        }
    }

    fn process_frame(&mut self, samples: &[i16]) -> VadEvent {
        // Always maintain pre-roll buffer
        self.pre_roll_buffer.push_back(samples.to_vec());
        if self.pre_roll_buffer.len() > self.pre_roll_frames {
            self.pre_roll_buffer.pop_front();
        }

        let is_speech = self.vad.is_voice_segment(samples, SampleRate::Rate16kHz).unwrap_or(false);

        if is_speech {
            self.speech_frames += 1;
            self.silence_frames = 0;
            if !self.is_speaking && self.speech_frames > 3 {
                self.is_speaking = true;
                // Return pre-roll buffer contents with speech start
                return VadEvent::SpeechStart {
                    pre_roll: self.pre_roll_buffer.iter().flatten().cloned().collect()
                };
            }
        } else {
            self.silence_frames += 1;
            self.speech_frames = 0;
            // Hangover: wait longer before declaring speech end
            if self.is_speaking && self.silence_frames > self.hangover_frames {
                self.is_speaking = false;
                return VadEvent::SpeechEnd;
            }
        }
        VadEvent::None
    }
}

enum VadEvent {
    None,
    SpeechStart { pre_roll: Vec<i16> },
```

**Option B: Port Silero VAD (more accurate, harder)**

Requires ONNX runtime integration:

```rust
// Cargo.toml
ort = "2.0"  # ONNX Runtime bindings

// Would need to bundle silero_vad.onnx model
// More complex but matches current Python behavior
```

#### 1.4 Audio Buffer Management

```rust
struct AudioBuffer {
    samples: Vec<f32>,
    sample_rate: u32,
    max_duration_secs: f32,
}

impl AudioBuffer {
    fn new(sample_rate: u32, max_duration_secs: f32) -> Self {
        let capacity = (sample_rate as f32 * max_duration_secs) as usize;
        Self {
            samples: Vec::with_capacity(capacity),
            sample_rate,
            max_duration_secs,
        }
    }

    fn append(&mut self, new_samples: &[f32]) {
        self.samples.extend_from_slice(new_samples);
        // Trim if exceeds max duration
        let max_samples = (self.sample_rate as f32 * self.max_duration_secs) as usize;
        if self.samples.len() > max_samples {
            let drain_count = self.samples.len() - max_samples;
            self.samples.drain(0..drain_count);
        }
    }

    fn clear(&mut self) {
        self.samples.clear();
    }

    fn to_wav_bytes(&self) -> Vec<u8> {
        // Convert to WAV format for API calls
        // ... WAV header + PCM data
    }
}
```

---

### Phase 2: Speech-to-Text Integration

#### 2.1 STT Provider Trait

```rust
use async_trait::async_trait;

#[async_trait]
pub trait SttProvider: Send + Sync {
    async fn transcribe(&self, audio: &[u8], format: AudioFormat) -> Result<String, SttError>;
    fn name(&self) -> &'static str;
}

pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u8,
    pub encoding: AudioEncoding,
}

pub enum AudioEncoding {
    Wav,
    Pcm16,
    Opus,
}

#[derive(Debug, thiserror::Error)]
pub enum SttError {
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("API error: {0}")]
    Api(String),
    #[error("Audio processing error: {0}")]
    Audio(String),
}
```

#### 2.2 OpenAI Whisper API Provider

```rust
use reqwest::multipart;

pub struct OpenAiSttProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,  // "whisper-1"
}

#[async_trait]
impl SttProvider for OpenAiSttProvider {
    async fn transcribe(&self, audio: &[u8], _format: AudioFormat) -> Result<String, SttError> {
        let part = multipart::Part::bytes(audio.to_vec())
            .file_name("audio.wav")
            .mime_str("audio/wav")?;

        let form = multipart::Form::new()
            .part("file", part)
            .text("model", self.model.clone());

        let response = self.client
            .post("https://api.openai.com/v1/audio/transcriptions")
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(SttError::Api(error_text));
        }

        let result: serde_json::Value = response.json().await?;
        Ok(result["text"].as_str().unwrap_or("").to_string())
    }

    fn name(&self) -> &'static str {
        "openai"
    }
}
```

#### 2.3 Groq Provider

```rust
pub struct GroqSttProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,  // "whisper-large-v3"
}

#[async_trait]
impl SttProvider for GroqSttProvider {
    async fn transcribe(&self, audio: &[u8], _format: AudioFormat) -> Result<String, SttError> {
        let part = multipart::Part::bytes(audio.to_vec())
            .file_name("audio.wav")
            .mime_str("audio/wav")?;

        let form = multipart::Form::new()
            .part("file", part)
            .text("model", self.model.clone());

        let response = self.client
            .post("https://api.groq.com/openai/v1/audio/transcriptions")
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(SttError::Api(error_text));
        }

        let result: serde_json::Value = response.json().await?;
        Ok(result["text"].as_str().unwrap_or("").to_string())
    }

    fn name(&self) -> &'static str {
        "groq"
    }
}
```

#### 2.4 Local Whisper (whisper-rs)

```rust
// Cargo.toml
whisper-rs = "0.11"

pub struct LocalWhisperProvider {
    ctx: whisper_rs::WhisperContext,
}

impl LocalWhisperProvider {
    pub fn new(model_path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let ctx = whisper_rs::WhisperContext::new(model_path)?;
        Ok(Self { ctx })
    }
}

#[async_trait]
impl SttProvider for LocalWhisperProvider {
    async fn transcribe(&self, audio: &[u8], format: AudioFormat) -> Result<String, SttError> {
        // Note: whisper-rs is sync, so we spawn_blocking
        let audio = audio.to_vec();
        let ctx = self.ctx.clone();  // Would need Arc<WhisperContext>

        tokio::task::spawn_blocking(move || {
            let mut state = ctx.create_state()?;
            let params = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });

            // Decode WAV to samples
            let samples = decode_wav_to_f32(&audio)?;

            state.full(params, &samples)?;

            let mut text = String::new();
            let num_segments = state.full_n_segments()?;
            for i in 0..num_segments {
                text.push_str(state.full_get_segment_text(i)?);
            }
            Ok(text)
        }).await.map_err(|e| SttError::Audio(e.to_string()))?
    }

    fn name(&self) -> &'static str {
        "local-whisper"
    }
}
```

#### 2.5 Provider Registry

```rust
use std::sync::Arc;

pub struct SttRegistry {
    providers: std::collections::HashMap<String, Arc<dyn SttProvider>>,
    current: String,
}

impl SttRegistry {
    pub fn new() -> Self {
        Self {
            providers: std::collections::HashMap::new(),
            current: String::new(),
        }
    }

    pub fn register(&mut self, name: &str, provider: Arc<dyn SttProvider>) {
        self.providers.insert(name.to_string(), provider);
    }

    pub fn set_current(&mut self, name: &str) -> Result<(), String> {
        if self.providers.contains_key(name) {
            self.current = name.to_string();
            Ok(())
        } else {
            Err(format!("Provider '{}' not found", name))
        }
    }

    pub fn get_current(&self) -> Option<Arc<dyn SttProvider>> {
        self.providers.get(&self.current).cloned()
    }
}
```

---

### Phase 3: LLM Formatting

#### 3.1 LLM Provider Trait

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, prompt: &str) -> Result<String, LlmError>;
    fn name(&self) -> &'static str;
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("API error: {0}")]
    Api(String),
}
```

#### 3.2 OpenAI LLM Provider

```rust
pub struct OpenAiLlmProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

#[async_trait]
impl LlmProvider for OpenAiLlmProvider {
    async fn complete(&self, prompt: &str) -> Result<String, LlmError> {
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "user", "content": prompt}
            ],
            "max_tokens": 4096,
        });

        let response = self.client
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            return Err(LlmError::Api(error_text));
        }

        let result: serde_json::Value = response.json().await?;
        Ok(result["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    fn name(&self) -> &'static str {
        "openai"
    }
}
```

#### 3.3 Prompt Templates (Port from Python)

```rust
pub struct PromptTemplate {
    pub name: String,
    pub system_prompt: String,
    pub user_template: String,  // Contains {transcript} placeholder
}

impl PromptTemplate {
    pub fn render(&self, transcript: &str) -> String {
        self.user_template.replace("{transcript}", transcript)
    }
}

// Default templates matching current Python implementation
pub fn default_templates() -> Vec<PromptTemplate> {
    vec![
        PromptTemplate {
            name: "default".to_string(),
            system_prompt: "You are a helpful assistant that formats transcribed speech.".to_string(),
            user_template: "Format and clean up this transcription, fixing grammar and punctuation while preserving the original meaning:\n\n{transcript}".to_string(),
        },
        PromptTemplate {
            name: "code".to_string(),
            system_prompt: "You are a code assistant.".to_string(),
            user_template: "The following is voice-dictated code or technical content. Format it appropriately:\n\n{transcript}".to_string(),
        },
    ]
}
```

---

### Phase 4: Pipeline Orchestration

> ⚠️ **Critical complexity here.** The conceptual flow is simple, but production requires solving hard problems.

#### 4.0 Pipeline Hard Problems

Before implementing, plan for these:

| Problem | What Goes Wrong | Solution |
|---------|-----------------|----------|
| **Backpressure** | STT calls take longer than audio arrives; queue grows unbounded | Bounded channel; drop oldest or pause capture |
| **Cancellation** | User stops recording; in-flight STT call keeps running | `CancellationToken` / `AbortHandle` on all async tasks |
| **Concurrency** | Audio callback on different thread than async runtime | Use channels to bridge; don't block audio thread |
| **Memory growth** | Long recording session; buffer grows to gigabytes | Cap buffer size; stream to temp file if needed |
| **Error recovery** | Provider returns 500; app wedges | Timeout + retry with backoff; surface error to user |

**Recommended approach**: Model pipeline as explicit state machine:

```rust
enum PipelineState {
    Idle,
    Recording { buffer: AudioBuffer, cancel: CancellationToken },
    Transcribing { task: JoinHandle<Result<String>>, cancel: CancellationToken },
    Formatting { transcript: String, task: JoinHandle<Result<String>> },
    Error { message: String },
}
```

#### 4.1 Recording Pipeline

```rust
use tokio::sync::mpsc;

pub enum PipelineEvent {
    RecordingStarted,
    VadSpeechStart,
    VadSpeechEnd,
    TranscriptPartial(String),
    TranscriptFinal(String),
    FormattedText(String),
    Error(String),
    RecordingStopped,
}

pub struct RecordingPipeline {
    stt_provider: Arc<dyn SttProvider>,
    llm_provider: Option<Arc<dyn LlmProvider>>,
    prompt_template: Option<PromptTemplate>,
    event_tx: mpsc::Sender<PipelineEvent>,
}

impl RecordingPipeline {
    pub async fn process_audio_chunk(&mut self, audio: AudioChunk) -> Result<(), PipelineError> {
        // 1. Resample if needed
        let resampled = resample_to_16khz(&audio.samples, audio.sample_rate);

        // 2. Run VAD
        let vad_result = self.vad.process(&resampled);

        match vad_result {
            VadEvent::SpeechEnd => {
                // 3. Transcribe accumulated audio
                let wav_bytes = self.buffer.to_wav_bytes();
                let transcript = self.stt_provider.transcribe(&wav_bytes, AudioFormat::wav()).await?;

                self.event_tx.send(PipelineEvent::TranscriptFinal(transcript.clone())).await?;

                // 4. Optional LLM formatting
                if let (Some(llm), Some(template)) = (&self.llm_provider, &self.prompt_template) {
                    let prompt = template.render(&transcript);
                    let formatted = llm.complete(&prompt).await?;
                    self.event_tx.send(PipelineEvent::FormattedText(formatted)).await?;
                }

                self.buffer.clear();
            }
            VadEvent::SpeechStart => {
                self.event_tx.send(PipelineEvent::VadSpeechStart).await?;
            }
            _ => {}
        }

        Ok(())
    }
}
```

#### 4.2 Tauri Command Integration

```rust
// In commands/recording.rs

use tauri::{AppHandle, Manager};

#[tauri::command]
pub async fn start_recording(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();

    // Create pipeline with current settings
    let settings = state.settings.lock().await;
    let pipeline = RecordingPipeline::new(
        settings.stt_provider.clone(),
        settings.llm_provider.clone(),
        settings.prompt_template.clone(),
    );

    // Start audio capture
    let (audio_tx, mut audio_rx) = tokio::sync::mpsc::channel(100);
    state.audio_capture.start(audio_tx).await?;

    // Spawn pipeline processing task
    let app_handle = app.clone();
    tokio::spawn(async move {
        while let Some(chunk) = audio_rx.recv().await {
            match pipeline.process_audio_chunk(chunk).await {
                Ok(()) => {}
                Err(e) => {
                    app_handle.emit_all("pipeline-error", e.to_string()).ok();
                }
            }
        }
    });

    // Emit events to frontend
    app.emit_all("recording-started", ()).ok();

    Ok(())
}

#[tauri::command]
pub async fn stop_recording(app: AppHandle) -> Result<String, String> {
    let state = app.state::<AppState>();
    state.audio_capture.stop().await?;

    // Get final transcript
    let transcript = state.pipeline.finalize().await?;

    app.emit_all("recording-stopped", &transcript).ok();

    Ok(transcript)
}
```

---

### Phase 5: Frontend Changes

#### 5.1 Update OverlayApp.tsx

Replace Pipecat client with Tauri event listeners:

```tsx
// BEFORE (Pipecat WebRTC)
const pipecat = useRef<PipecatClient | null>(null);

useEffect(() => {
  pipecat.current = new PipecatClient();
  pipecat.current.connect(`${serverUrl}/api/offer`);
  // ...
}, []);

// AFTER (Tauri events)
import { listen, invoke } from '@tauri-apps/api';

useEffect(() => {
  const unlisten = Promise.all([
    listen('transcript-partial', (event) => {
      setPartialTranscript(event.payload as string);
    }),
    listen('transcript-final', (event) => {
      setTranscript(event.payload as string);
    }),
    listen('formatted-text', (event) => {
      const text = event.payload as string;
      invoke('type_text', { text });
    }),
    listen('pipeline-error', (event) => {
      setError(event.payload as string);
    }),
  ]);

  return () => {
    unlisten.then(fns => fns.forEach(fn => fn()));
  };
}, []);

const startRecording = async () => {
  await invoke('start_recording');
};

const stopRecording = async () => {
  await invoke('stop_recording');
};
```

#### 5.2 Update tauri.ts

Remove HTTP-based config API, use Tauri commands:

```typescript
// BEFORE
export const configAPI = ky.create({
  prefixUrl: "http://127.0.0.1:8765",
});

// AFTER
import { invoke } from '@tauri-apps/api';

export const configAPI = {
  getProviders: () => invoke<Provider[]>('get_providers'),
  setProvider: (type: string, provider: string) =>
    invoke('set_provider', { type, provider }),
  getDefaultSections: () => invoke<Section[]>('get_default_sections'),
  // ... etc
};
```

#### 5.3 Update queries.ts

```typescript
// BEFORE
export const useProviders = () => useQuery({
  queryKey: ['providers'],
  queryFn: () => configAPI.get('api/config/providers').json(),
});

// AFTER
export const useProviders = () => useQuery({
  queryKey: ['providers'],
  queryFn: () => invoke<Provider[]>('get_providers'),
});
```

---

### Phase 6: Files to Remove

After migration is complete, remove:

```
server/                          # Entire Python server directory
├── main.py
├── pyproject.toml
├── api/
├── config/
├── processors/
├── services/
├── tests/
└── utils/

app/src/
├── lib/tauri.ts                 # Remove configAPI HTTP client parts
└── stores/recordingStore.ts     # Heavy refactor (remove Pipecat)
```

---

### Phase 7: Cargo.toml Dependencies

Add these to `app/src-tauri/Cargo.toml`:

```toml
[dependencies]
# Audio processing
cpal = "0.15"           # Already present
rubato = "0.15"         # Resampling
hound = "3.5"           # WAV encoding

# VAD
webrtc-vad = "0.4"      # Simple VAD
# OR for Silero VAD:
# ort = "2.0"           # ONNX runtime

# Local STT (optional - requires libclang to build)
whisper-rs = { version = "0.14", optional = true }
dirs = { version = "6.0", optional = true }

# HTTP client for API calls
reqwest = { version = "0.11", features = ["json", "multipart", "stream"] }

# Async
tokio = { version = "1", features = ["full"] }
async-trait = "0.1"

# Error handling
thiserror = "1.0"
anyhow = "1.0"

[features]
default = []
local-whisper = ["dep:whisper-rs", "dep:dirs"]
```

---

## Estimated Effort

| Phase | Effort | Risk | Notes |
|-------|--------|------|-------|
| Phase 1: E2E Manual Path | 3-4 days | Low | **Ship this first!** Proves architecture |
| Phase 2: Remote STT Providers | 2-3 days | Low | API calls are straightforward |
| Phase 3: Settings Migration | 1-2 days | Low | |
| Phase 4: VAD Enhancement | 3-5 days | **High** | VAD tuning is iterative; budget extra time |
| Phase 5: Pipeline Hardening | 2-3 days | Medium | State management complexity |
| Phase 6: LLM Formatting | 1-2 days | Low | |
| Phase 7: Local Whisper | 3-5 days | Medium | Platform-specific issues; model distribution |
| Phase 8: Cleanup | 1 day | Low | |
| Phase 9: Testing | 2-3 days | Medium | Cross-platform surface area |

**Total: ~3-4 weeks** for a single developer familiar with Rust and the codebase.

**Fast path to "works" (Phases 1-3 only): ~1-1.5 weeks** — delivers manual start/stop dictation with remote STT.

---

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| **VAD quality / tuning** | Chopped words, merged sentences, laggy stops | Pre-roll buffer + hangover; debug visualization; make thresholds configurable; defer VAD until manual path works |
| Resampling artifacts | Transcription quality | Defer resampling; let remote STT resample; only add for local whisper |
| Local Whisper performance | CPU usage, latency | Make local STT optional; default to remote API providers |
| Cross-platform audio issues | Platform compatibility | Test early on all target platforms; budget extra time |
| State management complexity | Bugs, race conditions | Model as explicit state machine; use Rust ownership; bounded queues |
| **Backpressure / memory growth** | OOM on long sessions | Cap buffer size; bounded channels; stream to temp file |
| **Cancellation bugs** | Zombie tasks, wedged state | Use `CancellationToken` everywhere; test rapid start/stop |
| Provider API failures | Bad UX | Timeouts, retries, backoff; surface errors to user |
| Secret management in-app | Security concerns | Use OS keychain (tauri-plugin-keyring) vs plaintext store |

---

## Technical Callouts

### webrtc-vad
- **Pros**: Simple, fast, proven
- **Cons**: Requires specific frame sizes (10/20/30ms) and PCM16; not magic in noisy environments
- **Must do**: Clear rules for frame size, consistent sample rate, buffering strategy

### WAV encoding (hound)
- Ensure correct little-endian PCM16 output
- Correct headers per chunk
- Avoid re-encoding repeatedly in hot paths (CPU cost)

### reqwest + multipart
- Set timeouts and implement retries
- Handle proxy/cert issues on corporate machines
- Implement rate-limit backoff
- Note: many "Whisper-like" endpoints aren't truly streaming

### Tauri events
- Define stable event contract early (payload types, event names)
- Consider versioning for future compatibility

---

## Alternatives Considered

### Why not Option A (Keep server, drop Pipecat)?
- Still requires managing two processes
- User pain point is "running a separate server"
- Less elegant long-term architecture

### Why not Option C (Bundle server as sidecar)?
- Simpler to implement
- But: larger bundle size (Python runtime + deps)
- Still two processes (potential resource waste)
- **Consider this as a stepping stone** if Option B timeline is too long

---

## Recommended Execution Order

The analysis above suggests a risk-reducing sequence that ships value earlier:

### Sprint 1: Prove End-to-End (1-1.5 weeks)
1. **Audio capture → manual stop → WAV blob → remote STT → type text**
2. No VAD, no resampling, no local whisper
3. This validates: capture, encoding, provider calls, events, typing
4. **Deliverable**: Working manual push-to-talk dictation

### Sprint 2: Polish & Settings (1 week)
1. Add remaining STT providers
2. Migrate settings to Tauri commands
3. Timeouts/retries/error handling
4. **Deliverable**: Feature-complete remote STT, no Python server

### Sprint 3: VAD Enhancement (1 week, optional)
1. Add VAD with pre-roll + hangover
2. Debug visualization
3. Tune thresholds
4. **Deliverable**: Auto-stop on silence

### Sprint 4: Advanced Features (ongoing, optional)
1. LLM formatting
2. Local whisper
3. Continuous dictation / partial transcripts

---

## Success Criteria

1. ✅ App launches as single process (no Python server needed)
2. ✅ Hotkey triggers recording → transcription → text typing
3. ✅ All current STT providers work (OpenAI, Groq, Deepgram)
4. ✅ LLM formatting works with current providers
5. ✅ Settings persist and are configurable in UI
6. ✅ Latency comparable to current implementation
7. ✅ Works on Windows, macOS, Linux
8. ✅ Bundle size reasonable (< 50MB without local Whisper model)
