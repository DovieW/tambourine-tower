use rodio::buffer::SamplesBuffer;
use rodio::{Decoder, OutputStreamBuilder, Source};
use std::io::Cursor;
use std::thread;
use std::time::Duration;

/// Types of sounds that can be played
#[derive(Debug, Clone, Copy)]
pub enum SoundType {
    RecordingStart,
    RecordingStop,
}

/// User-selectable sound cue theme.
///
/// Note: `Tambourine` intentionally preserves the legacy MP3 files so existing users
/// can keep the current sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCue {
    Tangerine,
    Maraca,
    Clave,
    Tambourine,
}

impl AudioCue {
    pub fn from_str(s: &str) -> Self {
        match s {
            "tangerine" => Self::Tangerine,
            "maraca" => Self::Maraca,
            "clave" => Self::Clave,
            "tambourine" => Self::Tambourine,
            // Unknown values: default to Tangerine.
            _ => Self::Tangerine,
        }
    }
}

// Embed audio files at compile time
const START_SOUND: &[u8] = include_bytes!("assets/start.mp3");
const STOP_SOUND: &[u8] = include_bytes!("assets/stop.mp3");

/// Best-effort estimate of how long a cue will be audible.
///
/// Used to avoid cutting off cues when we do side-effects (like system mute) shortly after
/// starting playback.
pub fn estimated_duration(sound_type: SoundType, cue: AudioCue) -> Duration {
    match cue {
        // For the legacy MP3 cue, use the decoder's total duration when available.
        // If unavailable, fall back to a conservative default.
        AudioCue::Tambourine => {
            let sound_data = match sound_type {
                SoundType::RecordingStart => START_SOUND,
                SoundType::RecordingStop => STOP_SOUND,
            };

            Decoder::new(Cursor::new(sound_data))
                .ok()
                .and_then(|d| d.total_duration())
                .unwrap_or(Duration::from_millis(500))
        }

        // Synth cues: keep in sync with durations in `build_synth_cue_source`.
        AudioCue::Tangerine => match sound_type {
            SoundType::RecordingStart => Duration::from_millis(255),
            SoundType::RecordingStop => Duration::from_millis(195),
        },
        AudioCue::Maraca => match sound_type {
            SoundType::RecordingStart => Duration::from_millis(45 + 30 + 45 + 30 + 60),
            SoundType::RecordingStop => Duration::from_millis(55 + 35 + 45),
        },
        AudioCue::Clave => match sound_type {
            SoundType::RecordingStart => Duration::from_millis(55 + 35 + 45),
            SoundType::RecordingStop => Duration::from_millis(80),
        },
    }
}

/// Play a sound effect (non-blocking)
pub fn play_sound(sound_type: SoundType, cue: AudioCue) {
    thread::spawn(move || {
        if let Err(e) = play_sound_blocking(sound_type, cue) {
            log::warn!("Failed to play sound: {}", e);
        }
    });
}

pub(crate) fn play_sound_blocking(
    sound_type: SoundType,
    cue: AudioCue,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let stream = OutputStreamBuilder::open_default_stream()?;

    // Some devices/backends take a moment to "wake" after being idle.
    // Since dropping `stream` stops playback, keep extra tail padding so we don't
    // clip the end of a cue (most noticeable on the first playback after idle).
    const TAIL_PAD: Duration = Duration::from_millis(250);

    match cue {
        // Preserve the existing cue exactly (legacy MP3 assets).
        AudioCue::Tambourine => {
            let sound_data = match sound_type {
                SoundType::RecordingStart => START_SOUND,
                SoundType::RecordingStop => STOP_SOUND,
            };
            let cursor = Cursor::new(sound_data);
            let decoded = Decoder::new(cursor)?.amplify(0.3);

            let duration = decoded
                .total_duration()
                .unwrap_or(Duration::from_millis(500));

            stream.mixer().add(decoded);
            thread::sleep(duration + TAIL_PAD);
        }

        // New cues are synthesized at runtime (no extra audio assets needed).
        _ => {
            let (seq, duration) = build_synth_cue_source(sound_type, cue);
            stream.mixer().add(seq);
            thread::sleep(duration + TAIL_PAD);
        }
    }

    Ok(())
}

