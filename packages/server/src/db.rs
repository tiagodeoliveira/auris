//! Postgres persistence layer.
//!
//! Connection string comes from `DATABASE_URL`; the `docker-compose.yml`
//! at the repo root brings up a local Postgres on `5432` matching the
//! default in `.env.example`.
//!
//! `<DATA_DIR>` (env var `MEETING_COMPANION_DATA_DIR`, default `./data`)
//! is still used for blob storage — transcript JSONL, moment screenshots
//! — but no longer hosts the relational store. The two surfaces are
//! independent so the server can scale horizontally with Postgres in
//! front while blob storage moves to S3 (or stays local during dev).
//!
//! All write paths run inside small, focused transactions on the
//! `PgPool`. The pool itself is held by `ServerHandle`; intent
//! handlers reach for it after `apply_intent` returns, keeping the
//! `ServerState` mutex free of any I/O.

use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::postgres::{PgPoolOptions, Postgres};
use sqlx::PgPool;
use tracing::info;

/// Absolute path to the data directory (`<DATA_DIR>` in the docs).
/// Resolves the env var, expands `~`, and creates the directory if
/// it doesn't exist yet. Hosts `blobs/` for transcript JSONL and
/// moment screenshots; the relational store lives in Postgres.
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

/// Open the Postgres pool against `$DATABASE_URL` and run pending
/// migrations. Idempotent on already-migrated databases.
pub async fn open_pool() -> Result<PgPool> {
    let url = std::env::var("DATABASE_URL").context(
        "DATABASE_URL is required (e.g. postgres://meeting_companion:dev@localhost:5432/meeting_companion). \
         Run `docker compose up -d postgres` from the repo root for a local instance."
    )?;
    let pool = open_pool_at(&url).await?;
    info!("postgres ready");
    Ok(pool)
}

/// Test/integration entrypoint: open against an arbitrary URL.
pub async fn open_pool_at(url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(url)
        .await
        .with_context(|| format!("failed to open postgres at {}", redact_url(url)))?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("sqlx migrations failed")?;

    Ok(pool)
}

/// Render a connection URL with the password masked. Used in error
/// messages so a misconfigured `DATABASE_URL` doesn't leak the
/// credential into logs.
///
/// Format expected: `scheme://[user[:password]@]host[:port]/path`.
/// We mask whatever sits between the first `:` after `//` and the
/// first `@`. Falls through unchanged if no `@` is present.
fn redact_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let after_scheme = scheme_end + 3;
    let Some(at_offset) = url[after_scheme..].find('@') else {
        return url.to_string();
    };
    let at_idx = after_scheme + at_offset;
    let Some(colon_offset) = url[after_scheme..at_idx].find(':') else {
        // user only, no password.
        return url.to_string();
    };
    let colon_idx = after_scheme + colon_offset;
    let mut out = String::with_capacity(url.len());
    out.push_str(&url[..colon_idx + 1]);
    out.push_str("***");
    out.push_str(&url[at_idx..]);
    out
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
/// One-shot UPSERT using Postgres' `ON CONFLICT ... DO UPDATE ... RETURNING`,
/// so the row comes back fresh in a single round trip regardless of
/// whether we inserted or updated.
pub async fn upsert_user_by_auth0_sub(
    pool: &PgPool,
    auth0_sub: &str,
    email: Option<&str>,
    name: Option<&str>,
) -> Result<UserRow> {
    let id = uuid::Uuid::new_v4().to_string();
    let row: UserRow = sqlx::query_as(
        r#"
        INSERT INTO users (id, auth0_sub, email, name)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (auth0_sub) DO UPDATE SET
            email = COALESCE(EXCLUDED.email, users.email),
            name = COALESCE(EXCLUDED.name, users.name),
            last_seen_at = NOW()
        RETURNING id, auth0_sub, email, name, created_at, last_seen_at
        "#,
    )
    .bind(&id)
    .bind(auth0_sub)
    .bind(email)
    .bind(name)
    .fetch_one(pool)
    .await
    .with_context(|| format!("upsert_user({auth0_sub})"))?;
    Ok(row)
}

// MARK: - Meetings

