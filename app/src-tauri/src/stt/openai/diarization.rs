//! OpenAI's diarization request/response contract (also through API Edge).
//! Speaker labels are local to a single request. No voice references are sent.
use super::OpenAiSttProvider;
use crate::stt::{openai_compat, SpeakerSegment, SttError, SttTranscript};
use serde::Deserialize;

#[derive(Deserialize)]
struct Response {
    text: String,
    segments: Vec<Segment>,
}
#[derive(Deserialize)]
struct Segment {
    speaker: String,
    text: String,
    start: f64,
    end: f64,
}

fn parse(value: serde_json::Value) -> Result<SttTranscript, SttError> {
    let response: Response = serde_json::from_value(value)
        .map_err(|_| SttError::Api("Invalid diarized transcription response".into()))?;
    let mut segments = Vec::with_capacity(response.segments.len());
    for segment in response.segments {
        if !segment.start.is_finite()
            || !segment.end.is_finite()
            || segment.start < 0.0
            || segment.end < segment.start
            || segment.speaker.len() > 100
        {
            return Err(SttError::Api("Invalid diarized speaker segment".into()));
        }
        segments.push(SpeakerSegment {
            speaker: segment.speaker,
            text: segment.text,
            start_seconds: segment.start,
            end_seconds: segment.end,
            part: 1,
        });
    }
    Ok(SttTranscript {
        text: response.text,
        segments,
    })
}

pub(super) async fn transcribe(
    provider: &OpenAiSttProvider,
    audio: &[u8],
) -> Result<SttTranscript, SttError> {
    let form = openai_compat::wav_transcription_form(
        audio,
        &provider.model,
        None,
        provider.default_language.as_deref(),
    )?
    .text("response_format", "diarized_json")
    .text("chunking_strategy", "auto");
    let endpoint = provider.transcriptions_url();
    let response = crate::http::with_cloudflare_access_headers_if_target(
        provider
            .client
            .post(&endpoint)
            .bearer_auth(&provider.api_key),
        &endpoint,
    )
    .multipart(form)
    .send()
    .await
    .map_err(|error| {
        if error.is_timeout() {
            SttError::Timeout
        } else {
            SttError::Network(error)
        }
    })?;
    if !response.status().is_success() {
        // Do not place meeting content or opaque provider responses in error logs.
        return Err(SttError::Api(format!(
            "Diarized transcription failed ({})",
            response.status()
        )));
    }
    parse(response.json().await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stt::{AudioFormat, SttProvider};
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    #[tokio::test]
    async fn diarized_request_preserves_segments_and_omits_unsupported_options() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"text":"Test meeting", "segments":[{"speaker":"A","text":"Test meeting","start":0.0,"end":2.0}]})))
            .expect(1).mount(&server).await;
        let provider = OpenAiSttProvider::new(
            "test-placeholder".into(),
            Some("gpt-4o-transcribe-diarize".into()),
            None,
            Some("must be omitted".into()),
        )
        .with_api_base_url(server.uri());
        let result = provider
            .transcribe_detailed(b"synthetic audio", &AudioFormat::default())
            .await
            .unwrap();
        assert_eq!(result.segments[0].speaker, "A");
        assert_eq!(result.text, "Test meeting");
        let requests = server.received_requests().await.unwrap();
        let body = String::from_utf8_lossy(&requests[0].body);
        assert!(body.contains("diarized_json"));
        assert!(body.contains("chunking_strategy"));
        assert!(body.contains("auto"));
        assert!(!body.contains("name=\"prompt\""));
        assert!(!body.contains("timestamp_granularities"));
        assert!(!body.contains("known_speaker"));
    }
}
