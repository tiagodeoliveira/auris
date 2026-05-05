//! SQLite persistence layer.
//!
//! One file (`<DATA_DIR>/server.db`) with migrations applied at boot.
//! `<DATA_DIR>` defaults to `./data` and can be overridden with the
//! `MEETING_COMPANION_DATA_DIR` env var; `<DATA_DIR>/blobs/` is
//! reserved for non-relational artefacts (transcripts, screenshots)
//! that future phases will write alongside this database.
//!
//! All write paths run inside small, focused transactions on the
//! `SqlitePool`. The pool itself is held by `ServerHandle`; intent
//! handlers reach for it after `apply_intent` returns, keeping the
//! `ServerState` mutex free of any I/O.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use tracing::info;

/// Absolute path to the data directory (`<DATA_DIR>` in the docs).
/// Resolves the env var, expands `~`, and creates the directory if
/// it doesn't exist yet. The same path will host the `blobs/`
/// subtree for non-DB artefacts in later phases.
pub fn data_dir() -> Result<PathBuf> {
    let raw = std::env::var("MEETING_COMPANION_DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    let expanded = if let Some(stripped) = raw.strip_prefix("~/") {
        let home = std::env::var("HOME").context("HOME not set; cannot expand ~")?;
        PathBuf::from(home).join(stripped)
    } else {
        PathBuf::from(raw)
    };
    std::fs::create_dir_all(&expanded)
        .with_context(|| format!("failed to create data dir at {}", expanded.display()))?;
    Ok(expanded)
}

/// Open the SQLite pool against `<DATA_DIR>/server.db` and run any
/// pending migrations. Idempotent on already-migrated databases.
pub async fn open_pool() -> Result<SqlitePool> {
    let dir = data_dir()?;
    let db_path = dir.join("server.db");
    let pool = open_pool_at(&db_path).await?;
    info!(path = %db_path.display(), "sqlite ready");
    Ok(pool)
}

/// Test/integration entrypoint: open against an arbitrary path
/// (e.g. `:memory:` via the connect options).
pub async fn open_pool_at(db_path: &Path) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await
        .with_context(|| format!("failed to open sqlite at {}", db_path.display()))?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("sqlx migrations failed")?;

    Ok(pool)
}

// MARK: - Meetings

/// Insert a meeting row. `metadata_json` is the already-serialised
/// JSON object (`HashMap<String, String>` → JSON object string).
pub async fn insert_meeting(
    pool: &SqlitePool,
    id: &str,
    started_at: DateTime<Utc>,
    description: Option<&str>,
    metadata_json: &str,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO meetings (id, started_at, description, metadata)
           VALUES (?1, ?2, ?3, ?4)"#,
    )
    .bind(id)
    .bind(started_at)
    .bind(description)
    .bind(metadata_json)
    .execute(pool)
    .await
    .with_context(|| format!("insert_meeting({id})"))?;
    Ok(())
}

/// Mark a meeting as ended at the given timestamp. No-op (silently)
/// if `meeting_id` doesn't exist or has already been ended.
pub async fn end_meeting(
    pool: &SqlitePool,
    meeting_id: &str,
    ended_at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        r#"UPDATE meetings
              SET ended_at = ?1
            WHERE id = ?2 AND ended_at IS NULL"#,
    )
    .bind(ended_at)
    .bind(meeting_id)
    .execute(pool)
    .await
    .with_context(|| format!("end_meeting({meeting_id})"))?;
    Ok(())
}

// MARK: - Moments

/// Insert a moment for the given meeting. `t` is the millisecond
/// offset from meeting start, matching the wire `MarkMoment { t, note }`.
/// Returns the generated moment id.
pub async fn insert_moment(
    pool: &SqlitePool,
    meeting_id: &str,
    t: i64,
    note: Option<&str>,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        r#"INSERT INTO moments (id, meeting_id, t, note)
           VALUES (?1, ?2, ?3, ?4)"#,
    )
    .bind(&id)
    .bind(meeting_id)
    .bind(t)
    .bind(note)
    .execute(pool)
    .await
    .with_context(|| format!("insert_moment(meeting={meeting_id}, t={t})"))?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        // `:memory:` is per-connection in SQLite; SqlitePool would
        // share isolated DBs across its 8 connections. Use
        // `mode=memory&cache=shared` via a temp path-like name so
        // every checkout sees the same in-memory DB.
        let opts = SqliteConnectOptions::new()
            .filename(":memory:")
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1) // single conn → genuine in-memory isolation
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn insert_then_end_meeting_round_trips() {
        let pool = pool().await;
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        insert_meeting(&pool, &id, now, Some("daily standup"), "{}")
            .await
            .unwrap();
        end_meeting(&pool, &id, now).await.unwrap();

        let row: (String, Option<String>) =
            sqlx::query_as("SELECT id, ended_at FROM meetings WHERE id = ?1")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, id);
        assert!(
            row.1.is_some(),
            "ended_at should be populated after end_meeting"
        );
    }

    #[tokio::test]
    async fn insert_moment_links_to_meeting() {
        let pool = pool().await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, Utc::now(), None, "{}")
            .await
            .unwrap();

        let mom = insert_moment(&pool, &mid, 1500, Some("breakthrough"))
            .await
            .unwrap();

        let (got_meeting_id, got_t, got_note): (String, i64, Option<String>) =
            sqlx::query_as("SELECT meeting_id, t, note FROM moments WHERE id = ?1")
                .bind(&mom)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(got_meeting_id, mid);
        assert_eq!(got_t, 1500);
        assert_eq!(got_note.as_deref(), Some("breakthrough"));
    }

    #[tokio::test]
    async fn moment_fk_blocks_orphan_inserts() {
        let pool = pool().await;
        // Inserting a moment without a meeting row should fail the FK.
        let res = insert_moment(&pool, "no-such-meeting", 0, None).await;
        assert!(res.is_err(), "expected FK violation on orphan moment");
    }
}