/// Insert a meeting row owned by `user_id`. `metadata_json` is the
/// already-serialised JSON object (`HashMap<String, String>` → JSON
/// object string).
pub async fn insert_meeting(
    pool: &PgPool,
    id: &str,
    user_id: &str,
    started_at: DateTime<Utc>,
    description: Option<&str>,
    metadata_json: &str,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO meetings (id, user_id, started_at, description, metadata)
           VALUES ($1, $2, $3, $4, $5)"#,
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

/// Find every unfinished meeting + its owner, one row per. Boot
/// recovery iterates this and re-spawns each user's pipeline.
pub async fn find_active_meetings_per_user(
    pool: &PgPool,
) -> Result<Vec<(String, String, Option<String>, String, DateTime<Utc>)>> {
    // (user_id, meeting_id, description, metadata_json, started_at)
    let rows = sqlx::query_as::<Postgres, (String, String, Option<String>, String, DateTime<Utc>)>(
        r#"SELECT user_id, id, description, metadata, started_at
             FROM meetings
            WHERE ended_at IS NULL
            ORDER BY user_id, started_at DESC"#,
    )
    .fetch_all(pool)
    .await
    .context("find_active_meetings_per_user")?;
    Ok(rows)
}

/// Persist the per-meeting LLM usage rollup + the model that
/// produced it. Called once at meeting stop, after the in-memory
/// `LlmUsageTracker` is drained. Storing `provider` + `model_id`
/// alongside the counts means a future cost-rollup view can apply
/// the right per-token rates against the right model for each
/// meeting — even after rates change or models are deprecated.
pub async fn record_meeting_llm_usage(
    pool: &PgPool,
    meeting_id: &str,
    provider: &str,
    model_id: &str,
    calls: u64,
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
) -> Result<()> {
    sqlx::query(
        r#"UPDATE meetings
              SET llm_calls               = $1,
                  llm_input_tokens        = $2,
                  llm_output_tokens       = $3,
                  llm_cached_input_tokens = $4,
                  llm_provider            = $5,
                  llm_model_id            = $6
            WHERE id = $7"#,
    )
    .bind(calls as i64)
    .bind(input_tokens as i64)
    .bind(output_tokens as i64)
    .bind(cached_input_tokens as i64)
    .bind(provider)
    .bind(model_id)
    .bind(meeting_id)
    .execute(pool)
    .await
    .with_context(|| format!("record_meeting_llm_usage({meeting_id})"))?;
    Ok(())
}

/// Mark a meeting as ended at the given timestamp. No-op (silently)
/// if `meeting_id` doesn't exist or has already been ended.
pub async fn end_meeting(pool: &PgPool, meeting_id: &str, ended_at: DateTime<Utc>) -> Result<()> {
    sqlx::query(
        r#"UPDATE meetings
              SET ended_at = $1
            WHERE id = $2 AND ended_at IS NULL"#,
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
    pool: &PgPool,
    meeting_id: &str,
    user_id: &str,
) -> Result<bool> {
    let res = sqlx::query(r#"DELETE FROM meetings WHERE id = $1 AND user_id = $2"#)
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
    pool: &PgPool,
    id: &str,
    meeting_id: &str,
    kind: &str,
    t: i64,
    note: Option<&str>,
    asset_path: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO moments (id, meeting_id, kind, t, note, asset_path)
           VALUES ($1, $2, $3, $4, $5, $6)"#,
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
    pool: &PgPool,
    moment_id: &str,
    summary: Option<&str>,
    status: &str,
) -> Result<()> {
    sqlx::query(
        r#"UPDATE moments
              SET summary = $1, summary_status = $2
            WHERE id = $3"#,
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
    pool: &PgPool,
    moment_id: &str,
    user_id: &str,
) -> Result<Option<Option<String>>> {
    // Read asset_path + ownership in one shot so we can fail-fast
    // without locking, and so the caller knows what file to remove.
    let asset_path: Option<Option<String>> = sqlx::query_scalar(
        r#"SELECT m.asset_path
             FROM moments m
             JOIN meetings me ON me.id = m.meeting_id
            WHERE m.id = $1 AND me.user_id = $2"#,
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
            WHERE id = $1
              AND meeting_id IN (SELECT id FROM meetings WHERE user_id = $2)"#,
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
    pool: &PgPool,
    moment_id: &str,
    asset_path: &str,
) -> Result<()> {
    sqlx::query(r#"UPDATE moments SET asset_path = $1 WHERE id = $2"#)
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
pub async fn list_moments_for_meeting(pool: &PgPool, meeting_id: &str) -> Result<Vec<MomentRow>> {
    let rows = sqlx::query_as::<_, MomentRow>(
        r#"SELECT id, meeting_id, kind, t, note, asset_path,
                  summary, summary_status, created_at
             FROM moments
            WHERE meeting_id = $1
            ORDER BY t ASC"#,
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("list_moments_for_meeting({meeting_id})"))?;
    Ok(rows)
}

// ─── Artifact subsystem (PLAN.md §3.7) ───────────────────────────────────

/// Row shape for the artifacts library. `asset_path` is relative to
/// `<DATA_DIR>/blobs/`. `short_summary` / `long_summary` populate
/// async after upload — until then `summary_status` is `pending` and
/// the artifact can't be attached to meetings.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ArtifactRow {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub mime_type: String,
    pub asset_path: String,
    pub short_summary: Option<String>,
    pub long_summary: Option<String>,
    pub summary_status: String,
    pub size_bytes: i64,
    pub created_at: DateTime<Utc>,
}

/// Insert a fresh artifact row owned by `user_id`. Summaries land
/// later via `update_artifact_summaries` once the async worker
/// completes; the row starts with `summary_status='pending'`.
pub async fn insert_artifact(
    pool: &PgPool,
    id: &str,
    user_id: &str,
    name: &str,
    mime_type: &str,
    asset_path: &str,
    size_bytes: i64,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO artifacts
               (id, user_id, name, mime_type, asset_path, size_bytes)
           VALUES ($1, $2, $3, $4, $5, $6)"#,
    )
    .bind(id)
    .bind(user_id)
    .bind(name)
    .bind(mime_type)
    .bind(asset_path)
    .bind(size_bytes)
    .execute(pool)
    .await
    .with_context(|| format!("insert_artifact({id})"))?;
    Ok(())
}

/// List one user's artifact library, newest first. Used by the Mac
/// Settings → Artifacts tab and the PWA artifacts modal.
pub async fn list_artifacts_for_user(pool: &PgPool, user_id: &str) -> Result<Vec<ArtifactRow>> {
    let rows = sqlx::query_as::<_, ArtifactRow>(
        r#"SELECT id, user_id, name, mime_type, asset_path,
                  short_summary, long_summary, summary_status,
                  size_bytes, created_at
             FROM artifacts
            WHERE user_id = $1
            ORDER BY created_at DESC"#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("list_artifacts_for_user({user_id})"))?;
    Ok(rows)
}

/// Look up one artifact, ownership-checked against `user_id` so a
/// rogue id from another user can't leak. Returns `None` if the row
/// doesn't exist OR isn't owned by the caller.
pub async fn get_artifact_for_user(
    pool: &PgPool,
    id: &str,
    user_id: &str,
) -> Result<Option<ArtifactRow>> {
    let row = sqlx::query_as::<_, ArtifactRow>(
        r#"SELECT id, user_id, name, mime_type, asset_path,
                  short_summary, long_summary, summary_status,
                  size_bytes, created_at
             FROM artifacts
            WHERE id = $1 AND user_id = $2"#,
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("get_artifact_for_user({id})"))?;
    Ok(row)
}

/// Populate the two summary fields and flip `summary_status`. Called
/// by the async summary worker once both summaries are produced.
/// `status` is one of `'done'` or `'failed'`.
pub async fn update_artifact_summaries(
    pool: &PgPool,
    id: &str,
    short_summary: &str,
    long_summary: &str,
    status: &str,
) -> Result<()> {
    sqlx::query(
        r#"UPDATE artifacts
              SET short_summary = $1,
                  long_summary  = $2,
                  summary_status = $3
            WHERE id = $4"#,
    )
    .bind(short_summary)
    .bind(long_summary)
    .bind(status)
    .bind(id)
    .execute(pool)
    .await
    .with_context(|| format!("update_artifact_summaries({id})"))?;
    Ok(())
}

/// Delete one artifact; ownership-checked. Cascade drops any
/// `meeting_artifacts` rows for it. Caller is responsible for the
/// blob on disk — the DB doesn't know the `<DATA_DIR>` root.
pub async fn delete_artifact_for_user(pool: &PgPool, id: &str, user_id: &str) -> Result<()> {
    sqlx::query(r#"DELETE FROM artifacts WHERE id = $1 AND user_id = $2"#)
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await
        .with_context(|| format!("delete_artifact_for_user({id})"))?;
    Ok(())
}

/// Attach an artifact to a meeting. FK-checked: both rows must
/// exist. Idempotent — re-attaching the same artifact silently
/// no-ops via `ON CONFLICT DO NOTHING`. Mid-meeting picker UX
/// relies on this so the user doesn't have to track what's
/// already attached.
pub async fn attach_artifact_to_meeting(
    pool: &PgPool,
    meeting_id: &str,
    artifact_id: &str,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO meeting_artifacts (meeting_id, artifact_id)
           VALUES ($1, $2)
           ON CONFLICT (meeting_id, artifact_id) DO NOTHING"#,
    )
    .bind(meeting_id)
    .bind(artifact_id)
    .execute(pool)
    .await
    .with_context(|| format!("attach_artifact_to_meeting({meeting_id}, {artifact_id})"))?;
    Ok(())
}

/// Detach one artifact from one meeting. No-op if the join row
/// already isn't there.
pub async fn detach_artifact_from_meeting(
    pool: &PgPool,
    meeting_id: &str,
    artifact_id: &str,
) -> Result<()> {
    sqlx::query(
        r#"DELETE FROM meeting_artifacts
            WHERE meeting_id = $1 AND artifact_id = $2"#,
    )
    .bind(meeting_id)
    .bind(artifact_id)
    .execute(pool)
    .await
    .with_context(|| format!("detach_artifact_from_meeting({meeting_id}, {artifact_id})"))?;
    Ok(())
}

/// List artifacts attached to one meeting, attach order. Joined
/// against `artifacts` so the caller gets the full row in one trip.
pub async fn list_artifacts_for_meeting(
    pool: &PgPool,
    meeting_id: &str,
) -> Result<Vec<ArtifactRow>> {
    let rows = sqlx::query_as::<_, ArtifactRow>(
        r#"SELECT a.id, a.user_id, a.name, a.mime_type, a.asset_path,
                  a.short_summary, a.long_summary, a.summary_status,
                  a.size_bytes, a.created_at
             FROM artifacts a
             JOIN meeting_artifacts ma ON ma.artifact_id = a.id
            WHERE ma.meeting_id = $1
            ORDER BY ma.attached_at ASC"#,
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("list_artifacts_for_meeting({meeting_id})"))?;
    Ok(rows)
}

// ─── Items (per-meeting persisted modes) ────────────────────────────

/// Append one item to its meeting's persisted history. Used by the
/// items-persistence task on every Append-strategy `ItemsUpdate`
/// broadcast (actions / open_questions). `ON CONFLICT DO NOTHING` so
/// the writer is idempotent — a transient retry that re-broadcasts
/// the same id is a no-op.
pub async fn insert_item_row(
    pool: &PgPool,
    meeting_id: &str,
    mode: &str,
    item: &crate::contract::Item,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO items (id, meeting_id, mode, text, detail, t_ms, meta)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           ON CONFLICT (id) DO NOTHING"#,
    )
    .bind(&item.id)
    .bind(meeting_id)
    .bind(mode)
    .bind(&item.text)
    .bind(&item.detail)
    .bind(item.t as i64)
    .bind(item.meta.as_ref())
    .execute(pool)
    .await
    .with_context(|| format!("insert_item_row({})", item.id))?;
    Ok(())
}

/// Replace-strategy persistence: drop everything for `(meeting_id,
/// mode)` and insert the new set in a single transaction so a crash
/// can never leave a torn snapshot. Used for highlights / summary /
/// chat — modes whose live state is "the current full list, no
/// history."
pub async fn replace_items_for_meeting_mode(
    pool: &PgPool,
    meeting_id: &str,
    mode: &str,
    items: &[crate::contract::Item],
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(r#"DELETE FROM items WHERE meeting_id = $1 AND mode = $2"#)
        .bind(meeting_id)
        .bind(mode)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("replace_items: delete {meeting_id}/{mode}"))?;
    for item in items {
        sqlx::query(
            r#"INSERT INTO items (id, meeting_id, mode, text, detail, t_ms, meta)
               VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
        )
        .bind(&item.id)
        .bind(meeting_id)
        .bind(mode)
        .bind(&item.text)
        .bind(&item.detail)
        .bind(item.t as i64)
        .bind(item.meta.as_ref())
        .execute(&mut *tx)
        .await
        .with_context(|| format!("replace_items: insert {}", item.id))?;
    }
    tx.commit().await?;
    Ok(())
}

/// Read every persisted item for a meeting, grouped by mode and
/// ordered by `created_at` within each group. Powers the meeting-
/// detail view's per-mode tabs. Excludes transcript-mode items —
/// those live in the JSONL blob and aren't written to this table
/// at all.
pub async fn list_items_for_meeting_grouped(
    pool: &PgPool,
    meeting_id: &str,
) -> Result<std::collections::HashMap<String, Vec<crate::contract::Item>>> {
    let rows: Vec<(
        String,
        String,
        String,
        Option<String>,
        i64,
        Option<serde_json::Value>,
    )> = sqlx::query_as(
        r#"SELECT id, mode, text, detail, t_ms, meta
                 FROM items
                WHERE meeting_id = $1
             ORDER BY mode, created_at"#,
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("list_items_for_meeting_grouped({meeting_id})"))?;
    let mut grouped: std::collections::HashMap<String, Vec<crate::contract::Item>> =
        std::collections::HashMap::new();
    for (id, mode, text, detail, t_ms, meta) in rows {
        grouped
            .entry(mode)
            .or_default()
            .push(crate::contract::Item {
                id,
                text,
                detail,
                t: t_ms.max(0) as u64,
                meta,
            });
    }
    Ok(grouped)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Most DB tests need an owner user to satisfy the meetings FK.
    /// Each invocation gets a fresh `auth0_sub` so concurrent tests
    /// inside one DB don't clash on the unique index.
    async fn test_user(pool: &PgPool) -> String {
        let sub = format!("test|{}", uuid::Uuid::new_v4());
        upsert_user_by_auth0_sub(pool, &sub, None, None)
            .await
            .unwrap()
            .id
    }

    #[sqlx::test]
    async fn insert_then_end_meeting_round_trips(pool: PgPool) {
        let uid = test_user(&pool).await;
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        insert_meeting(&pool, &id, &uid, now, Some("daily standup"), "{}")
            .await
            .unwrap();
        end_meeting(&pool, &id, now).await.unwrap();

        let row: (String, Option<DateTime<Utc>>) =
            sqlx::query_as("SELECT id, ended_at FROM meetings WHERE id = $1")
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

    #[sqlx::test]
    async fn insert_moment_links_to_meeting(pool: PgPool) {
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
                 FROM moments WHERE id = $1"#,
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

    #[sqlx::test]
    async fn moment_fk_blocks_orphan_inserts(pool: PgPool) {
        let id = uuid::Uuid::new_v4().to_string();
        let res = insert_moment(&pool, &id, "no-such-meeting", "manual", 0, None, None).await;
        assert!(res.is_err(), "expected FK violation on orphan moment");
    }

    // ─── Artifact subsystem (PLAN.md §3.7) ───────────────────────────────

    #[sqlx::test]
    async fn insert_artifact_round_trips(pool: PgPool) {
        let uid = test_user(&pool).await;
        let aid = uuid::Uuid::new_v4().to_string();
        insert_artifact(
            &pool,
            &aid,
            &uid,
            "agenda.md",
            "text/markdown",
            "artifacts/u/x.md",
            1234,
        )
        .await
        .unwrap();
        let row = get_artifact_for_user(&pool, &aid, &uid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.id, aid);
        assert_eq!(row.name, "agenda.md");
        assert_eq!(row.mime_type, "text/markdown");
        assert_eq!(row.asset_path, "artifacts/u/x.md");
        assert_eq!(row.size_bytes, 1234);
        assert_eq!(row.summary_status, "pending");
        assert!(row.short_summary.is_none());
        assert!(row.long_summary.is_none());
    }

    #[sqlx::test]
    async fn list_artifacts_for_user_orders_newest_first(pool: PgPool) {
        let uid = test_user(&pool).await;
        let a1 = uuid::Uuid::new_v4().to_string();
        insert_artifact(&pool, &a1, &uid, "first.md", "text/markdown", "p1", 10)
            .await
            .unwrap();
        // Force a later created_at.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let a2 = uuid::Uuid::new_v4().to_string();
        insert_artifact(&pool, &a2, &uid, "second.md", "text/markdown", "p2", 20)
            .await
            .unwrap();
        let rows = list_artifacts_for_user(&pool, &uid).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, a2, "newest first");
        assert_eq!(rows[1].id, a1);
    }

    #[sqlx::test]
    async fn update_artifact_summaries_transitions_status(pool: PgPool) {
        let uid = test_user(&pool).await;
        let aid = uuid::Uuid::new_v4().to_string();
        insert_artifact(&pool, &aid, &uid, "x.md", "text/markdown", "p", 1)
            .await
            .unwrap();
        update_artifact_summaries(&pool, &aid, "short text", "long text", "done")
            .await
            .unwrap();
        let row = get_artifact_for_user(&pool, &aid, &uid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.short_summary.as_deref(), Some("short text"));
        assert_eq!(row.long_summary.as_deref(), Some("long text"));
        assert_eq!(row.summary_status, "done");
    }

    #[sqlx::test]
    async fn delete_artifact_removes_row_and_join(pool: PgPool) {
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, Utc::now(), None, "{}")
            .await
            .unwrap();
        let aid = uuid::Uuid::new_v4().to_string();
        insert_artifact(&pool, &aid, &uid, "x.md", "text/markdown", "p", 1)
            .await
            .unwrap();
        attach_artifact_to_meeting(&pool, &mid, &aid).await.unwrap();
        delete_artifact_for_user(&pool, &aid, &uid).await.unwrap();
        // Artifact row gone.
        assert!(get_artifact_for_user(&pool, &aid, &uid)
            .await
            .unwrap()
            .is_none());
        // Cascade dropped the join row too.
        let attached = list_artifacts_for_meeting(&pool, &mid).await.unwrap();
        assert!(attached.is_empty());
    }

    #[sqlx::test]
    async fn attach_artifact_round_trips_through_join(pool: PgPool) {
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, Utc::now(), None, "{}")
            .await
            .unwrap();
        let a1 = uuid::Uuid::new_v4().to_string();
        let a2 = uuid::Uuid::new_v4().to_string();
        insert_artifact(&pool, &a1, &uid, "a1.md", "text/markdown", "p1", 1)
            .await
            .unwrap();
        insert_artifact(&pool, &a2, &uid, "a2.md", "text/markdown", "p2", 1)
            .await
            .unwrap();
        attach_artifact_to_meeting(&pool, &mid, &a1).await.unwrap();
        attach_artifact_to_meeting(&pool, &mid, &a2).await.unwrap();
        let attached = list_artifacts_for_meeting(&pool, &mid).await.unwrap();
        assert_eq!(attached.len(), 2);
    }

    #[sqlx::test]
    async fn detach_artifact_removes_join_row(pool: PgPool) {
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, Utc::now(), None, "{}")
            .await
            .unwrap();
        let aid = uuid::Uuid::new_v4().to_string();
        insert_artifact(&pool, &aid, &uid, "x.md", "text/markdown", "p", 1)
            .await
            .unwrap();
        attach_artifact_to_meeting(&pool, &mid, &aid).await.unwrap();
        detach_artifact_from_meeting(&pool, &mid, &aid)
            .await
            .unwrap();
        let attached = list_artifacts_for_meeting(&pool, &mid).await.unwrap();
        assert!(attached.is_empty());
        // Artifact row itself stays.
        assert!(get_artifact_for_user(&pool, &aid, &uid)
            .await
            .unwrap()
            .is_some());
    }

    #[sqlx::test]
    async fn attach_to_nonexistent_meeting_fails_fk(pool: PgPool) {
        let uid = test_user(&pool).await;
        let aid = uuid::Uuid::new_v4().to_string();
        insert_artifact(&pool, &aid, &uid, "x.md", "text/markdown", "p", 1)
            .await
            .unwrap();
        let res = attach_artifact_to_meeting(&pool, "no-such-meeting", &aid).await;
        assert!(res.is_err(), "expected FK violation");
    }

    #[sqlx::test]
    async fn update_moment_summary_round_trips(pool: PgPool) {
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
            sqlx::query_as("SELECT summary, summary_status FROM moments WHERE id = $1")
                .bind(&moment_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0.as_deref(), Some("Summary text"));
        assert_eq!(row.1, "done");
    }

    #[sqlx::test]
    async fn list_moments_returns_oldest_first(pool: PgPool) {
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

    #[test]
    fn redact_url_masks_password() {
        assert_eq!(
            redact_url("postgres://user:secret@host:5432/db"),
            "postgres://user:***@host:5432/db"
        );
        // No password — pass through untouched.
        assert_eq!(
            redact_url("postgres://user@host/db"),
            "postgres://user@host/db"
        );
    }
}
