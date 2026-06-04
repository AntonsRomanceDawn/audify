//! Background worker: drains the queue and runs episodes through the pipeline.
//!
//! Drives each job through the state machine (Normalizing → Synthesizing →
//! Assembling → Ready), persisting progress. Chunk audio is written to disk and
//! recorded, so a retry resumes from the first un-synthesized chunk instead of
//! re-billing the TTS API. Failures mark the job and episode `failed`.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use tokio::time::sleep;

use audify_core::{
    Audio, AudioFormat, ClaimedJob, Config, Document, EpisodeStatus, JobState, Normalizer, Queue,
    Storage, Synthesizer,
};
use audify_db::{ChunkRepository, DbQueue, DocumentRepository, EpisodeRepository};
use audify_pipeline::{assemble, chunk, normalize::RuleNormalizer, storage::LocalStorage,
    synth::VoxtralSynthesizer};

/// How long a claimed job is leased before it's considered abandoned.
const LEASE_SECS: i64 = 1800;
/// How long to wait when the queue is empty before polling again.
const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// Build a worker from config and run its loop.
pub async fn run(config: &Config, http: reqwest::Client, once: bool) -> anyhow::Result<()> {
    let database_url = config.require_database_url()?;
    let voice_id = config.require_voice_id()?;
    let mistral_api_key = config.require_mistral_api_key()?;
    let pool = audify_db::connect(database_url)
        .await
        .context("connecting to database")?;

    let worker = Worker {
        queue: DbQueue::new(pool.clone()),
        documents: DocumentRepository::new(pool.clone()),
        episodes: EpisodeRepository::new(pool.clone()),
        chunks: ChunkRepository::new(pool),
        normalizer: RuleNormalizer::new()?,
        synthesizer: VoxtralSynthesizer::new(
            http,
            &config.mistral_base_url,
            mistral_api_key,
            &config.tts_model,
            voice_id,
        ),
        storage: LocalStorage::new(&config.output_dir),
        output_dir: config.output_dir.clone(),
    };
    worker.run(once).await
}

struct Worker {
    queue: DbQueue,
    documents: DocumentRepository,
    episodes: EpisodeRepository,
    chunks: ChunkRepository,
    normalizer: RuleNormalizer,
    synthesizer: VoxtralSynthesizer,
    storage: LocalStorage,
    output_dir: String,
}

impl Worker {
    async fn run(self, once: bool) -> anyhow::Result<()> {
        tracing::info!(once, "worker started");
        loop {
            match self.queue.claim(LEASE_SECS).await {
                Ok(Some(job)) => {
                    tracing::info!(job_id = %job.job_id, episode_id = %job.episode_id, attempts = job.attempts, "claimed job");
                    if let Err(e) = self.process(&job).await {
                        tracing::error!(job_id = %job.job_id, error = %e, "job failed");
                        let _ = self.queue.fail(job.job_id, &e.to_string()).await;
                        let _ = self
                            .episodes
                            .set_status(job.episode_id, EpisodeStatus::Failed)
                            .await;
                    } else {
                        let _ = self.queue.complete(job.job_id).await;
                        tracing::info!(job_id = %job.job_id, "job complete");
                    }
                }
                Ok(None) => {
                    if once {
                        break;
                    }
                    sleep(POLL_INTERVAL).await;
                    continue;
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to claim job");
                    if once {
                        break;
                    }
                    sleep(POLL_INTERVAL).await;
                    continue;
                }
            }
            if once {
                break;
            }
        }
        Ok(())
    }

    #[tracing::instrument(skip(self, job), fields(episode_id = %job.episode_id))]
    async fn process(&self, job: &ClaimedJob) -> anyhow::Result<()> {
        let episode_id = job.episode_id;
        let episode = self
            .episodes
            .get(episode_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("episode {episode_id} not found"))?;
        self.episodes
            .set_status(episode_id, EpisodeStatus::Processing)
            .await?;

        // Reconstruct the extracted document and produce speakable chunks.
        let doc_row = self
            .documents
            .get(episode.document_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("document {} not found", episode.document_id))?;
        let document: Document =
            serde_json::from_value(doc_row.struct_json).context("decoding stored document")?;
        let text = self.normalizer.normalize(&document)?;
        let chunk_texts = chunk::chunk_text(&text, chunk::DEFAULT_TARGET_CHARS);

        // Reuse existing chunk rows on retry; otherwise (re)create them.
        let existing = self.chunks.list_by_episode(episode_id).await?;
        let rows = if !existing.is_empty() && existing.len() == chunk_texts.len() {
            existing
        } else {
            let mut rows = Vec::with_capacity(chunk_texts.len());
            for (i, t) in chunk_texts.iter().enumerate() {
                rows.push(self.chunks.upsert(episode_id, i as i32, t).await?);
            }
            rows
        };

        // Synthesize each chunk, reusing any already rendered on disk.
        self.queue.set_state(job.job_id, JobState::Synthesizing).await?;
        let chunk_dir = Path::new(&self.output_dir)
            .join("chunks")
            .join(episode_id.to_string());
        tokio::fs::create_dir_all(&chunk_dir)
            .await
            .with_context(|| format!("creating {}", chunk_dir.display()))?;

        let mut audios: Vec<Audio> = Vec::with_capacity(rows.len());
        for row in &rows {
            let path = chunk_dir.join(format!("{}.wav", row.idx));
            let cached = row.audio_path.is_some()
                && tokio::fs::try_exists(&path).await.unwrap_or(false);
            if cached {
                let bytes = tokio::fs::read(&path).await.context("reading cached chunk")?;
                audios.push(Audio {
                    bytes,
                    format: AudioFormat::Wav,
                });
                tracing::info!(idx = row.idx, "reused cached chunk");
            } else {
                let audio = self.synthesizer.synthesize(&row.text, AudioFormat::Wav).await?;
                tokio::fs::write(&path, &audio.bytes)
                    .await
                    .context("writing chunk audio")?;
                self.chunks.set_audio(row.id, &path.to_string_lossy()).await?;
                tracing::info!(idx = row.idx, "synthesized chunk");
                audios.push(audio);
            }
        }

        // Assemble and finalize.
        self.queue.set_state(job.job_id, JobState::Assembling).await?;
        let mp3 = assemble::to_mp3(&audios).await?;
        let key = format!("{episode_id}.mp3");
        let final_path = self.storage.store(&key, &mp3).await?;
        let duration_sec = (episode.char_count as f64 / 12.5).round() as i32;
        self.episodes
            .set_ready(episode_id, &final_path, duration_sec)
            .await?;
        tracing::info!(%final_path, bytes = mp3.len(), duration_sec, "episode ready");
        Ok(())
    }
}
