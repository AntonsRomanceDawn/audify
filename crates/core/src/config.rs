//! Typed configuration, loaded from `AUDIFY_*` environment variables and
//! validated at startup. Invalid config is a hard error before any work begins.

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::types::AudioFormat;

/// Application configuration. All fields are populated from environment
/// variables prefixed with `AUDIFY_` (e.g. `AUDIFY_MISTRAL_API_KEY`).
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Mistral API key for Voxtral TTS.
    pub mistral_api_key: String,

    /// Base URL of the Mistral API.
    #[serde(default = "defaults::base_url")]
    pub mistral_base_url: String,

    /// TTS model identifier.
    #[serde(default = "defaults::tts_model")]
    pub tts_model: String,

    /// Preset voice id to render with. Optional so commands that don't synthesize
    /// (e.g. listing voices) work before one has been chosen.
    #[serde(default)]
    pub voice_id: Option<String>,

    /// Requested output format: mp3 | wav | pcm | flac | opus.
    #[serde(default = "defaults::response_format")]
    pub response_format: String,

    /// Directory rendered audio is written to.
    #[serde(default = "defaults::output_dir")]
    pub output_dir: String,

    /// Timeout for outbound HTTP calls, in seconds.
    #[serde(default = "defaults::http_timeout_secs")]
    pub http_timeout_secs: u64,
}

mod defaults {
    pub fn base_url() -> String {
        "https://api.mistral.ai".to_string()
    }
    pub fn tts_model() -> String {
        "voxtral-mini-tts-2603".to_string()
    }
    pub fn response_format() -> String {
        "mp3".to_string()
    }
    pub fn output_dir() -> String {
        "./output".to_string()
    }
    pub fn http_timeout_secs() -> u64 {
        120
    }
}

impl Config {
    /// Load configuration from the environment and validate it.
    ///
    /// Returns [`Error::Config`] if anything is missing or invalid, so callers
    /// can fail fast at startup.
    pub fn load() -> Result<Self> {
        let cfg: Config = config::Config::builder()
            .add_source(config::Environment::with_prefix("AUDIFY"))
            .build()
            .map_err(|e| Error::Config(e.to_string()))?
            .try_deserialize()
            .map_err(|e| Error::Config(e.to_string()))?;

        cfg.validate()?;
        Ok(cfg)
    }

    /// The parsed, validated audio format.
    pub fn audio_format(&self) -> Result<AudioFormat> {
        self.response_format.parse()
    }

    /// The configured voice id, or a helpful error if none is set yet.
    pub fn require_voice_id(&self) -> Result<&str> {
        match self.voice_id.as_deref().map(str::trim) {
            Some(v) if !v.is_empty() => Ok(v),
            _ => Err(Error::Config(
                "AUDIFY_VOICE_ID is not set — run `audify voices` to list available voice ids".into(),
            )),
        }
    }

    /// Validate invariants that `serde` cannot express on its own.
    fn validate(&self) -> Result<()> {
        if self.mistral_api_key.trim().is_empty() {
            return Err(Error::Config(
                "AUDIFY_MISTRAL_API_KEY must not be empty".into(),
            ));
        }
        // voice_id is validated only when a command actually synthesizes audio,
        // so `audify voices` works before one has been chosen.
        if self.http_timeout_secs == 0 {
            return Err(Error::Config(
                "AUDIFY_HTTP_TIMEOUT_SECS must be greater than 0".into(),
            ));
        }
        // Surfaces a bad format string at startup rather than mid-pipeline.
        let _ = self.audio_format()?;
        Ok(())
    }
}
