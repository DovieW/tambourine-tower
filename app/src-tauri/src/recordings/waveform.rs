//! Bounded, local waveform analysis. Only PCM samples stream through memory;
//! the webview receives at most 4096 min/max pairs, never decoded meeting audio.
use super::RecordingStore;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek};

const BINS: usize = 4096;
const VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RecordingWaveform {
    pub duration_seconds: f64,
    /// Interleaved minimum and maximum amplitude, normalized to [-1, 1].
    pub peaks: Vec<f32>,
}

#[derive(Serialize, Deserialize)]
struct CachedWaveform {
    version: u8,
    bytes: u64,
    modified_nanos: u128,
    waveform: RecordingWaveform,
}

impl RecordingStore {
    fn waveform_path(&self, id: &str) -> std::path::PathBuf {
        self.dir.join(format!("{id}.waveform.json"))
    }

    pub(super) fn remove_waveform(&self, id: &str) -> Result<(), String> {
        let path = self.waveform_path(id);
        if self.fs.exists(&path) {
            self.fs
                .remove_file(&path)
                .map_err(|_| "Could not remove audio waveform")?;
        }
        Ok(())
    }

    /// Run on a blocking worker. Serialize against save/deletion so a completed
    /// analysis cannot recreate private metadata after the recording was deleted.
    pub fn waveform(&self, id: &str) -> Result<Option<RecordingWaveform>, String> {
        let _guard = self
            .media_write
            .lock()
            .map_err(|_| "Recording store unavailable")?;
        let Some(path) = self.wav_path_if_exists(id)? else {
            return Ok(None);
        };
        let path = path
            .canonicalize()
            .map_err(|_| "Could not locate recording")?;
        let directory = self
            .dir
            .canonicalize()
            .map_err(|_| "Could not locate recordings")?;
        if path.parent() != Some(directory.as_path()) {
            return Err("Recording is outside its storage directory".into());
        }
        let metadata = std::fs::metadata(&path).map_err(|_| "Could not read audio metadata")?;
        let modified_nanos = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        let cache_path = self.waveform_path(id);
        if self
            .fs
            .metadata(&cache_path)
            .is_ok_and(|m| m.len() <= 256 * 1024)
        {
            if let Ok(cache) = self.fs.read(&cache_path) {
                if let Ok(cache) = serde_json::from_slice::<CachedWaveform>(&cache) {
                    if cache.version == VERSION
                        && cache.bytes == metadata.len()
                        && cache.modified_nanos == modified_nanos
                        && !cache.waveform.peaks.is_empty()
                        && cache.waveform.peaks.len() <= BINS * 2
                        && cache.waveform.duration_seconds.is_finite()
                        && cache.waveform.duration_seconds > 0.0
                        && cache
                            .waveform
                            .peaks
                            .iter()
                            .all(|p| p.is_finite() && p.abs() <= 1.0)
                    {
                        return Ok(Some(cache.waveform));
                    }
                }
            }
        }
        let reader = hound::WavReader::open(path).map_err(|_| "This recording cannot be played")?;
        let waveform = analyze(reader)?;
        let cache = CachedWaveform {
            version: VERSION,
            bytes: metadata.len(),
            modified_nanos,
            waveform: waveform.clone(),
        };
        // A cache failure must not prevent playback. A partial file is regenerated.
        if let Ok(bytes) = serde_json::to_vec(&cache) {
            let _ = self.fs.write_private(&cache_path, &bytes);
        }
        Ok(Some(waveform))
    }
}

fn analyze<R: Read + Seek>(mut reader: hound::WavReader<R>) -> Result<RecordingWaveform, String> {
    let spec = reader.spec();
    if spec.channels == 0 || spec.sample_rate == 0 {
        return Err("Invalid audio format".into());
    }
    let samples = reader.len() as usize;
    let frames = samples / spec.channels as usize;
    if frames == 0 {
        return Err("This recording contains no audio".into());
    }
    let count = frames.clamp(1, BINS);
    let mut peaks = vec![0.0_f32; count * 2];
    let mut add = |index: usize, sample: f32| {
        let frame = index / spec.channels as usize;
        let bin = ((frame as u64 * count as u64) / frames.max(1) as u64) as usize;
        let bin = bin.min(count - 1) * 2;
        let sample = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        peaks[bin] = peaks[bin].min(sample);
        peaks[bin + 1] = peaks[bin + 1].max(sample);
    };
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for (index, sample) in reader.samples::<f32>().enumerate() {
                add(index, sample.map_err(|_| "Audio is incomplete or damaged")?);
            }
        }
        hound::SampleFormat::Int => {
            let scale = 2_f32.powi(spec.bits_per_sample as i32 - 1);
            for (index, sample) in reader.samples::<i32>().enumerate() {
                add(
                    index,
                    sample.map_err(|_| "Audio is incomplete or damaged")? as f32 / scale,
                );
            }
        }
    }
    Ok(RecordingWaveform {
        duration_seconds: frames as f64 / spec.sample_rate as f64,
        peaks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn empty_audio_has_a_useful_error_instead_of_invalid_peaks() {
        let mut wav = Cursor::new(Vec::new());
        hound::WavWriter::new(
            &mut wav,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap()
        .finalize()
        .unwrap();
        wav.set_position(0);
        assert_eq!(
            analyze(hound::WavReader::new(wav).unwrap()).unwrap_err(),
            "This recording contains no audio"
        );
    }

    #[test]
    fn bounded_peaks_and_duration_are_independent_of_channels() {
        let mut wav = Cursor::new(Vec::new());
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut writer = hound::WavWriter::new(&mut wav, spec).unwrap();
            for _ in 0..160_000 {
                writer.write_sample(-16384_i16).unwrap();
                writer.write_sample(16384_i16).unwrap();
            }
            writer.finalize().unwrap();
        }
        wav.set_position(0);
        let result = analyze(hound::WavReader::new(wav).unwrap()).unwrap();
        assert_eq!(result.duration_seconds, 10.0);
        assert_eq!(result.peaks.len(), BINS * 2);
        assert!(result
            .peaks
            .iter()
            .enumerate()
            .all(|(i, p)| *p == if i % 2 == 0 { -0.5 } else { 0.5 }));
    }

    #[test]
    fn cache_is_regenerated_and_deleted_with_audio() {
        let temp = tempfile::tempdir().unwrap();
        let store = RecordingStore::new(temp.path().to_owned());
        let mut wav = Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(
            &mut wav,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        writer.write_sample(42_i16).unwrap();
        writer.finalize().unwrap();
        store.save_wav("test", wav.get_ref()).unwrap();
        assert!(store.waveform("../test").is_err());
        assert!(store.waveform("missing").unwrap().is_none());
        store.waveform("test").unwrap().unwrap();
        let cache = store.waveform_path("test");
        std::fs::write(&cache, b"broken").unwrap();
        assert_eq!(store.waveform("test").unwrap().unwrap().peaks.len(), 2);
        assert!(store.delete_wav_if_exists("test").unwrap());
        assert!(!cache.exists());
    }
}
