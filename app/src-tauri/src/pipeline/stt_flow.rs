//! STT transcription flow helpers.
//!
//! This module extracts the shared logic for running STT transcription with
//! retry, optional timeout, and cancellation.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::stt::{with_retry_report, AudioFormat, RetryConfig, RetryTelemetry, SttProvider};

use super::types::PipelineError;
use super::utils::normalize_stt_text;

/// Result of running STT transcription.
pub(super) struct SttResult {
    /// The transcribed (and normalized) text.
    pub text: String,
    pub segments: Vec<crate::stt::SpeakerSegment>,
    /// Duration of the STT request in milliseconds.
    pub duration_ms: u64,
    /// Retry/backoff telemetry for this STT request.
    pub retry: RetryTelemetry,
}

/// Run STT transcription with retry, optional timeout, and cancellation.
///
/// Returns the transcribed text and duration, or an error.
/// If `timeout` is None, no timeout is applied (useful for test endpoints).
pub(super) async fn run_stt_transcription(
    stt_provider: Arc<dyn SttProvider>,
    wav_bytes: &[u8],
    retry_config: &RetryConfig,
    timeout: Option<Duration>,
    cancel_token: &CancellationToken,
    log_prefix: &str,
) -> Result<SttResult, PipelineError> {
    let format = AudioFormat::default();
    let wav = Arc::new(wav_bytes.to_vec());

    let transcription_future = async {
        with_retry_report(retry_config, || {
            let provider = stt_provider.clone();
            let wav = wav.clone();
            let format = format.clone();
            async move { provider.transcribe_detailed(wav.as_slice(), &format).await }
        })
        .await
    };

    let stt_start = std::time::Instant::now();

    let stt_result = if let Some(timeout_duration) = timeout {
        // With timeout
        tokio::select! {
            biased;

            _ = cancel_token.cancelled() => {
                log::info!("{}: Transcription cancelled", log_prefix);
                Err(PipelineError::Cancelled)
            }

            _ = tokio::time::sleep(timeout_duration) => {
                log::warn!("{}: Transcription timed out after {:?}", log_prefix, timeout_duration);
                Err(PipelineError::Timeout(timeout_duration))
            }

            result = transcription_future => {
                Ok(result)
            }
        }
    } else {
        // No timeout, just cancellation
        tokio::select! {
            biased;

            _ = cancel_token.cancelled() => {
                log::info!("{}: Transcription cancelled", log_prefix);
                Err(PipelineError::Cancelled)
            }

            result = transcription_future => {
                Ok(result)
            }
        }
    };

    let duration_ms = stt_start.elapsed().as_millis() as u64;

    match stt_result {
        Ok(outcome) => match outcome.result {
            Ok(transcript) => {
                let normalized = normalize_stt_text(transcript.text);
                log::info!("{}: STT complete, {} chars", log_prefix, normalized.len());
                Ok(SttResult {
                    text: normalized,
                    segments: transcript.segments,
                    duration_ms,
                    retry: outcome.telemetry,
                })
            }
            Err(e) => Err(PipelineError::from(e)),
        },
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stt::SttError;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct FlakyProvider {
        calls: AtomicU32,
        succeed_on_call: u32,
    }

    impl FlakyProvider {
        fn new(succeed_on_call: u32) -> Self {
            Self {
                calls: AtomicU32::new(0),
                succeed_on_call,
            }
        }

        fn calls(&self) -> u32 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl SttProvider for FlakyProvider {
        async fn transcribe(
            &self,
            _audio: &[u8],
            _format: &AudioFormat,
        ) -> Result<String, SttError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call < self.succeed_on_call {
                Err(SttError::Timeout)
            } else {
                Ok("  hello from stt  ".to_string())
            }
        }

        fn name(&self) -> &'static str {
            "flaky"
        }
    }

    struct ConfigErrorProvider {
        calls: AtomicU32,
    }

    impl ConfigErrorProvider {
        fn new() -> Self {
            Self {
                calls: AtomicU32::new(0),
            }
        }

        fn calls(&self) -> u32 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl SttProvider for ConfigErrorProvider {
        async fn transcribe(
            &self,
            _audio: &[u8],
            _format: &AudioFormat,
        ) -> Result<String, SttError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(SttError::Config("missing test config".to_string()))
        }

        fn name(&self) -> &'static str {
            "config-error"
        }
    }

    struct DelayedProvider {
        delay: Duration,
    }

    #[async_trait]
    impl SttProvider for DelayedProvider {
        async fn transcribe(
            &self,
            _audio: &[u8],
            _format: &AudioFormat,
        ) -> Result<String, SttError> {
            tokio::time::sleep(self.delay).await;
            Ok("eventual transcript".to_string())
        }

        fn name(&self) -> &'static str {
            "delayed"
        }
    }

    fn no_delay_retry_config(max_retries: u32) -> RetryConfig {
        RetryConfig {
            max_retries,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            retry_on_rate_limit: true,
        }
    }

    #[tokio::test]
    async fn returns_retry_telemetry_after_successful_retry() {
        let provider = Arc::new(FlakyProvider::new(3));
        let retry_config = no_delay_retry_config(3);
        let cancel_token = CancellationToken::new();

        let result = run_stt_transcription(
            provider.clone(),
            b"fake wav",
            &retry_config,
            Some(Duration::from_secs(1)),
            &cancel_token,
            "test retry telemetry",
        )
        .await
        .expect("STT should eventually succeed");

        assert_eq!(result.text, "hello from stt  ");
        assert_eq!(provider.calls(), 3);
        assert_eq!(result.retry.attempts, 3);
        assert_eq!(result.retry.retries, 2);
        assert_eq!(result.retry.total_delay_ms, 0);
        assert!(result.retry.last_error.is_some());
    }

    #[tokio::test]
    async fn does_not_retry_non_retryable_config_errors() {
        let provider = Arc::new(ConfigErrorProvider::new());
        let retry_config = no_delay_retry_config(3);
        let cancel_token = CancellationToken::new();

        let result = run_stt_transcription(
            provider.clone(),
            b"fake wav",
            &retry_config,
            Some(Duration::from_secs(1)),
            &cancel_token,
            "test non retryable",
        )
        .await;

        assert!(matches!(
            result,
            Err(PipelineError::Stt(SttError::Config(_)))
        ));
        assert_eq!(provider.calls(), 1);
    }

    #[tokio::test]
    async fn none_timeout_allows_slow_test_transcription_to_complete() {
        let provider = Arc::new(DelayedProvider {
            delay: Duration::from_millis(10),
        });
        let retry_config = no_delay_retry_config(0);
        let cancel_token = CancellationToken::new();

        let result = run_stt_transcription(
            provider,
            b"fake wav",
            &retry_config,
            None,
            &cancel_token,
            "test no timeout",
        )
        .await
        .expect("no-timeout STT should complete");

        assert_eq!(result.text, "eventual transcript");
    }

    #[tokio::test]
    async fn configured_timeout_fails_slow_transcription() {
        let provider = Arc::new(DelayedProvider {
            delay: Duration::from_millis(50),
        });
        let retry_config = no_delay_retry_config(0);
        let cancel_token = CancellationToken::new();

        let result = run_stt_transcription(
            provider,
            b"fake wav",
            &retry_config,
            Some(Duration::from_millis(1)),
            &cancel_token,
            "test timeout",
        )
        .await;

        assert!(matches!(result, Err(PipelineError::Timeout(_))));
    }

    #[tokio::test]
    async fn cancellation_wins_when_already_cancelled() {
        let provider = Arc::new(DelayedProvider {
            delay: Duration::from_millis(50),
        });
        let retry_config = no_delay_retry_config(0);
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();

        let result = run_stt_transcription(
            provider,
            b"fake wav",
            &retry_config,
            Some(Duration::ZERO),
            &cancel_token,
            "test cancellation",
        )
        .await;

        assert!(matches!(result, Err(PipelineError::Cancelled)));
    }
}
