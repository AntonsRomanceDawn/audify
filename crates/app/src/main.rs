//! Audify CLI — Phase 1 entrypoint.
//!
//! `audify convert --url https://example.com/article` runs the full pipeline and
//! writes an audio file. `audify voices` lists the preset voice ids your account
//! can use (and confirms the API key + endpoint work).

mod api;
mod service;
mod worker;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};
use tracing_subscriber::EnvFilter;

use audify_core::{Config, Source};
use audify_pipeline::{
    extract::RoutingExtractor, normalize::RuleNormalizer, process, storage::LocalStorage,
    synth::VoxtralSynthesizer,
};

/// Turn articles into a private podcast.
#[derive(Parser, Debug)]
#[command(name = "audify", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Convert an article (URL, raw text, or PDF file) into an audio file.
    Convert {
        /// URL of an article to fetch and convert.
        #[arg(long, conflicts_with_all = ["text", "file"])]
        url: Option<String>,
        /// Raw text to convert directly.
        #[arg(long, conflicts_with = "file")]
        text: Option<String>,
        /// Path to a local PDF file.
        #[arg(long)]
        file: Option<String>,
        /// Output file name (without extension). Defaults to a hash of the source.
        #[arg(long)]
        out: Option<String>,
        /// Print the speakable text and exit without synthesizing (free; no API call).
        #[arg(long)]
        preview: bool,
    },
    /// Submit a source for async processing: persists it and enqueues a job.
    Submit {
        #[arg(long, conflicts_with_all = ["text", "file"])]
        url: Option<String>,
        #[arg(long, conflicts_with = "file")]
        text: Option<String>,
        #[arg(long)]
        file: Option<String>,
    },
    /// Run the background worker: drain the queue and render episodes.
    Worker {
        /// Process at most one job (or none) and exit, instead of looping.
        #[arg(long)]
        once: bool,
    },
    /// Run the HTTP API server (episodes, audio, RSS feed).
    Serve,
    /// List the preset voice ids available to your account.
    Voices,
    /// Apply database migrations.
    Migrate,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let cli = Cli::parse();
    let config = Config::load().context("loading configuration")?;

    match cli.command {
        Command::Migrate => run_migrate(&config).await,
        Command::Submit { url, text, file } => {
            run_submit(&config, http_client(&config)?, url, text, file).await
        }
        Command::Worker { once } => worker::run(&config, http_client(&config)?, once).await,
        Command::Serve => api::serve(&config, http_client(&config)?).await,
        Command::Voices => run_voices(&config, http_client(&config)?).await,
        Command::Convert {
            url,
            text,
            file,
            out,
            preview,
        } => run_convert(&config, http_client(&config)?, url, text, file, out, preview).await,
    }
}

/// Build the outbound HTTP client used for fetching and synthesis.
fn http_client(config: &Config) -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(config.http_timeout_secs))
        .user_agent(concat!("audify/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("building HTTP client")
}

async fn run_migrate(config: &Config) -> anyhow::Result<()> {
    let database_url = config.require_database_url()?;
    let pool = audify_db::connect(database_url)
        .await
        .context("connecting to database")?;
    audify_db::migrate(&pool).await.context("applying migrations")?;
    tracing::info!("migrations applied");
    println!("Migrations applied.");
    Ok(())
}

async fn run_voices(config: &Config, http: reqwest::Client) -> anyhow::Result<()> {
    // voice_id is intentionally unused here; we only need the key + base URL.
    let synth = VoxtralSynthesizer::new(
        http,
        &config.mistral_base_url,
        &config.mistral_api_key,
        &config.tts_model,
        "",
    );
    let body = synth.list_voices().await.context("listing voices")?;

    // Pretty-print if it parses as JSON; otherwise show it raw.
    match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(json) => println!("{}", serde_json::to_string_pretty(&json)?),
        Err(_) => println!("{body}"),
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_convert(
    config: &Config,
    http: reqwest::Client,
    url: Option<String>,
    text: Option<String>,
    file: Option<String>,
    out: Option<String>,
    preview: bool,
) -> anyhow::Result<()> {
    let source = resolve_source(url, text, file)?;

    let extractor = RoutingExtractor::new(http.clone());
    let normalizer = RuleNormalizer::new()?;

    // Dry run: show what would be spoken, no API call, no cost.
    if preview {
        let p = audify_pipeline::preview(&source, &extractor, &normalizer)
            .await
            .context("previewing")?;
        println!(
            "Title: {:?}\nInput chars: {}\nChunks: {}\n--- speakable text ---\n{}",
            p.title, p.char_count, p.chunks, p.text
        );
        return Ok(());
    }

    let voice_id = config.require_voice_id()?;
    let key = out.unwrap_or_else(|| source_key(&source));

    let synthesizer = VoxtralSynthesizer::new(
        http,
        &config.mistral_base_url,
        &config.mistral_api_key,
        &config.tts_model,
        voice_id,
    );
    let storage = LocalStorage::new(&config.output_dir);

    let output = process(&source, &extractor, &normalizer, &synthesizer, &storage, &key)
        .await
        .context("running pipeline")?;

    tracing::info!(
        path = %output.path,
        chars = output.char_count,
        bytes = output.audio_bytes,
        "done"
    );
    println!("Wrote {} ({} chars in)", output.path, output.char_count);
    Ok(())
}

async fn run_submit(
    config: &Config,
    http: reqwest::Client,
    url: Option<String>,
    text: Option<String>,
    file: Option<String>,
) -> anyhow::Result<()> {
    let source = resolve_source(url, text, file)?;
    let database_url = config.require_database_url()?;
    let voice_id = config.require_voice_id()?.to_string();

    let pool = audify_db::connect(database_url)
        .await
        .context("connecting to database")?;
    let documents = audify_db::DocumentRepository::new(pool.clone());
    let episodes = audify_db::EpisodeRepository::new(pool.clone());
    let queue = audify_db::DbQueue::new(pool);
    let extractor = RoutingExtractor::new(http);

    let r = service::submit(&source, &extractor, &documents, &episodes, &queue, &voice_id).await?;

    tracing::info!(episode_id = %r.episode_id, job_id = %r.job_id, char_count = r.char_count, "submitted");
    println!(
        "Submitted episode {} (job {}) — {} chars, est ${:.4}. Run `audify worker` to process.",
        r.episode_id, r.job_id, r.char_count, r.est_cost
    );
    Ok(())
}

/// Resolve mutually exclusive source flags into a [`Source`].
fn resolve_source(
    url: Option<String>,
    text: Option<String>,
    file: Option<String>,
) -> anyhow::Result<Source> {
    match (url, text, file) {
        (Some(url), _, _) => Ok(Source::Url(url)),
        (_, Some(text), _) => Ok(Source::Text(text)),
        (_, _, Some(file)) => Ok(Source::File(PathBuf::from(file))),
        _ => bail!("provide --url <URL>, --text <TEXT>, or --file <PDF>"),
    }
}

/// Stable storage key derived from the source content (first 16 hex of SHA-256).
fn source_key(source: &Source) -> String {
    let material = match source {
        Source::Url(u) => u.clone(),
        Source::Text(t) => t.clone(),
        Source::File(p) => p.to_string_lossy().into_owned(),
    };
    let digest = Sha256::digest(material.as_bytes());
    digest
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .init();
}
