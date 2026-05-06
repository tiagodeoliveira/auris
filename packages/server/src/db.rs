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

// MARK: - Users

/// Server-internal user row. `id` is the UUID we mint; `auth0_sub`
/// is the stable identity from Auth0 ("auth0|...", "google-oauth2|...",
/// etc.). The schema keeps `email` + `name` as best-effort copies of
/// what Auth0 returned at the most recent login.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserRow {
    pub id: String,
    pub auth0_sub: String,
    pub email: Option<String>,
    pub name: Option<String>,
    #[allow(dead_code)]
    pub created_at: DateTime<Utc>,
    #[allow(dead_code)]
    pub last_seen_at: DateTime<Utc>,
}

/// Find or create a `users` row matching `auth0_sub`. Updates `email`,
/// `name`, and `last_seen_at` on every call so the local mirror tracks
/// whatever the most recent JWT claimed (Auth0 is authoritative for
/// these — we just keep a copy for offline reads).
///
/// Returns the row in either case so callers always get the local `id`
/// to scope their writes against.
pub async fn upsert_user_by_auth0_sub(
    pool: &SqlitePool,
    auth0_sub: &str,
    email: Option<&str>,
    name: Option<&str>,
) -> Result<UserRow> {
    // Try the read-side first — the steady state is "user already
    // exists, refresh their fields." A single UPSERT could collapse
    // these but `INSERT ... ON CONFLICT` with `RETURNING` requires a
    // schema we'd rather not couple to.
    let existing: Option<UserRow> = sqlx::query_as(
        r#"SELECT id, auth0_sub, email, name, created_at, last_seen_at
             FROM users WHERE auth0_sub = ?1"#,
    )
    .bind(auth0_sub)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("upsert_user.lookup({auth0_sub})"))?;

    if let Some(row) = existing {
        sqlx::query(
            r#"UPDATE users
                  SET email = COALESCE(?2, email),
                      name = COALESCE(?3, name),
                      last_seen_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                WHERE id = ?1"#,
        )
        .bind(&row.id)
        .bind(email)
        .bind(name)
        .execute(pool)
        .await
        .with_context(|| format!("upsert_user.refresh({auth0_sub})"))?;
        // Re-read so the returned row reflects the freshly-bumped
        // `last_seen_at` and any updated email/name.
        let refreshed: UserRow = sqlx::query_as(
            r#"SELECT id, auth0_sub, email, name, created_at, last_seen_at
                 FROM users WHERE id = ?1"#,
        )
        .bind(&row.id)
        .fetch_one(pool)
        .await
        .with_context(|| format!("upsert_user.reread({auth0_sub})"))?;
        return Ok(refreshed);
    }

    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        r#"INSERT INTO users (id, auth0_sub, email, name)
           VALUES (?1, ?2, ?3, ?4)"#,
    )
    .bind(&id)
    .bind(auth0_sub)
    .bind(email)
    .bind(name)
    .execute(pool)
    .await
    .with_context(|| format!("upsert_user.insert({auth0_sub})"))?;
    sqlx::query_as(
        r#"SELECT id, auth0_sub, email, name, created_at, last_seen_at
             FROM users WHERE id = ?1"#,
    )
    .bind(&id)
    .fetch_one(pool)
    .await
    .with_context(|| format!("upsert_user.reread_new({auth0_sub})"))
}

// MARK: - Meetings

/// Insert a meeting row owned by `user_id`. `metadata_json` is the
/// already-serialised JSON object (`HashMap<String, String>` → JSON
/// object string).
pub async fn insert_meeting(
    pool: &SqlitePool,
    id: &str,
    user_id: &str,
    started_at: DateTime<Utc>,
    description: Option<&str>,
    metadata_json: &str,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO meetings (id, user_id, started_at, description, metadata)
           VALUES (?1, ?2, ?3, ?4, ?5)"#,
    )
    .bind(id)
    .bind(user_id)
    .bind(started_at)
    .bind(description)
    .bind(metadata_json)
    .execute(pool)
    .await
    .with_context(|| format!("insert_meeting({id})"))?;
    Ok(())
}

/// Find the most recent meeting whose `ended_at` is NULL. There
/// should be at most one in normal operation (we clear the field
/// on `stop_meeting`); if multiple exist due to crash sequencing
/// we pick the newest by `started_at` and ignore older ones —
/// boot recovery covers the most likely "the user was mid-meeting
/// when the server died" case.
///
/// Returns `(id, description, metadata_json, started_at)` or `None`.
pub async fn find_active_meeting(
    pool: &SqlitePool,
) -> Result<Option<(String, Option<String>, String, DateTime<Utc>)>> {
    let row: Option<(String, Option<String>, String, DateTime<Utc>)> = sqlx::query_as(
        r#"SELECT id, description, metadata, started_at
             FROM meetings
            WHERE ended_at IS NULL
            ORDER BY started_at DESC
            LIMIT 1"#,
    )
    .fetch_optional(pool)
    .await
    .context("find_active_meeting")?;
    Ok(row)
}