fn build_synth_cue_source(sound_type: SoundType, cue: AudioCue) -> (SamplesBuffer, Duration) {
    const SAMPLE_RATE: u32 = 44_100;
    const CHANNELS: u16 = 1;

    fn frames_for(d: Duration) -> usize {
        (d.as_secs_f32() * SAMPLE_RATE as f32).round() as usize
    }

    fn push_silence(samples: &mut Vec<f32>, d: Duration) {
        let n = frames_for(d);
        samples.extend(std::iter::repeat_n(0.0, n));
    }

    fn soft_clip(x: f32) -> f32 {
        // Gentle saturation to avoid harsh digital clipping.
        // tanh is a bit expensive, but cue playback is short.
        x.tanh()
    }

    fn push_chime(samples: &mut Vec<f32>, freq_hz: f32, d: Duration, amp: f32) {
        use std::f32::consts::PI;

        let n = frames_for(d);
        if n == 0 {
            return;
        }

        // Fast attack, exponential decay.
        let attack = ((SAMPLE_RATE as f32) * 0.004).round() as usize;
        let attack = attack.min(n).max(1);
        let decay_k = 6.0_f32; // larger = faster decay

        // Slight detune + a couple harmonics for a bell-ish tone.
        let detune = 0.0045;

        for i in 0..n {
            let t = i as f32 / SAMPLE_RATE as f32;
            let env = (-decay_k * t / d.as_secs_f32().max(0.001)).exp();
            let atk = if i < attack {
                i as f32 / attack as f32
            } else {
                1.0
            };

            let base = (2.0 * PI * (freq_hz * (1.0 + detune)) * t).sin();
            let h2 = (2.0 * PI * (freq_hz * 2.01) * t).sin() * 0.35;
            let h3 = (2.0 * PI * (freq_hz * 3.00) * t).sin() * 0.18;

            let v = (base + h2 + h3) * amp * env * atk;
            samples.push(soft_clip(v));
        }
    }

    fn push_woodblock(samples: &mut Vec<f32>, freq_hz: f32, d: Duration, amp: f32, seed: &mut u32) {
        use std::f32::consts::PI;

        let n = frames_for(d);
        if n == 0 {
            return;
        }

        // Short, percussive envelope.
        let attack = ((SAMPLE_RATE as f32) * 0.0015).round() as usize;
        let attack = attack.min(n).max(1);
        let decay_k = 10.0_f32;

        // Simple differentiated noise for a "click" component.
        let mut prev_noise = 0.0_f32;

        for i in 0..n {
            let t = i as f32 / SAMPLE_RATE as f32;
            let env = (-decay_k * t / d.as_secs_f32().max(0.001)).exp();
            let atk = if i < attack {
                i as f32 / attack as f32
            } else {
                1.0
            };

            // xorshift32
            *seed ^= *seed << 13;
            *seed ^= *seed >> 17;
            *seed ^= *seed << 5;
            let r = (*seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            let click = (r - prev_noise) * 0.45;
            prev_noise = r;

            let tone = (2.0 * PI * freq_hz * t).sin() * 0.9
                + (2.0 * PI * (freq_hz * 2.6) * t).sin() * 0.25;

            let v = (tone * 0.75 + click) * amp * env * atk;
            samples.push(soft_clip(v));
        }
    }

    fn push_shaker(samples: &mut Vec<f32>, d: Duration, amp: f32, seed: &mut u32) {
        let n = frames_for(d);
        if n == 0 {
            return;
        }

        // Very fast attack + fast decay to feel like a maraca/shaker.
        let attack = ((SAMPLE_RATE as f32) * 0.001).round() as usize;
        let attack = attack.min(n).max(1);
        let decay_k = 14.0_f32;

        // High-pass-ish by differentiating noise.
        let mut prev = 0.0_f32;

        for i in 0..n {
            let t = i as f32 / SAMPLE_RATE as f32;
            let env = (-decay_k * t / d.as_secs_f32().max(0.001)).exp();
            let atk = if i < attack {
                i as f32 / attack as f32
            } else {
                1.0
            };

            *seed ^= *seed << 13;
            *seed ^= *seed >> 17;
            *seed ^= *seed << 5;
            let r = (*seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            let hp = r - prev;
            prev = r;

            let v = hp * amp * env * atk;
            samples.push(soft_clip(v));
        }
    }

    // Build the cue explicitly per type so we can use more realistic synthesis.
    let mut samples: Vec<f32> = Vec::new();
    let mut duration = Duration::from_millis(0);
    let mut seed: u32 = 0xA1B2_C3D4;

    match cue {
        AudioCue::Tangerine => {
            // Friendly chime: short arpeggio up (start) / down (stop).
            // Uses additive harmonics + decay instead of flat sine notes.
            match sound_type {
                SoundType::RecordingStart => {
                    // Keep this cue snappy; it should be informative, not a jingle.
                    let d1 = Duration::from_millis(70);
                    let gap = Duration::from_millis(20);
                    let d2 = Duration::from_millis(80);
                    let d3 = Duration::from_millis(65);

                    push_chime(&mut samples, 523.25, d1, 0.20); // C5
                    push_silence(&mut samples, gap);
                    push_chime(&mut samples, 659.25, d2, 0.19); // E5
                    push_silence(&mut samples, gap);
                    push_chime(&mut samples, 783.99, d3, 0.17); // G5

                    duration = d1 + gap + d2 + gap + d3;
                }
                SoundType::RecordingStop => {
                    let d1 = Duration::from_millis(80);
                    let gap = Duration::from_millis(20);
                    let d2 = Duration::from_millis(95);

                    push_chime(&mut samples, 659.25, d1, 0.18); // E5
                    push_silence(&mut samples, gap);
                    push_chime(&mut samples, 523.25, d2, 0.18); // C5

                    duration = d1 + gap + d2;
                }
            }
        }

        AudioCue::Maraca => {
            // Percussive shaker: quick bursts of filtered noise.
            match sound_type {
                SoundType::RecordingStart => {
                    let tick = Duration::from_millis(45);
                    let gap = Duration::from_millis(30);
                    let tick2 = Duration::from_millis(60);

                    push_shaker(&mut samples, tick, 0.32, &mut seed);
                    push_silence(&mut samples, gap);
                    push_shaker(&mut samples, tick, 0.30, &mut seed);
                    push_silence(&mut samples, gap);
                    push_shaker(&mut samples, tick2, 0.28, &mut seed);

                    duration = tick + gap + tick + gap + tick2;
                }
                SoundType::RecordingStop => {
                    let tick = Duration::from_millis(55);
                    let gap = Duration::from_millis(35);
                    push_shaker(&mut samples, tick, 0.30, &mut seed);
                    push_silence(&mut samples, gap);
                    push_shaker(&mut samples, Duration::from_millis(45), 0.24, &mut seed);
                    duration = tick + gap + Duration::from_millis(45);
                }
            }
        }

        AudioCue::Clave => {
            // Woodblock / claves feel: two short taps (start) and one firmer tap (stop).
            match sound_type {
                SoundType::RecordingStart => {
                    let tap = Duration::from_millis(55);
                    let gap = Duration::from_millis(35);
                    push_woodblock(&mut samples, 1750.0, tap, 0.38, &mut seed);
                    push_silence(&mut samples, gap);
                    push_woodblock(&mut samples, 2100.0, Duration::from_millis(45), 0.32, &mut seed);
                    duration = tap + gap + Duration::from_millis(45);
                }
                SoundType::RecordingStop => {
                    let tap = Duration::from_millis(80);
                    push_woodblock(&mut samples, 1550.0, tap, 0.36, &mut seed);
                    duration = tap;
                }
            }
        }

        // Should never hit: Tambourine handled in play_sound_blocking.
        // If it does, keep duration at the default 0.
        AudioCue::Tambourine => {}
    }

    let seq = SamplesBuffer::new(CHANNELS, SAMPLE_RATE, samples);
    (seq, duration)
}
