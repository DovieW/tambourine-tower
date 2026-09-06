//! Append-only recovery audio. The capture worker writes here, never the audio callback.
//! Files contain a fixed header followed by little-endian float samples. A crash can
//! leave a partial final frame; readers truncate that frame rather than guessing.
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"KOLPCM01";
const MAX_SECONDS: u64 = 4 * 60 * 60;
const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Delete one owned recovery bundle. Keep audio visible when sensitive progress
/// cannot be removed, and retain its mode until the audio has been removed.
/// Missing files are harmless so interrupted cleanup can be retried.
pub fn discard(path: &Path) -> io::Result<()> {
    for file in [
        path.with_extension("transcripts"),
        path.with_extension("progress"),
        path.to_path_buf(),
        path.with_extension("options.json"),
    ] {
        match std::fs::remove_file(file) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err(io::Error::other("Could not remove saved recording files")),
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct Journal {
    file: File,
    rate: u32,
    channels: u16,
    bytes: u64,
    unsynced: u64,
    failed: bool,
}

impl Journal {
    pub fn create(path: &Path, rate: u32, channels: u16) -> io::Result<Self> {
        if rate == 0 || rate > 192_000 || channels == 0 || channels > 2 {
            return Err(io::Error::other("Unsupported recovery audio format"));
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        file.write_all(MAGIC)?;
        file.write_all(&rate.to_le_bytes())?;
        file.write_all(&channels.to_le_bytes())?;
        file.sync_all()?;
        Ok(Self {
            file,
            rate,
            channels,
            bytes: 0,
            unsynced: 0,
            failed: false,
        })
    }

    pub fn append(&mut self, samples: &[f32], rate: u32, channels: u16) -> io::Result<()> {
        if self.failed || rate != self.rate || channels != self.channels {
            self.failed = true;
            return Err(io::Error::other(
                "Recovery audio format changed or storage failed",
            ));
        }
        let count = samples.len() as u64 * 4;
        if self.bytes.saturating_add(count)
            > (MAX_SECONDS * self.rate as u64 * self.channels as u64 * 4).min(MAX_BYTES)
        {
            self.failed = true;
            return Err(io::Error::other("Recovery recording size limit reached"));
        }
        let bytes: Vec<u8> = samples
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let result = self.file.write_all(&bytes).and_then(|()| {
            self.bytes += count;
            self.unsynced += count;
            if self.unsynced >= self.rate as u64 * self.channels as u64 * 4 {
                self.file.sync_data()?;
                self.unsynced = 0;
            }
            Ok(())
        });
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    pub fn finish(&self) -> io::Result<()> {
        if self.failed {
            return Err(io::Error::other(
                "Recovery storage failed; retained audio may be incomplete",
            ));
        }
        self.file.sync_all()
    }
}

/// Prepare one final, mono 16-kHz WAV without loading the multi-gigabyte raw
/// journal into memory. Blocks here are encoding only, never provider requests.
/// The normalized result is bounded by the four-hour capture limit (~440 MiB).
/// Failure or cancellation leaves the source untouched.
pub fn final_wav(
    path: &Path,
    max_bytes: usize,
    cancelled: impl Fn() -> bool,
) -> io::Result<Vec<u8>> {
    let (rate, channels, _) = read_chunk(path, 0, 1)?;
    let bytes = std::fs::metadata(path)?.len().saturating_sub(14);
    let total_frames = bytes / (channels as u64 * 4);
    if bytes > MAX_BYTES || total_frames > MAX_SECONDS * rate as u64 {
        return Err(io::Error::other(
            "Recording exceeds the supported size; audio is retained",
        ));
    }
    if total_frames == 0 {
        return Err(io::Error::other("No recorded audio"));
    }
    let estimated_bytes = (total_frames * 16000).div_ceil(rate as u64) * 2 + 44;
    if max_bytes > 0 && estimated_bytes > max_bytes as u64 {
        return Err(io::Error::other(
            "This recording exceeds the current transcription size limit. Your full audio is saved.",
        ));
    }
    let mut output = std::io::Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(
        &mut output,
        hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .map_err(io::Error::other)?;
    let mut start = 0;
    while start < total_frames {
        if cancelled() {
            return Err(io::Error::other(
                "Transcription cancelled. Your audio is saved.",
            ));
        }
        let (_, _, samples) = read_chunk(path, start, rate * 60)?;
        if samples.is_empty() {
            return Err(io::Error::other(
                "Audio preparation incomplete; source is retained",
            ));
        }
        start += samples.len() as u64 / channels as u64;
        let mut buffer = super::AudioBuffer::new(rate, channels, 60.0);
        buffer.append(&samples);
        let (wav, _) = buffer
            .to_wav_bytes_with_config(super::AudioEncodeConfig {
                resample_to_16khz: true,
                highpass_enabled: false,
                ..Default::default()
            })
            .map_err(io::Error::other)?;
        let mut reader =
            hound::WavReader::new(std::io::Cursor::new(wav)).map_err(io::Error::other)?;
        for sample in reader.samples::<i16>() {
            writer
                .write_sample(sample.map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
        }
    }
    writer.finalize().map_err(io::Error::other)?;
    Ok(output.into_inner())
}

pub fn read_chunk(path: &Path, start_frame: u64, frames: u32) -> io::Result<(u32, u16, Vec<f32>)> {
    use std::io::{Seek, SeekFrom};
    let mut file = File::open(path)?;
    let mut header = [0u8; 14];
    file.read_exact(&mut header)?;
    let rate = u32::from_le_bytes(header[8..12].try_into().unwrap());
    let channels = u16::from_le_bytes(header[12..14].try_into().unwrap());
    if &header[..8] != MAGIC
        || rate == 0
        || rate > 192_000
        || !(1..=2).contains(&channels)
        || frames > rate * 60
    {
        return Err(io::Error::other(
            "Invalid recovery audio header or chunk size",
        ));
    }
    let offset = start_frame
        .checked_mul(channels as u64 * 4)
        .and_then(|x| x.checked_add(14))
        .ok_or_else(|| io::Error::other("Invalid recovery offset"))?;
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(frames as u64 * channels as u64 * 4)
        .read_to_end(&mut bytes)?;
    bytes.truncate(bytes.len() / (channels as usize * 4) * channels as usize * 4);
    let samples = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    Ok((rate, channels, samples))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discard_removes_only_owned_sidecars_and_preserves_audio_if_progress_cleanup_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recording.pcm");
        std::fs::write(&path, b"audio").unwrap();
        std::fs::write(path.with_extension("options.json"), b"options").unwrap();
        std::fs::create_dir(path.with_extension("transcripts")).unwrap();
        let unrelated = dir.path().join("another.pcm");
        std::fs::write(&unrelated, b"other audio").unwrap();
        assert!(discard(&path).is_err());
        assert!(path.exists());
        assert!(path.with_extension("options.json").exists());
        std::fs::remove_dir(path.with_extension("transcripts")).unwrap();
        std::fs::write(path.with_extension("transcripts"), b"progress").unwrap();
        discard(&path).unwrap();
        discard(&path).unwrap();
        assert!(!path.exists());
        assert!(!path.with_extension("transcripts").exists());
        assert!(!path.with_extension("options.json").exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn final_transcription_contains_the_entire_recording_and_retains_source() {
        let path = std::env::temp_dir().join(format!("kolboo-final-{}.pcm", uuid::Uuid::new_v4()));
        let mut journal = Journal::create(&path, 16000, 1).unwrap();
        journal.append(&vec![0.25; 16000 * 61], 16000, 1).unwrap();
        journal.finish().unwrap();
        drop(journal);
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&[1, 2])
            .unwrap();
        let source_length = std::fs::metadata(&path).unwrap().len();
        let wav = final_wav(&path, 0, || false).unwrap();
        let mut reader = hound::WavReader::new(std::io::Cursor::new(wav)).unwrap();
        assert_eq!(reader.duration(), 16000 * 61);
        assert!(reader.samples::<i16>().all(|sample| sample.unwrap() > 8000));
        assert!(final_wav(&path, 0, || true).is_err());
        assert!(final_wav(&path, 1024, || false).is_err());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), source_length);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn duration_limit_is_independent_of_sample_rate() {
        let path = std::env::temp_dir().join(format!("kolboo-limit-{}.pcm", uuid::Uuid::new_v4()));
        let mut journal = Journal::create(&path, 16000, 1).unwrap();
        journal.bytes = MAX_SECONDS * 16000 * 4;
        assert!(journal.append(&[0.0], 16000, 1).is_err());
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn preserves_samples_and_ignores_a_partial_crash_frame() {
        let path =
            std::env::temp_dir().join(format!("kolboo-journal-{}.pcm", uuid::Uuid::new_v4()));
        let mut journal = Journal::create(&path, 16000, 1).unwrap();
        journal.append(&[0.25, -0.5], 16000, 1).unwrap();
        journal.finish().unwrap();
        drop(journal);
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&[1, 2])
            .unwrap();
        assert_eq!(read_chunk(&path, 0, 100).unwrap().2, vec![0.25, -0.5]);
        assert!(Journal::create(&path, 16000, 1).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn format_change_fails_closed() {
        let path =
            std::env::temp_dir().join(format!("kolboo-journal-{}.pcm", uuid::Uuid::new_v4()));
        let mut journal = Journal::create(&path, 16000, 1).unwrap();
        assert!(journal.append(&[0.0], 48000, 1).is_err());
        assert!(journal.finish().is_err());
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }
}