/// Find every unfinished meeting + its owner, one row per. Boot
/// recovery iterates this and re-spawns each user's pipeline.
/// Pre-OAuth rows have `user_id IS NULL` — the column was nullable
/// in migration 0003 to keep the schema buildable through the
/// transition. We skip those (no user → no UserState to attach to).
pub async fn find_active_meetings_per_user(
    pool: &SqlitePool,
) -> Result<Vec<(String, String, Option<String>, String, DateTime<Utc>)>> {
    // (user_id, meeting_id, description, metadata_json, started_at)
    let rows = sqlx::query_as(
        r#"SELECT user_id, id, description, metadata, started_at
             FROM meetings
            WHERE ended_at IS NULL AND user_id IS NOT NULL
            ORDER BY user_id, started_at DESC"#,
    )
    .fetch_all(pool)
    .await
    .context("find_active_meetings_per_user")?;
    Ok(rows)
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

/// Delete a meeting only if it belongs to `user_id`. The `moments`
/// foreign key has `ON DELETE CASCADE`, so this single statement
/// removes the moments rows too. Disk-side blob cleanup is the
/// caller's responsibility (see `api::delete_meeting`).
///
/// Returns `Ok(true)` if a row was actually removed, `Ok(false)`
/// when the id wasn't found *or* was owned by someone else — the
/// API surfaces both as 404 to avoid leaking existence.
pub async fn delete_meeting_for_user(
    pool: &SqlitePool,
    meeting_id: &str,
    user_id: &str,
) -> Result<bool> {
    let res = sqlx::query(r#"DELETE FROM meetings WHERE id = ?1 AND user_id = ?2"#)
        .bind(meeting_id)
        .bind(user_id)
        .execute(pool)
        .await
        .with_context(|| format!("delete_meeting_for_user({meeting_id})"))?;
    Ok(res.rows_affected() > 0)
}

// MARK: - Moments

/// Insert a moment for the given meeting. `t` is the millisecond
/// offset from meeting start. `kind` is the discriminator for the
/// moment-creation mode ("manual" today; future modes might use
/// "interview" etc.) — the async summary worker dispatches on it.
/// `id` is supplied by the caller so client-side workflows can
/// pre-mint and use the id for the screenshot path before the
/// row exists; pass a freshly-minted UUID if there's no preference.
pub async fn insert_moment(
    pool: &SqlitePool,
    id: &str,
    meeting_id: &str,
    kind: &str,
    t: i64,
    note: Option<&str>,
    asset_path: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO moments (id, meeting_id, kind, t, note, asset_path)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
    )
    .bind(id)
    .bind(meeting_id)
    .bind(kind)
    .bind(t)
    .bind(note)
    .bind(asset_path)
    .execute(pool)
    .await
    .with_context(|| format!("insert_moment(id={id}, meeting={meeting_id}, t={t})"))?;
    Ok(())
}

/// Replace `summary` + flip `summary_status` to `done` (or `failed`).
/// Called by the moment-summary worker after the LLM round trip.
pub async fn update_moment_summary(
    pool: &SqlitePool,
    moment_id: &str,
    summary: Option<&str>,
    status: &str,
) -> Result<()> {
    sqlx::query(
        r#"UPDATE moments
              SET summary = ?1, summary_status = ?2
            WHERE id = ?3"#,
    )
    .bind(summary)
    .bind(status)
    .bind(moment_id)
    .execute(pool)
    .await
    .with_context(|| format!("update_moment_summary(id={moment_id})"))?;
    Ok(())
}

/// Delete a single moment row only if its owning meeting belongs
/// to `user_id`. Returns `Ok(Some(asset_path))` on success (with
/// the screenshot path the caller should remove from disk),
/// `Ok(None)` if the row didn't exist *or* the meeting belongs to
/// another user (API surfaces both as 404).
pub async fn delete_moment_for_user(
    pool: &SqlitePool,
    moment_id: &str,
    user_id: &str,
) -> Result<Option<Option<String>>> {
    // Read asset_path + ownership in one shot so we can fail-fast
    // without locking, and so the caller knows what file to remove.
    let asset_path: Option<Option<String>> = sqlx::query_scalar(
        r#"SELECT m.asset_path
             FROM moments m
             JOIN meetings me ON me.id = m.meeting_id
            WHERE m.id = ?1 AND me.user_id = ?2"#,
    )
    .bind(moment_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("delete_moment_for_user.read({moment_id})"))?;
    if asset_path.is_none() {
        return Ok(None);
    }
    let res = sqlx::query(
        r#"DELETE FROM moments
            WHERE id = ?1
              AND meeting_id IN (SELECT id FROM meetings WHERE user_id = ?2)"#,
    )
    .bind(moment_id)
    .bind(user_id)
    .execute(pool)
    .await
    .with_context(|| format!("delete_moment_for_user({moment_id})"))?;
    if res.rows_affected() == 0 {
        return Ok(None);
    }
    Ok(asset_path)
}

/// Set or replace a moment's `asset_path`. Used by the late-binding
/// screenshot upload endpoint that lands an image after a WS-initiated
/// `mark_moment` already created the row.
pub async fn update_moment_asset_path(
    pool: &SqlitePool,
    moment_id: &str,
    asset_path: &str,
) -> Result<()> {
    sqlx::query(r#"UPDATE moments SET asset_path = ?1 WHERE id = ?2"#)
        .bind(asset_path)
        .bind(moment_id)
        .execute(pool)
        .await
        .with_context(|| format!("update_moment_asset_path(id={moment_id})"))?;
    Ok(())
}

/// Row shape for `list_moments_for_meeting`. `asset_path` is the
/// relative path under `<DATA_DIR>/blobs/...` (or NULL); the REST
/// endpoint maps it to a `/screenshot` URL clients can fetch.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MomentRow {
    pub id: String,
    pub meeting_id: String,
    pub kind: String,
    pub t: i64,
    pub note: Option<String>,
    pub asset_path: Option<String>,
    pub summary: Option<String>,
    pub summary_status: String,
    pub created_at: DateTime<Utc>,
}

/// List moments for a meeting, oldest first (`t ASC` so the order
/// matches the meeting's natural timeline).
pub async fn list_moments_for_meeting(
    pool: &SqlitePool,
    meeting_id: &str,
) -> Result<Vec<MomentRow>> {
    let rows = sqlx::query_as::<_, MomentRow>(
        r#"SELECT id, meeting_id, kind, t, note, asset_path,
                  summary, summary_status, created_at
             FROM moments
            WHERE meeting_id = ?1
            ORDER BY t ASC"#,
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("list_moments_for_meeting({meeting_id})"))?;
    Ok(rows)
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

    /// Most DB tests need an owner user to satisfy the meetings FK.
    /// Upsert a fixed test user here to keep each `#[tokio::test]`
    /// terse.
    async fn test_user(pool: &SqlitePool) -> String {
        upsert_user_by_auth0_sub(pool, "test|owner", None, None)
            .await
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn insert_then_end_meeting_round_trips() {
        let pool = pool().await;
        let uid = test_user(&pool).await;
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        insert_meeting(&pool, &id, &uid, now, Some("daily standup"), "{}")
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
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, Utc::now(), None, "{}")
            .await
            .unwrap();

        let moment_id = uuid::Uuid::new_v4().to_string();
        insert_moment(
            &pool,
            &moment_id,
            &mid,
            "manual",
            1500,
            Some("breakthrough"),
            Some("blobs/meetings/X/screenshots/Y.png"),
        )
        .await
        .unwrap();

        let row: (String, String, i64, Option<String>, Option<String>, String) = sqlx::query_as(
            r#"SELECT meeting_id, kind, t, note, asset_path, summary_status
                 FROM moments WHERE id = ?1"#,
        )
        .bind(&moment_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, mid);
        assert_eq!(row.1, "manual");
        assert_eq!(row.2, 1500);
        assert_eq!(row.3.as_deref(), Some("breakthrough"));
        assert_eq!(row.4.as_deref(), Some("blobs/meetings/X/screenshots/Y.png"));
        assert_eq!(row.5, "pending", "summary should start as pending");
    }

    #[tokio::test]
    async fn moment_fk_blocks_orphan_inserts() {
        let pool = pool().await;
        let id = uuid::Uuid::new_v4().to_string();
        let res = insert_moment(&pool, &id, "no-such-meeting", "manual", 0, None, None).await;
        assert!(res.is_err(), "expected FK violation on orphan moment");
    }

    #[tokio::test]
    async fn update_moment_summary_round_trips() {
        let pool = pool().await;
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, Utc::now(), None, "{}")
            .await
            .unwrap();
        let moment_id = uuid::Uuid::new_v4().to_string();
        insert_moment(&pool, &moment_id, &mid, "manual", 0, None, None)
            .await
            .unwrap();

        update_moment_summary(&pool, &moment_id, Some("Summary text"), "done")
            .await
            .unwrap();

        let row: (Option<String>, String) =
            sqlx::query_as("SELECT summary, summary_status FROM moments WHERE id = ?1")
                .bind(&moment_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0.as_deref(), Some("Summary text"));
        assert_eq!(row.1, "done");
    }

    #[tokio::test]
    async fn list_moments_returns_oldest_first() {
        let pool = pool().await;
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, Utc::now(), None, "{}")
            .await
            .unwrap();
        let later = uuid::Uuid::new_v4().to_string();
        let earlier = uuid::Uuid::new_v4().to_string();
        // Insert later first to confirm ordering by `t` (not insert order).
        insert_moment(&pool, &later, &mid, "manual", 5000, None, None)
            .await
            .unwrap();
        insert_moment(&pool, &earlier, &mid, "manual", 1000, None, None)
            .await
            .unwrap();
        let rows = list_moments_for_meeting(&pool, &mid).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, earlier);
        assert_eq!(rows[1].id, later);
    }
}
