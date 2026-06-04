//! Library error type. Library crates use `thiserror`; only the binary uses `anyhow`.

use thiserror::Error;

/// All recoverable failures in the Audify pipeline.
#[derive(Debug, Error)]
pub enum Error {
    /// Configuration was missing or invalid. Raised at startup, before any work.
    #[error("configuration error: {0}")]
    Config(String),

    /// The provided source could not be understood (bad URL, empty text, ...).
    #[error("invalid source: {0}")]
    InvalidSource(String),

    /// Fetching or parsing the document failed.
    #[error("extraction failed: {0}")]
    Extraction(String),

    /// The text-to-speech provider failed.
    #[error("synthesis failed: {0}")]
    Synthesis(String),

    /// Persisting the rendered audio failed.
    #[error("storage error: {0}")]
    Storage(String),

    /// An outbound HTTP call failed at the transport level.
    #[error("http error: {0}")]
    Http(String),
}

/// Convenience alias used throughout the library crates.
pub type Result<T> = std::result::Result<T, Error>;
