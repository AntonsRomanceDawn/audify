//! HTTP API (Axum). Handlers stay thin — they call repositories/services and
//! map to responses; no business logic lives here.

use std::path::Path as FsPath;
use std::time::Duration;

use anyhow::Context;
use axum::{
    extract::{Path, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use tower::{ServiceBuilder, ServiceExt};
use tower_http::{
    compression::CompressionLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    services::ServeFile,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};
use uuid::Uuid;

use audify_core::{Config, Source};
use audify_db::{DbQueue, DocumentRepository, EpisodeRepository, EpisodeRow, FeedRow};
use audify_pipeline::extract::RoutingExtractor;

use crate::service;

/// Shared state handed to every handler.
#[derive(Clone)]
struct AppState {
    documents: DocumentRepository,
    episodes: EpisodeRepository,
    queue: DbQueue,
    http: reqwest::Client,
    voice_id: String,
    output_dir: String,
    public_base_url: String,
    /// (user, pass) for Basic Auth on feed/stream; `None` disables auth.
    basic_auth: Option<(String, String)>,
}

/// Build the server from config and serve until shutdown.
pub async fn serve(config: &Config, http: reqwest::Client) -> anyhow::Result<()> {
    let database_url = config.require_database_url()?;
    let voice_id = config.require_voice_id()?.to_string();
    let pool = audify_db::connect(database_url)
        .await
        .context("connecting to database")?;

    let state = AppState {
        documents: DocumentRepository::new(pool.clone()),
        episodes: EpisodeRepository::new(pool.clone()),
        queue: DbQueue::new(pool),
        http,
        voice_id,
        output_dir: config.output_dir.clone(),
        public_base_url: config.public_base_url.trim_end_matches('/').to_string(),
        basic_auth: match (&config.basic_auth_user, &config.basic_auth_pass) {
            (Some(u), Some(p)) => Some((u.clone(), p.clone())),
            _ => {
                tracing::warn!("basic auth disabled (AUDIFY_BASIC_AUTH_USER/PASS not set)");
                None
            }
        },
    };

    let app = router(state);
    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .with_context(|| format!("binding {}", config.bind_addr))?;
    tracing::info!(addr = %config.bind_addr, "listening");
    axum::serve(listener, app).await.context("server error")?;
    Ok(())
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/", get(index))
        .route("/feed.xml", get(feed))
        .route("/api/articles", post(create_article))
        .route("/api/episodes", get(list_episodes))
        .route("/api/episodes/{id}", get(get_episode))
        .route("/api/episodes/{id}/stream", get(stream_episode))
        // Runs for all routes; enforces Basic Auth only on the podcast-facing ones.
        .route_layer(middleware::from_fn_with_state(state.clone(), basic_auth))
        .with_state(state)
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
                .layer(TraceLayer::new_for_http())
                .layer(TimeoutLayer::with_status_code(
                    StatusCode::REQUEST_TIMEOUT,
                    Duration::from_secs(30),
                ))
                .layer(CompressionLayer::new())
                .layer(PropagateRequestIdLayer::x_request_id()),
        )
}

async fn health() -> &'static str {
    "ok"
}

/// Minimal submit/listen page (behind the same Basic Auth as everything else).
async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

