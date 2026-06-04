//! Concrete pipeline stages and the orchestrator that wires them together.
//!
//! Each submodule provides an implementation of a trait from `audify-core`.
//! Nothing here is referenced by type from outside — callers depend on the
//! `audify-core` traits and pick an implementation at construction time.

pub mod assemble;
pub mod chunk;
pub mod extract;
pub mod normalize;
pub mod storage;
pub mod synth;

use audify_core::{
    Audio, AudioFormat, Error, Extractor, Normalizer, Result, Source, Storage, Synthesizer,
};
use futures::stream::{self, StreamExt, TryStreamExt};

/// How many chunks to synthesize concurrently.
const SYNTH_CONCURRENCY: usize = 4;

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

/// Run the full pipeline: extract → normalize → chunk → synthesize → assemble → store.
///
/// `key` is the storage key without extension; episodes are always MP3.
#[tracing::instrument(skip_all, fields(key = %key))]
pub async fn process(
    source: &Source,
    extractor: &dyn Extractor,
    normalizer: &dyn Normalizer,
    synthesizer: &dyn Synthesizer,
    storage: &dyn Storage,
    key: &str,
) -> Result<ProcessOutput> {
    let document = extractor.extract(source).await?;
    if document.is_empty() {
        return Err(Error::Extraction("no content extracted from source".into()));
    }
    let char_count = document.char_count();
    tracing::info!(char_count, title = ?document.title, sections = document.sections.len(), "extracted");

    let text = normalizer.normalize(&document)?;
    let chunks = chunk::chunk_text(&text, chunk::DEFAULT_TARGET_CHARS);
    tracing::info!(speakable_chars = text.chars().count(), chunks = chunks.len(), "normalized + chunked");

    let segments = synthesize_chunks(synthesizer, &chunks).await?;
    tracing::info!(segments = segments.len(), "synthesized");

    let mp3 = assemble::to_mp3(&segments).await?;
    tracing::info!(bytes = mp3.len(), "assembled");

    let object_key = format!("{key}.mp3");
    let path = storage.store(&object_key, &mp3).await?;
    tracing::info!(%path, "stored");

    Ok(ProcessOutput {
        path,
        title: document.title,
        char_count,
        audio_bytes: mp3.len(),
    })
}

/// Result of a dry-run preview: extraction + normalization, no synthesis.
#[derive(Debug, Clone)]
pub struct PreviewOutput {
    pub title: Option<String>,
    pub char_count: usize,
    pub chunks: usize,
    /// The exact speakable text that would be sent to the synthesizer.
    pub text: String,
}

/// Run extract → normalize → chunk only, returning the speakable text. No API
/// calls, no cost — for checking extraction quality before synthesizing.
#[tracing::instrument(skip_all)]
pub async fn preview(
    source: &Source,
    extractor: &dyn Extractor,
    normalizer: &dyn Normalizer,
) -> Result<PreviewOutput> {
    let document = extractor.extract(source).await?;
    if document.is_empty() {
        return Err(Error::Extraction("no content extracted from source".into()));
    }
    let char_count = document.char_count();
    let text = normalizer.normalize(&document)?;
    let chunks = chunk::chunk_text(&text, chunk::DEFAULT_TARGET_CHARS).len();
    Ok(PreviewOutput {
        title: document.title,
        char_count,
        chunks,
        text,
    })
}

/// Synthesize chunks to WAV concurrently, preserving order.
async fn synthesize_chunks(
    synthesizer: &dyn Synthesizer,
    chunks: &[String],
) -> Result<Vec<Audio>> {
    stream::iter(chunks.iter())
        .map(|chunk| synthesizer.synthesize(chunk, AudioFormat::Wav))
        .buffered(SYNTH_CONCURRENCY)
        .try_collect()
        .await
}
