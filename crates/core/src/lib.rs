//! Shared types, traits, errors, and configuration for Audify.
//!
//! Every other crate depends on the traits defined here, never on a concrete
//! provider type. Swapping a provider means writing a new struct that implements
//! one of these traits — nothing else in the codebase changes.

pub mod config;
pub mod error;
pub mod traits;
pub mod types;

pub use config::Config;
pub use error::{Error, Result};
pub use traits::{ClaimedJob, Extractor, Normalizer, Queue, Storage, Synthesizer};
pub use types::{
    Audio, AudioFormat, Document, EpisodeStatus, JobState, Section, Source,
};
