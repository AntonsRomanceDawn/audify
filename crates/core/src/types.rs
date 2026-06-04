//! Domain types shared across the pipeline.

use serde::{Deserialize, Serialize};

/// What the user asked us to turn into audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A web page to fetch and extract.
    Url(String),
    /// Raw text supplied directly (already an "article").
    Text(String),
}

/// A document extracted into clean structure, before normalization.
///
/// Keeping structure (rather than one flat string) is what lets the normalizer
/// announce headings, drop whole sections (references), and segment sensibly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Document {
    /// Title, if one could be identified.
    pub title: Option<String>,
    /// Ordered sections in reading order.
    pub sections: Vec<Section>,
}

/// One section of a document: an optional heading and its paragraphs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Section {
    /// Heading text, if this block had one.
    pub heading: Option<String>,
    /// Paragraphs of body text, already trimmed.
    pub paragraphs: Vec<String>,
}

impl Document {
    /// Total character count of all body text — used for cost estimation.
    pub fn char_count(&self) -> usize {
        self.sections
            .iter()
            .flat_map(|s| s.paragraphs.iter())
            .map(|p| p.chars().count())
            .sum()
    }

    /// True when there is no usable body text.
    pub fn is_empty(&self) -> bool {
        self.sections.iter().all(|s| s.paragraphs.is_empty())
    }
}

/// Audio output formats the synthesizer can be asked to produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    Mp3,
    Wav,
    Pcm,
    Flac,
    Opus,
}

impl AudioFormat {
    /// The file extension for this format.
    pub fn extension(&self) -> &'static str {
        match self {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Wav => "wav",
            AudioFormat::Pcm => "pcm",
            AudioFormat::Flac => "flac",
            AudioFormat::Opus => "opus",
        }
    }

    /// The wire value the Voxtral API expects in `response_format`.
    pub fn as_api_str(&self) -> &'static str {
        self.extension()
    }
}

impl std::str::FromStr for AudioFormat {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mp3" => Ok(AudioFormat::Mp3),
            "wav" => Ok(AudioFormat::Wav),
            "pcm" => Ok(AudioFormat::Pcm),
            "flac" => Ok(AudioFormat::Flac),
            "opus" => Ok(AudioFormat::Opus),
            other => Err(crate::Error::Config(format!(
                "unsupported audio format: {other}"
            ))),
        }
    }
}

/// Rendered audio: raw bytes plus the format they are encoded in.
#[derive(Debug, Clone)]
pub struct Audio {
    pub bytes: Vec<u8>,
    pub format: AudioFormat,
}
