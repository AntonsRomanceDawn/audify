//! Concrete pipeline stages and the orchestrator that wires them together.
//!
//! Each submodule provides an implementation of a trait from `audify-core`.
//! Nothing here is referenced by type from outside — callers depend on the
//! `audify-core` traits and pick an implementation at construction time.

pub mod extract;
pub mod normalize;
pub mod storage;
pub mod synth;

use audify_core::{
    AudioFormat, Error, Extractor, Normalizer, Result, Source, Storage, Synthesizer,
};

/// Result of a full pipeline run.
#[derive(Debug, Clone)]
pub struct ProcessOutput {
    /// Locator (path or URI) of the stored audio.
    pub path: String,
    /// Title detected during extraction, if any.
    pub title: Option<String>,
    /// Characters of speakable text sent to the synthesizer.
    pub char_count: usize,
    /// Size of the rendered audio in bytes.
    pub audio_bytes: usize,
}

/// Run the full pipeline: extract → normalize → synthesize → store.
///
/// `key` is the storage key without extension; the audio format's extension is
/// appended automatically.
#[tracing::instrument(skip_all, fields(key = %key))]
pub async fn process(
    source: &Source,
    extractor: &dyn Extractor,
    normalizer: &dyn Normalizer,
    synthesizer: &dyn Synthesizer,
    storage: &dyn Storage,
    format: AudioFormat,
    key: &str,
) -> Result<ProcessOutput> {
    let document = extractor.extract(source).await?;
    if document.is_empty() {
        return Err(Error::Extraction("no content extracted from source".into()));
    }
    let char_count = document.char_count();
    tracing::info!(char_count, title = ?document.title, sections = document.sections.len(), "extracted");

    let text = normalizer.normalize(&document)?;
    tracing::info!(speakable_chars = text.chars().count(), "normalized");

    let audio = synthesizer.synthesize(&text, format).await?;
    tracing::info!(bytes = audio.bytes.len(), format = ?audio.format, "synthesized");

    let object_key = format!("{key}.{}", audio.format.extension());
    let path = storage.store(&object_key, &audio.bytes).await?;
    tracing::info!(%path, "stored");

    Ok(ProcessOutput {
        path,
        title: document.title,
        char_count,
        audio_bytes: audio.bytes.len(),
    })
}
