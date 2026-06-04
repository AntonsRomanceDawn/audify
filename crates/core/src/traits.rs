//! The provider-agnostic seams of the pipeline.
//!
//! Each stage is a trait. Concrete providers live in `audify-pipeline`. The rest
//! of the codebase depends only on these traits, so a provider swap is a new
//! struct and a one-line wiring change — nothing more.

use async_trait::async_trait;

use crate::error::Result;
use crate::types::{Audio, AudioFormat, Document, Source};

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
