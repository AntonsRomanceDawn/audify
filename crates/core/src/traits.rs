//! The provider-agnostic seams of the pipeline.
//!
//! Each stage is a trait. Concrete providers live in `audify-pipeline`. The rest
//! of the codebase depends only on these traits, so a provider swap is a new
//! struct and a one-line wiring change — nothing more.

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::Result;
use crate::types::{Audio, AudioFormat, Document, JobState, Source};

/// Turns a [`Source`] into a structured [`Document`].
#[async_trait]
pub trait Extractor: Send + Sync {
    async fn extract(&self, source: &Source) -> Result<Document>;
}

/// Turns a [`Document`] into a single speakable plain-text string.
///
/// Synchronous on purpose: the default implementation is pure rule-based text
/// processing. An LLM-backed "speakify" implementation can wrap or replace it
/// later without changing this signature (it would implement an async variant
/// behind its own trait if needed).
pub trait Normalizer: Send + Sync {
    fn normalize(&self, document: &Document) -> Result<String>;
}

/// Turns speakable text into [`Audio`] in the requested [`AudioFormat`].
#[async_trait]
pub trait Synthesizer: Send + Sync {
    async fn synthesize(&self, text: &str, format: AudioFormat) -> Result<Audio>;
}

/// Persists rendered bytes under a key, returning a locator (path or URI).
#[async_trait]
pub trait Storage: Send + Sync {
    async fn store(&self, key: &str, bytes: &[u8]) -> Result<String>;
}

/// A job claimed from the queue for processing.
#[derive(Debug, Clone)]
pub struct ClaimedJob {
    pub job_id: Uuid,
    pub episode_id: Uuid,
    pub attempts: i32,
}

/// The work queue. Phase 2 backs this with the database (durable, restart-safe);
/// Phase 4 can swap in Redis behind the same trait without touching callers.
#[async_trait]
pub trait Queue: Send + Sync {
    /// Enqueue processing for an episode; returns the new job id.
    async fn enqueue(&self, episode_id: Uuid) -> Result<Uuid>;

    /// Claim the next runnable job (pending, or lease-expired and reclaimable),
    /// leasing it for `lease_secs`. Returns `None` if nothing is available.
    async fn claim(&self, lease_secs: i64) -> Result<Option<ClaimedJob>>;

    /// Record a job's current state for progress visibility.
    async fn set_state(&self, job_id: Uuid, state: JobState) -> Result<()>;

    /// Mark a job finished successfully.
    async fn complete(&self, job_id: Uuid) -> Result<()>;

    /// Mark a job failed with an error message.
    async fn fail(&self, job_id: Uuid, error: &str) -> Result<()>;
}
