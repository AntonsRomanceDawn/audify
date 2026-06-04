//! Application services shared by the CLI and the HTTP API.

use anyhow::{bail, Context};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use audify_core::{Extractor, Queue, Source};
use audify_db::{DbQueue, DocumentRepository, EpisodeRepository};

/// Per-1000-character price used for the cost estimate.
const PRICE_PER_1K_CHARS: f64 = 0.016;

/// Outcome of submitting a source for processing.
pub struct Submitted {
    pub episode_id: Uuid,
    pub job_id: Uuid,
    pub char_count: i32,
    pub est_cost: f64,
}

/// Extract a source, persist its (deduplicated) document + a new episode, and
/// enqueue a processing job. The expensive TTS work happens later in the worker.
pub async fn submit(
    source: &Source,
    extractor: &dyn Extractor,
    documents: &DocumentRepository,
    episodes: &EpisodeRepository,
    queue: &DbQueue,
    voice_id: &str,
) -> anyhow::Result<Submitted> {
    let document = extractor.extract(source).await.context("extracting")?;
    if document.is_empty() {
        bail!("no content extracted from source");
    }

    let char_count = i32::try_from(document.char_count()).unwrap_or(i32::MAX);
    let struct_json = serde_json::to_value(&document).context("serializing document")?;
    let content_hash = sha256_hex(&struct_json.to_string());
    let (source_type, source_ref) = source_descriptor(source);

    let doc = documents
        .upsert(
            &source_type,
            &source_ref,
            &content_hash,
            document.title.as_deref(),
            &struct_json,
        )
        .await?;
    let est_cost = char_count as f64 / 1000.0 * PRICE_PER_1K_CHARS;
    let episode = episodes.create(doc.id, voice_id, char_count, est_cost).await?;
    let job_id = queue.enqueue(episode.id).await?;

    Ok(Submitted {
        episode_id: episode.id,
        job_id,
        char_count,
        est_cost,
    })
}

/// Persisted source-type tag and a human-readable reference.
fn source_descriptor(source: &Source) -> (String, String) {
    match source {
        Source::Url(u) => ("url".into(), u.clone()),
        Source::Text(t) => ("text".into(), t.chars().take(120).collect()),
        Source::File(p) => ("file".into(), p.to_string_lossy().into_owned()),
    }
}

/// Full hex SHA-256 of a string.
fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