const INDEX_HTML: &str = r###"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Audify</title>
<style>
  body { font-family: system-ui, sans-serif; max-width: 640px; margin: 40px auto; padding: 0 16px; background:#111; color:#eee; }
  h1 { font-weight: 600; } h2 { font-weight: 500; font-size: 18px; margin-top: 28px; }
  input, button { font-size: 16px; padding: 10px; border-radius: 8px; border: 1px solid #333; }
  input { width: 100%; background:#1c1c1c; color:#eee; box-sizing:border-box; }
  button { background:#3a6df0; color:#fff; border:none; cursor:pointer; margin-top:8px; }
  #msg { margin-top:12px; min-height:20px; color:#9fb4ff; }
  ul { list-style:none; padding:0; }
  li { padding:10px 0; border-bottom:1px solid #222; display:flex; justify-content:space-between; align-items:center; gap:8px; }
  .status { font-size:13px; color:#999; }
  a { color:#9fb4ff; }
  .feed { margin-top:24px; font-size:13px; color:#777; }
</style>
</head>
<body>
  <h1>Audify</h1>
  <input id="src" placeholder="Paste an article URL (or raw text)…" autofocus>
  <button id="add">Add to podcast</button>
  <div id="msg"></div>
  <h2>Episodes</h2>
  <ul id="list"></ul>
  <div class="feed">Subscribe in a podcast app: <code>/feed.xml</code></div>
<script>
const msg = document.getElementById('msg');
async function add() {
  const v = document.getElementById('src').value.trim();
  if (!v) return;
  const body = /^https?:\/\//i.test(v) ? { url: v } : { text: v };
  msg.textContent = 'Submitting…';
  try {
    const r = await fetch('/api/articles', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    if (!r.ok) { const t = await r.json().catch(() => ({})); msg.textContent = 'Error: ' + (t.error || r.status); return; }
    const j = await r.json();
    msg.textContent = 'Queued ' + j.episode_id.slice(0, 8) + ' — rendering…';
    document.getElementById('src').value = '';
    load();
  } catch (e) { msg.textContent = 'Error: ' + e; }
}
async function load() {
  try {
    const r = await fetch('/api/episodes?limit=20');
    if (!r.ok) return;
    const eps = await r.json();
    document.getElementById('list').innerHTML = eps.map(e =>
      '<li><span class="status">' + e.status + (e.duration_sec ? ' &middot; ' + e.duration_sec + 's' : '') + '</span>' +
      (e.stream_url ? '<a href="' + e.stream_url + '">&#9654; play</a>' : '<span class="status">&hellip;</span>') +
      '</li>').join('') || '<li class="status">No episodes yet.</li>';
  } catch (e) {}
}
document.getElementById('add').addEventListener('click', add);
document.getElementById('src').addEventListener('keydown', e => { if (e.key === 'Enter') add(); });
load();
setInterval(load, 5000);
</script>
</body>
</html>"###;

/// Enforce HTTP Basic Auth on podcast-facing routes when credentials are set.
async fn basic_auth(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    // Everything except the health check requires auth (when configured): the
    // feed, audio, and the submit/list endpoints (so nobody can run up the bill).
    let is_protected = request.uri().path() != "/health";

    if is_protected {
        if let Some((user, pass)) = &state.basic_auth {
            let presented = request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|h| h.to_str().ok())
                .and_then(|h| h.strip_prefix("Basic "))
                .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
                .and_then(|bytes| String::from_utf8(bytes).ok());
            let expected = format!("{user}:{pass}");
            if presented.as_deref() != Some(expected.as_str()) {
                return Err(ApiError::Unauthorized);
            }
        }
    }
    Ok(next.run(request).await)
}

// ---- episodes ----

#[derive(Deserialize)]
struct Pagination {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_limit() -> i64 {
    50
}

async fn list_episodes(
    State(state): State<AppState>,
    Query(page): Query<Pagination>,
) -> Result<Json<Vec<EpisodeDto>>, ApiError> {
    let rows = state
        .episodes
        .list(page.limit.clamp(1, 200), page.offset.max(0))
        .await?;
    let dtos = rows
        .into_iter()
        .map(|r| EpisodeDto::from_row(r, &state.public_base_url))
        .collect();
    Ok(Json(dtos))
}

async fn get_episode(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<EpisodeDto>, ApiError> {
    let row = state.episodes.get(id).await?.ok_or(ApiError::NotFound)?;
    Ok(Json(EpisodeDto::from_row(row, &state.public_base_url)))
}

/// Serve the rendered MP3 with byte-range support (handled by `ServeFile`).
async fn stream_episode(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    request: Request,
) -> Result<Response, ApiError> {
    let episode = state.episodes.get(id).await?.ok_or(ApiError::NotFound)?;
    if episode.audio_path.is_none() {
        return Err(ApiError::NotFound); // not rendered yet
    }
    // Resolve from the output dir + id, avoiding the OS-specific stored path.
    let path = FsPath::new(&state.output_dir).join(format!("{id}.mp3"));
    Ok(ServeFile::new(path).oneshot(request).await.into_response())
}

// ---- submit ----

#[derive(Deserialize)]
struct CreateArticle {
    url: Option<String>,
    text: Option<String>,
}

#[derive(Serialize)]
struct SubmitDto {
    episode_id: Uuid,
    job_id: Uuid,
}

async fn create_article(
    State(state): State<AppState>,
    Json(req): Json<CreateArticle>,
) -> Result<(StatusCode, Json<SubmitDto>), ApiError> {
    let source = match (req.url, req.text) {
        (Some(url), _) => Source::Url(url),
        (_, Some(text)) => Source::Text(text),
        _ => return Err(ApiError::BadRequest("provide 'url' or 'text'".into())),
    };
    let extractor = RoutingExtractor::new(state.http.clone());
    // Submission failures are almost always content issues (bad URL, no
    // extractable text) — surface the reason to the caller, not an opaque 500.
    let r = service::submit(
        &source,
        &extractor,
        &state.documents,
        &state.episodes,
        &state.queue,
        &state.voice_id,
    )
    .await
    .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(SubmitDto {
            episode_id: r.episode_id,
            job_id: r.job_id,
        }),
    ))
}

// ---- RSS feed ----

async fn feed(State(state): State<AppState>) -> Result<Response, ApiError> {
    let rows = state.episodes.list_ready_for_feed(200).await?;
    let xml = build_rss(&rows, &state.public_base_url, &state.output_dir);
    Ok((
        [(header::CONTENT_TYPE, "application/rss+xml; charset=utf-8")],
        xml,
    )
        .into_response())
}

fn build_rss(rows: &[FeedRow], base_url: &str, output_dir: &str) -> String {
    let mut items = String::new();
    for r in rows {
        let title = xml_escape(r.title.as_deref().unwrap_or("Untitled"));
        let length = std::fs::metadata(FsPath::new(output_dir).join(format!("{}.mp3", r.id)))
            .map(|m| m.len())
            .unwrap_or(0);
        let pub_date = r.created_at.to_rfc2822();
        let duration = r.duration_sec.unwrap_or(0);
        items.push_str(&format!(
            "  <item>\n    \
             <title>{title}</title>\n    \
             <guid isPermaLink=\"false\">{id}</guid>\n    \
             <pubDate>{pub_date}</pubDate>\n    \
             <enclosure url=\"{base_url}/api/episodes/{id}/stream\" length=\"{length}\" type=\"audio/mpeg\"/>\n    \
             <itunes:duration>{duration}</itunes:duration>\n  \
             </item>\n",
            id = r.id,
        ));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <rss version=\"2.0\" xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\">\n\
         <channel>\n  \
         <title>Audify</title>\n  \
         <link>{base_url}</link>\n  \
         <description>Your articles, read aloud.</description>\n  \
         <language>en</language>\n\
         {items}</channel>\n</rss>\n"
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ---- shared DTO + errors ----

/// Client-facing episode shape.
#[derive(Serialize)]
struct EpisodeDto {
    id: Uuid,
    status: String,
    voice_id: String,
    char_count: i32,
    duration_sec: Option<i32>,
    created_at: String,
    /// Present once the audio is ready.
    stream_url: Option<String>,
}

impl EpisodeDto {
    fn from_row(row: EpisodeRow, base_url: &str) -> Self {
        let stream_url = row
            .audio_path
            .as_ref()
            .map(|_| format!("{base_url}/api/episodes/{}/stream", row.id));
        Self {
            id: row.id,
            status: row.status,
            voice_id: row.voice_id,
            char_count: row.char_count,
            duration_sec: row.duration_sec,
            created_at: row.created_at.to_rfc3339(),
            stream_url,
        }
    }
}

/// API error mapped to an HTTP response.
enum ApiError {
    NotFound,
    BadRequest(String),
    Unauthorized,
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if let ApiError::Unauthorized = self {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Basic realm=\"audify\"")],
                Json(serde_json::json!({ "error": "unauthorized" })),
            )
                .into_response();
        }
        let (status, message) = match self {
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Unauthorized => unreachable!("handled above"),
            ApiError::Internal(e) => {
                tracing::error!(error = %e, "request failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
            }
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError::Internal(e)
    }
}

impl From<audify_core::Error> for ApiError {
    fn from(e: audify_core::Error) -> Self {
        ApiError::Internal(e.into())
    }
}
