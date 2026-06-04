//! Mistral Voxtral text-to-speech client (plain reqwest + JSON, no SDK).
//!
//! NOTE: Mistral documents the SDK call `client.audio.speech.complete(...)`
//! rather than the raw wire format. This client codes against the documented
//! field names (`model`, `input`, `voice_id`, `response_format`, and a base64
//! `audio_data` in the response) and keeps the base URL configurable. Verify
//! the exact path/schema against the live API with a real key before relying
//! on it; the endpoint path is the most likely thing to need adjustment.

use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};

use audify_core::{Audio, AudioFormat, Error, Result, Synthesizer};

/// Voxtral cloud TTS synthesizer.
pub struct VoxtralSynthesizer {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    voice_id: String,
}

impl VoxtralSynthesizer {
    pub fn new(
        client: reqwest::Client,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        voice_id: impl Into<String>,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            voice_id: voice_id.into(),
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/audio/speech", self.base_url.trim_end_matches('/'))
    }

    /// Fetch the raw JSON list of available voices from the API.
    ///
    /// Returns the response body verbatim so callers can display whatever shape
    /// the API uses. Doubles as a connectivity / auth smoke-test.
    pub async fn list_voices(&self) -> Result<String> {
        let url = format!("{}/v1/audio/voices", self.base_url.trim_end_matches('/'));
        let resp = self
            .client
            .get(url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(Error::Synthesis(format!(
                "listing voices failed ({status}): {body}"
            )));
        }
        Ok(body)
    }
}

#[derive(Serialize)]
struct SpeechRequest<'a> {
    model: &'a str,
    input: &'a str,
    voice_id: &'a str,
    response_format: &'a str,
}

#[derive(Deserialize)]
struct SpeechResponse {
    /// Base64-encoded audio bytes.
    audio_data: String,
}

#[async_trait]
impl Synthesizer for VoxtralSynthesizer {
    async fn synthesize(&self, text: &str, format: AudioFormat) -> Result<Audio> {
        let body = SpeechRequest {
            model: &self.model,
            input: text,
            voice_id: &self.voice_id,
            response_format: format.as_api_str(),
        };

        let resp = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let detail = resp.text().await.unwrap_or_default();
            return Err(Error::Synthesis(format!("Voxtral returned {status}: {detail}")));
        }

        let parsed: SpeechResponse = resp
            .json()
            .await
            .map_err(|e| Error::Synthesis(format!("decoding Voxtral response: {e}")))?;

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(parsed.audio_data.as_bytes())
            .map_err(|e| Error::Synthesis(format!("decoding base64 audio: {e}")))?;

        Ok(Audio { bytes, format })
    }
}
