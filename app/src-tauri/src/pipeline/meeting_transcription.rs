//! Post-recording uploads and durable partial transcripts. No History entries or
//! rewriting here: callers receive one assembled transcript for the normal flow.
use super::{stt_flow::SttResult, PipelineError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
};
use tokio_util::sync::CancellationToken;

pub(super) const MAX_MEETING_WAV_BYTES: usize = 16000 * 2 * 4 * 60 * 60 + 44;
const UPLOAD_FRAMES: usize = 16000 * 10 * 60; // 19.2 MB, below the managed 25 MB body limit.
const MAX_CHECKPOINT_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct CompletedUpload {
    key: String,
    text: String,
    duration_ms: u64,
    #[serde(default)]
    segments: Vec<crate::stt::SpeakerSegment>,
}

fn storage_error(_: impl std::fmt::Display) -> PipelineError {
    PipelineError::Config(
        "Meeting progress could not be saved or read. Your audio is retained.".into(),
    )
}

pub(super) async fn transcribe<F, Fut>(
    wav: &[u8],
    path: Option<&Path>,
    settings_key: &str,
    cancel: &CancellationToken,
    upload: F,
) -> Result<SttResult, PipelineError>
where
    F: FnMut(Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = Result<SttResult, PipelineError>>,
{
    transcribe_blocks(wav, path, settings_key, cancel, UPLOAD_FRAMES, upload).await
}

async fn transcribe_blocks<F, Fut>(
    wav: &[u8],
    path: Option<&Path>,
    settings_key: &str,
    cancel: &CancellationToken,
    block_frames: usize,
    mut upload: F,
) -> Result<SttResult, PipelineError>
where
    F: FnMut(Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = Result<SttResult, PipelineError>>,
{
    let mut reader = hound::WavReader::new(std::io::Cursor::new(wav)).map_err(storage_error)?;
    let spec = reader.spec();
    if spec.channels != 1
        || spec.sample_rate != 16000
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
        || wav.len() > MAX_MEETING_WAV_BYTES
    {
        return Err(PipelineError::Config(
            "Unsupported meeting audio format".into(),
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = path
        .map(|path| options.open(path))
        .transpose()
        .map_err(storage_error)?;
    let mut bytes = Vec::new();
    if let Some(file) = &mut file {
        if file.metadata().map_err(storage_error)?.len() > MAX_CHECKPOINT_BYTES {
            return Err(storage_error("progress too large"));
        }
        file.read_to_end(&mut bytes).map_err(storage_error)?;
    }
    // A crash during append may leave only the last JSON line incomplete.
    let valid_length = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let mut completed = HashMap::new();
    for line in bytes[..valid_length]
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
    {
        let entry: CompletedUpload = serde_json::from_slice(line).map_err(storage_error)?;
        completed.insert(entry.key.clone(), entry);
    }
    use std::io::{Seek, SeekFrom};
    if let Some(file) = &mut file {
        file.set_len(valid_length as u64).map_err(storage_error)?;
        file.seek(SeekFrom::End(0)).map_err(storage_error)?;
    }
    let source_key = Sha256::digest(wav);
    let mut output = SttResult {
        segments: Vec::new(),
        text: String::new(),
        duration_ms: 0,
        retry: Default::default(),
    };
    let mut start = 0;
    let mut part = 1;
    while start < reader.duration() {
        if cancel.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        let mut samples = reader
            .samples::<i16>()
            .take(block_frames)
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        // Prefer a quiet boundary near the end, without gaps or overlapping text.
        if samples.len() == block_frames && start + (samples.len() as u32) < reader.duration() {
            let window = 4800;
            if samples.len() > 160000 + window {
                let begin = samples.len() - 160000;
                let boundary = (begin..samples.len() - window)
                    .step_by(window)
                    .min_by_key(|offset| {
                        samples[*offset..*offset + window]
                            .iter()
                            .map(|value| (*value as i64).pow(2))
                            .sum::<i64>()
                    })
                    .unwrap_or(samples.len());
                samples.truncate(boundary);
            }
        }
        if samples.is_empty() {
            return Err(storage_error("empty block"));
        }
        let end = start + samples.len() as u32;
        let key = format!(
            "{:x}",
            Sha256::digest(format!("v1:{source_key:x}:{settings_key}:{start}:{end}"))
        );
        let entry = if let Some(entry) = completed.remove(&key) {
            entry
        } else {
            let mut buffer = std::io::Cursor::new(Vec::new());
            let mut writer = hound::WavWriter::new(&mut buffer, spec).map_err(storage_error)?;
            for sample in samples {
                writer.write_sample(sample).map_err(storage_error)?;
            }
            writer.finalize().map_err(storage_error)?;
            let result = upload(buffer.into_inner()).await?;
            output.retry.attempts += result.retry.attempts;
            output.retry.retries += result.retry.retries;
            output.retry.total_delay_ms += result.retry.total_delay_ms;
            output.retry.last_error = result.retry.last_error;
            let entry = CompletedUpload {
                segments: result.segments,
                key,
                text: result.text,
                duration_ms: result.duration_ms,
            };
            let mut line = serde_json::to_vec(&entry).map_err(storage_error)?;
            line.push(b'\n');
            if let Some(file) = &mut file {
                if file.metadata().map_err(storage_error)?.len() + line.len() as u64
                    > MAX_CHECKPOINT_BYTES
                {
                    return Err(storage_error("progress too large"));
                }
                file.write_all(&line).map_err(storage_error)?;
                file.sync_all().map_err(storage_error)?;
            }
            entry
        };
        if !entry.text.trim().is_empty() {
            if !output.text.is_empty() {
                output.text.push('\n');
            }
            output.text.push_str(entry.text.trim());
        }
        output.duration_ms += entry.duration_ms;
        output
            .segments
            .extend(entry.segments.into_iter().map(|mut segment| {
                segment.part = part;
                segment.start_seconds += start as f64 / 16000.0;
                segment.end_seconds += start as f64 / 16000.0;
                segment
            }));
        part += 1;
        start = end;
        reader.seek(start).map_err(storage_error)?;
    }
    if cancel.is_cancelled() {
        return Err(PipelineError::Cancelled);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn audio(frames: usize) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(
            &mut cursor,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..frames {
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
        cursor.into_inner()
    }
    fn result(text: &str) -> SttResult {
        SttResult {
            segments: Vec::new(),
            text: text.into(),
            duration_ms: 1,
            retry: Default::default(),
        }
    }
    fn path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "kolboo-meeting-{}.transcripts",
            uuid::Uuid::new_v4()
        ))
    }

    #[tokio::test]
    async fn diarized_meeting_checkpoints_preserve_request_local_speakers() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("checkpoint");
        let wav = audio(32000);
        let token = CancellationToken::new();
        let original = transcribe_blocks(&wav, Some(&path), "diarize", &token, 16000, |_| async {
            let mut result = result("Hello");
            result.segments.push(crate::stt::SpeakerSegment {
                speaker: "A".into(),
                text: "Hello".into(),
                start_seconds: 0.0,
                end_seconds: 0.5,
                part: 1,
            });
            Ok(result)
        })
        .await
        .unwrap();
        let resumed = transcribe_blocks(&wav, Some(&path), "diarize", &token, 16000, |_| async {
            panic!("completed uploads must not repeat")
        })
        .await
        .unwrap();
        assert_eq!(resumed.segments.len(), 2);
        assert_eq!(resumed.segments[0].part, 1);
        assert_eq!(resumed.segments[1].part, 2);
        assert_eq!(resumed.segments[1].start_seconds, 1.0);
        assert_eq!(
            serde_json::to_value(&original.segments).unwrap(),
            serde_json::to_value(&resumed.segments).unwrap()
        );
        let document = crate::stt::speaker_document(&resumed.text, &resumed.segments);
        assert!(document.contains("Part 2"));
        assert_eq!(document.matches("Speaker A").count(), 2);
    }

    #[tokio::test]
    async fn failed_upload_resumes_and_ignores_partial_crash_checkpoint() {
        let path = path();
        let wav = audio(48000);
        let cancel = CancellationToken::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let failed = transcribe_blocks(&wav, Some(&path), "model-a", &cancel, 16000, move |_| {
            let index = count.fetch_add(1, Ordering::SeqCst);
            async move {
                if index == 1 {
                    Err(PipelineError::Cancelled)
                } else {
                    Ok(result("first"))
                }
            }
        })
        .await;
        assert!(failed.is_err());
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{partial")
            .unwrap();
        let count = calls.clone();
        let resumed = transcribe_blocks(&wav, Some(&path), "model-a", &cancel, 16000, move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            async { Ok(result("next")) }
        })
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 4); // successful first upload not repeated
        assert_eq!(resumed.text, "first\nnext\nnext");
        let cached = transcribe_blocks(&wav, Some(&path), "model-a", &cancel, 16000, |_| async {
            panic!("completed uploads must not run again")
        })
        .await
        .unwrap();
        assert_eq!(cached.text, resumed.text);
        let count = calls.clone();
        transcribe_blocks(&wav, Some(&path), "model-b", &cancel, 16000, move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            async { Ok(result("new model")) }
        })
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 7);
        let mut changed_audio = wav.clone();
        *changed_audio.last_mut().unwrap() = 1;
        let count = calls.clone();
        transcribe_blocks(
            &changed_audio,
            Some(&path),
            "model-b",
            &cancel,
            16000,
            move |_| {
                count.fetch_add(1, Ordering::SeqCst);
                async { Ok(result("changed audio")) }
            },
        )
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 10);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn cancellation_between_uploads_preserves_completed_work() {
        let path = path();
        let wav = audio(32000);
        let token = CancellationToken::new();
        let cancel = token.clone();
        assert!(
            transcribe_blocks(&wav, Some(&path), "model", &token, 16000, move |_| {
                cancel.cancel();
                async { Ok(result("saved")) }
            })
            .await
            .is_err()
        );
        let result = transcribe_blocks(
            &wav,
            Some(&path),
            "model",
            &CancellationToken::new(),
            16000,
            |_| async { Ok(result("remaining")) },
        )
        .await
        .unwrap();
        assert_eq!(result.text, "saved\nremaining");
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn long_meeting_has_small_uploads_no_missing_audio_and_one_result() {
        let path = path();
        let frames = 16000 * 35 * 60;
        let wav = audio(frames);
        assert!(wav.len() > 50 * 1024 * 1024);
        let total = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let uploaded = total.clone();
        let count = calls.clone();
        let output = transcribe(
            &wav,
            Some(&path),
            "model",
            &CancellationToken::new(),
            move |chunk| {
                assert!(chunk.len() < 20 * 1024 * 1024);
                let reader = hound::WavReader::new(std::io::Cursor::new(chunk)).unwrap();
                uploaded.fetch_add(reader.duration() as usize, Ordering::SeqCst);
                count.fetch_add(1, Ordering::SeqCst);
                async { Ok(result("part")) }
            },
        )
        .await
        .unwrap();
        assert_eq!(total.load(Ordering::SeqCst), frames);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert_eq!(output.text, "part\npart\npart\npart");
        std::fs::remove_file(path).unwrap();
    }
}
