//! Repository pattern: one struct per entity. No raw SQL leaks outside this module.
//!
//! Uses sqlx's compile-time-checked `query!`/`query_as!` macros: every statement
//! is verified against the live schema at build time. Builds without a database
//! use the committed `.sqlx/` offline cache (run `cargo sqlx prepare` after
//! changing any query).
//!
//! Note on the `as "col!"` annotations: Postgres reports every column of an
//! `INSERT ... RETURNING` as nullable, so non-null columns are forced with `!`.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use audify_core::{EpisodeStatus, Error, Result};

/// A row from the `documents` table.
#[derive(Debug, Clone)]
pub struct DocumentRow {
    pub id: Uuid,
    pub source_type: String,
    pub source_ref: String,
    pub content_hash: String,
    pub title: Option<String>,
    pub struct_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// Persists extracted documents, deduplicated by content hash.
#[derive(Clone)]
pub struct DocumentRepository {
    pool: PgPool,
}

impl DocumentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert a document, or return the existing one with the same content hash.
    /// This is the dedup point: the same source submitted twice maps to one row.
    pub async fn upsert(
        &self,
        source_type: &str,
        source_ref: &str,
        content_hash: &str,
        title: Option<&str>,
        struct_json: &serde_json::Value,
    ) -> Result<DocumentRow> {
        sqlx::query_as!(
            DocumentRow,
            r#"
            INSERT INTO documents (source_type, source_ref, content_hash, title, struct_json)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (content_hash) DO UPDATE SET title = EXCLUDED.title
            RETURNING
                id AS "id!",
                source_type AS "source_type!",
                source_ref AS "source_ref!",
                content_hash AS "content_hash!",
                title,
                struct_json AS "struct_json!: serde_json::Value",
                created_at AS "created_at!"
            "#,
            source_type,
            source_ref,
            content_hash,
            title,
            struct_json,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("upsert document: {e}")))
    }

    pub async fn find_by_hash(&self, content_hash: &str) -> Result<Option<DocumentRow>> {
        sqlx::query_as!(
            DocumentRow,
            r#"
            SELECT id, source_type, source_ref, content_hash, title,
                   struct_json AS "struct_json!: serde_json::Value", created_at
            FROM documents WHERE content_hash = $1
            "#,
            content_hash,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("find document by hash: {e}")))
    }
}

/// A row from the `episodes` table. `status` is stored as text; use
/// [`EpisodeRow::status`] to get the typed value.
#[derive(Debug, Clone)]
pub struct EpisodeRow {
    pub id: Uuid,
    pub document_id: Uuid,
    pub status: String,
    pub voice_id: String,
    pub char_count: i32,
    pub duration_sec: Option<i32>,
    pub audio_path: Option<String>,
    pub est_cost_usd: f64,
    pub created_at: DateTime<Utc>,
}

impl EpisodeRow {
    /// Parse the stored status string into the typed enum.
    pub fn status(&self) -> Result<EpisodeStatus> {
        self.status.parse()
    }
}

/// Persists episodes and their lifecycle.
#[derive(Clone)]
pub struct EpisodeRepository {
    pool: PgPool,
}

impl EpisodeRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create a new episode in the `pending` state.
    pub async fn create(
        &self,
        document_id: Uuid,
        voice_id: &str,
        char_count: i32,
        est_cost_usd: f64,
    ) -> Result<EpisodeRow> {
        sqlx::query_as!(
            EpisodeRow,
            r#"
            INSERT INTO episodes (document_id, status, voice_id, char_count, est_cost_usd)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING
                id AS "id!",
                document_id AS "document_id!",
                status AS "status!",
                voice_id AS "voice_id!",
                char_count AS "char_count!",
                duration_sec,
                audio_path,
                est_cost_usd AS "est_cost_usd!",
                created_at AS "created_at!"
            "#,
            document_id,
            EpisodeStatus::Pending.as_str(),
            voice_id,
            char_count,
            est_cost_usd,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("create episode: {e}")))
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<EpisodeRow>> {
        sqlx::query_as!(
            EpisodeRow,
            r#"
            SELECT id, document_id, status, voice_id, char_count, duration_sec,
                   audio_path, est_cost_usd, created_at
            FROM episodes WHERE id = $1
            "#,
            id,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("get episode: {e}")))
    }

    /// List episodes, newest first, with pagination.
    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<EpisodeRow>> {
        sqlx::query_as!(
            EpisodeRow,
            r#"
            SELECT id, document_id, status, voice_id, char_count, duration_sec,
                   audio_path, est_cost_usd, created_at
            FROM episodes ORDER BY created_at DESC LIMIT $1 OFFSET $2
            "#,
            limit,
            offset,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("list episodes: {e}")))
    }

    pub async fn set_status(&self, id: Uuid, status: EpisodeStatus) -> Result<()> {
        sqlx::query!(
            "UPDATE episodes SET status = $2 WHERE id = $1",
            id,
            status.as_str(),
        )
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("set episode status: {e}")))?;
        Ok(())
    }

    /// Mark an episode ready with its final audio path and duration.
    pub async fn set_ready(&self, id: Uuid, audio_path: &str, duration_sec: i32) -> Result<()> {
        sqlx::query!(
            "UPDATE episodes SET status = $2, audio_path = $3, duration_sec = $4 WHERE id = $1",
            id,
            EpisodeStatus::Ready.as_str(),
            audio_path,
            duration_sec,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("set episode ready: {e}")))?;
        Ok(())
    }
}
