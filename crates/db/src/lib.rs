//! Database access for Audify: connection pool, migrations, and repositories.
//!
//! Uses sqlx with *runtime* queries (not the compile-time `query!` macros), so
//! the crate builds without a live database — the DB is only needed to run.
//! Compile-time checking can be layered on later by switching to `query!` plus a
//! committed offline cache, with no change to the repository API.

pub mod repo;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use audify_core::{Error, Result};

pub use repo::{
    ChunkRepository, ChunkRow, DbQueue, DocumentRepository, DocumentRow, EpisodeRepository,
    EpisodeRow, FeedRow, JobRepository,
};

/// Connect to Postgres and return a pooled handle.
pub async fn connect(database_url: &str) -> Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await
        .map_err(|e| Error::Database(format!("connecting to database: {e}")))
}

/// Apply all pending migrations. Migrations are embedded at compile time, so a
/// live database is only required at run time.
pub async fn migrate(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("../../db/migrations")
        .run(pool)
        .await
        .map_err(|e| Error::Database(format!("running migrations: {e}")))?;
    Ok(())
}
