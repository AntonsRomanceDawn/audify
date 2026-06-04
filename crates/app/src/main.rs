//! Audify CLI — Phase 1 entrypoint.
//!
//! `audify convert --url https://example.com/article` runs the full pipeline and
//! writes an audio file. `audify voices` lists the preset voice ids your account
//! can use (and confirms the API key + endpoint work).

use std::time::Duration;

use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};
use tracing_subscriber::EnvFilter;

use audify_core::{Config, Source};
use audify_pipeline::{
    extract::ArticleExtractor, normalize::RuleNormalizer, process, storage::LocalStorage,
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
    /// Convert an article (URL or raw text) into an audio file.
    Convert {
        /// URL of an article to fetch and convert.
        #[arg(long, conflicts_with = "text")]
        url: Option<String>,
        /// Raw text to convert directly.
        #[arg(long)]
        text: Option<String>,
        /// Output file name (without extension). Defaults to a hash of the source.
        #[arg(long)]
        out: Option<String>,
    },
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
        Command::Voices => run_voices(&config, http_client(&config)?).await,
        Command::Convert { url, text, out } => {
            run_convert(&config, http_client(&config)?, url, text, out).await
        }
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

async fn run_convert(
    config: &Config,
    http: reqwest::Client,
    url: Option<String>,
    text: Option<String>,
    out: Option<String>,
) -> anyhow::Result<()> {
    let source = match (url, text) {
        (Some(url), _) => Source::Url(url),
        (None, Some(text)) => Source::Text(text),
        (None, None) => bail!("provide --url <URL> or --text <TEXT>"),
    };

    let format = config.audio_format()?;
    let voice_id = config.require_voice_id()?;
    let key = out.unwrap_or_else(|| source_key(&source));

    let extractor = ArticleExtractor::new(http.clone());
    let normalizer = RuleNormalizer::new()?;
    let synthesizer = VoxtralSynthesizer::new(
        http,
        &config.mistral_base_url,
        &config.mistral_api_key,
        &config.tts_model,
        voice_id,
    );
    let storage = LocalStorage::new(&config.output_dir);

    let output = process(
        &source,
        &extractor,
        &normalizer,
        &synthesizer,
        &storage,
        format,
        &key,
    )
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

/// Stable storage key derived from the source content (first 16 hex of SHA-256).
fn source_key(source: &Source) -> String {
    let material = match source {
        Source::Url(u) => u.as_str(),
        Source::Text(t) => t.as_str(),
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
