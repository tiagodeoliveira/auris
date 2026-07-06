//! Meeting persistence: insert, end, and query meetings, plus
//! meeting-to-meeting attachment management.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::postgres::Postgres;
use sqlx::PgPool;
use std::collections::HashMap;

/// Decode the `meetings.metadata` JSON-as-TEXT column (see
/// 0001_initial_schema.sql — deliberately TEXT, not JSONB) into a
/// flat `<String, String>` map. Malformed rows (schema drift, manual
/// edits, partial writes) fall back to an empty map and log at
/// warn so corruption is visible without breaking the read path.
fn parse_metadata_map(meeting_id: &str, raw: serde_json::Value) -> HashMap<String, String> {
    match serde_json::from_value(raw) {
        Ok(map) => map,
        Err(err) => {
            tracing::warn!(meeting_id, error = %err, "meeting metadata not a string map; falling back to empty");
            HashMap::new()
        }
    }
}

/// Parse a raw `meetings.metadata` TEXT blob into a flat
/// `<String, String>` map. The column is JSON-as-TEXT, so every
/// reader must decode it as `String` and parse here — decoding it
/// as `sqlx::types::Json<_>` fails ColumnDecode on every row.
fn parse_metadata_json(meeting_id: &str, raw: &str) -> HashMap<String, String> {
    let parsed = serde_json::from_str(raw).unwrap_or(serde_json::Value::Null);
    parse_metadata_map(meeting_id, parsed)
}

/// Insert a meeting row owned by `user_id`. `metadata_json` is the
/// already-serialised JSON object (`HashMap<String, String>` → JSON
/// object string). `assist_sensitivity` is the canonical wire string
/// ("aggressive" / "moderate" / "minimal") or `None` (= NULL column
/// = treated as Moderate by the load path).
pub async fn insert_meeting(
    pool: &PgPool,
    id: &str,
    user_id: &str,
    started_at: DateTime<Utc>,
    description: Option<&str>,
    metadata_json: &str,
    assist_sensitivity: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO meetings (id, user_id, started_at, description, metadata, assist_sensitivity)
           VALUES ($1, $2, $3, $4, $5, $6)"#,
    )
    .bind(id)
    .bind(user_id)
    .bind(started_at)
    .bind(description)
    .bind(metadata_json)
    .bind(assist_sensitivity)
    .execute(pool)
    .await
    .with_context(|| format!("insert_meeting({id})"))?;
    Ok(())
}

/// Insert a fully-formed, already-ended meeting row recovered from a
/// JSONL transcript by the `recover-meeting` binary. Unlike
/// `insert_meeting`, both `started_at` and `ended_at` are known up
/// front, and the insert is idempotent via `ON CONFLICT (id) DO
/// NOTHING` so recovery re-runs (`--force`) land cleanly.
///
/// `user_id` MUST be the internal `users.id` UUID — the FK on
/// `meetings.user_id` rejects raw Auth0 subs. Callers resolve the sub
/// first (see `users::upsert_user_by_auth0_sub`).
pub async fn insert_recovered_meeting(
    pool: &PgPool,
    id: &str,
    user_id: &str,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    description: Option<&str>,
    metadata_json: &str,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO meetings (id, user_id, started_at, ended_at, description, metadata)
           VALUES ($1, $2, $3, $4, $5, $6)
           ON CONFLICT (id) DO NOTHING"#,
    )
    .bind(id)
    .bind(user_id)
    .bind(started_at)
    .bind(ended_at)
    .bind(description)
    .bind(metadata_json)
    .execute(pool)
    .await
    .with_context(|| format!("insert_recovered_meeting({id})"))?;
    Ok(())
}

/// Update an active meeting's `assist_sensitivity` column. Called
/// when `SetAssistSensitivity` lands mid-meeting. Stored as the
/// canonical wire string so a future reconnect / page-reload sees
/// the same value the user picked. Idempotent at the SQL level —
/// re-setting to the same value is a no-op on the column.
pub async fn set_assist_sensitivity(pool: &PgPool, meeting_id: &str, value: &str) -> Result<()> {
    sqlx::query(
        r#"UPDATE meetings
              SET assist_sensitivity = $1
            WHERE id = $2"#,
    )
    .bind(value)
    .bind(meeting_id)
    .execute(pool)
    .await
    .with_context(|| format!("set_assist_sensitivity({meeting_id}, {value})"))?;
    Ok(())
}

/// Find every unfinished meeting + its owner, one row per. Boot
/// recovery iterates this and re-spawns each user's pipeline.
/// Returns `(user_id, meeting_id, description, metadata_json,
/// started_at, assist_sensitivity)`. `assist_sensitivity` is the
/// raw column value; callers should parse via `AssistSensitivity::
/// from_str` and fall back to the default for NULL / unknown.
pub async fn find_active_meetings_per_user(
    pool: &PgPool,
) -> Result<
    Vec<(
        String,
        String,
        Option<String>,
        String,
        DateTime<Utc>,
        Option<String>,
    )>,
> {
    let rows = sqlx::query_as::<
        Postgres,
        (
            String,
            String,
            Option<String>,
            String,
            DateTime<Utc>,
            Option<String>,
        ),
    >(
        r#"SELECT user_id, id, description, metadata, started_at, assist_sensitivity
             FROM meetings
            WHERE ended_at IS NULL
            ORDER BY user_id, started_at DESC"#,
    )
    .fetch_all(pool)
    .await
    .context("find_active_meetings_per_user")?;
    Ok(rows)
}

/// Insert one (meeting_id, pool) row in `meeting_llm_usage`.
/// Called once per pool at meeting stop, after each pool's
/// `LlmUsageTracker` is drained. Storing provider + model alongside
/// the counts so a future cost-rollup view can apply the right
/// per-token rates against the right model — even after rates or
/// model availability change.
#[allow(clippy::too_many_arguments)]
pub async fn insert_meeting_llm_usage(
    pool_db: &PgPool,
    meeting_id: &str,
    pool: &str,
    provider: &str,
    model_id: &str,
    calls: u64,
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO meeting_llm_usage
               (meeting_id, pool, provider, model_id,
                calls, input_tokens, output_tokens, cached_input_tokens)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
    )
    .bind(meeting_id)
    .bind(pool)
    .bind(provider)
    .bind(model_id)
    .bind(calls as i64)
    .bind(input_tokens as i64)
    .bind(output_tokens as i64)
    .bind(cached_input_tokens as i64)
    .execute(pool_db)
    .await
    .with_context(|| format!("insert_meeting_llm_usage({meeting_id}, {pool})"))?;
    Ok(())
}

/// One per-pool usage row for a meeting, read back from
/// `meeting_llm_usage` (migration 0011). Field order matches the
/// SELECT in `list_meeting_llm_usage` 1:1.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MeetingLlmUsageRow {
    pub pool: String,
    pub provider: String,
    pub model_id: String,
    pub calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_input_tokens: i64,
}

/// All per-pool usage rows for one meeting, ordered by pool name so
/// the serialized JSON is deterministic ("background" then "chat").
/// Empty vec for meetings finalized before migration 0011 (their
/// usage lives in the legacy `meetings.llm_*` columns) and for
/// meetings whose finalize path recorded nothing.
pub async fn list_meeting_llm_usage(
    pool_db: &PgPool,
    meeting_id: &str,
) -> Result<Vec<MeetingLlmUsageRow>> {
    let rows: Vec<MeetingLlmUsageRow> = sqlx::query_as(
        r#"SELECT pool, provider, model_id,
                  calls, input_tokens, output_tokens, cached_input_tokens
             FROM meeting_llm_usage
            WHERE meeting_id = $1
            ORDER BY pool ASC"#,
    )
    .bind(meeting_id)
    .fetch_all(pool_db)
    .await
    .with_context(|| format!("list_meeting_llm_usage({meeting_id})"))?;
    Ok(rows)
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

/// Update the post-meeting wrap-up extractor's status column on a
/// meeting. Valid values are `'running'`, `'success'`, `'failed'`.
/// Called from `summarizer::wrap_up::extract` at task start (running)
/// and on completion (success or failed). Idempotent — re-running
/// with the same status is fine. No-op if `meeting_id` doesn't exist.
pub async fn set_wrap_up_status(pool: &PgPool, meeting_id: &str, status: &str) -> Result<()> {
    sqlx::query(
        r#"UPDATE meetings
              SET wrap_up_status = $1
            WHERE id = $2"#,
    )
    .bind(status)
    .bind(meeting_id)
    .execute(pool)
    .await
    .with_context(|| format!("set_wrap_up_status({meeting_id}, {status})"))?;
    Ok(())
}

/// Boot-time sweep: a meeting that ended but still reads
/// `wrap_up_status='running'` was orphaned by a restart mid-finalize —
/// finalize and the wrap-up retry are detached tasks and die with the
/// process, and nothing else ever flips the status (the retry endpoint
/// rejects 'running' rows with 400 `already_running`). Flip those rows
/// to 'failed' so the clients' existing failed banner + retry paths
/// apply. Rows with `ended_at IS NULL` are deliberately spared — they
/// belong to boot recovery (`recover_active_meetings`), whose
/// re-spawned pipeline runs a fresh finalize that overwrites the
/// status itself. Returns the number of rows swept.
pub async fn fail_orphaned_wrap_ups(pool: &PgPool) -> Result<u64> {
    let res = sqlx::query(
        r#"UPDATE meetings
              SET wrap_up_status = 'failed'
            WHERE wrap_up_status = 'running' AND ended_at IS NOT NULL"#,
    )
    .execute(pool)
    .await
    .context("fail_orphaned_wrap_ups")?;
    Ok(res.rows_affected())
}

/// Find meetings whose post-stop finalize was interrupted by a process
/// death: `ended_at` is set but `wrap_up_status` is still stuck at
/// `'running'`. Finalize marks `running` up front and the extractor owns
/// the terminal `success`/`failed` write — so if the process dies in
/// between (deploy seconds after StopMeeting, OOM, panic) the row stays
/// `running` forever, and `POST /meetings/:id/retry-wrap-up` rejects it
/// with `already_running`. At boot no extractor can actually be in
/// flight in a fresh process, so `running` here unambiguously means
/// "killed mid-run". Returns `(user_id, meeting_id)` pairs, oldest
/// first, for the boot re-kick (`workers::wrap_up::rekick_interrupted`).
pub async fn find_interrupted_wrap_ups(pool: &PgPool) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query_as::<Postgres, (String, String)>(
        r#"SELECT user_id, id
             FROM meetings
            WHERE ended_at IS NOT NULL
              AND wrap_up_status = 'running'
            ORDER BY started_at"#,
    )
    .fetch_all(pool)
    .await
    .context("find_interrupted_wrap_ups")?;
    Ok(rows)
}

/// Read a meeting's current `description` column and parsed `metadata`
/// map — the inputs the finalize-time backfill worker needs to decide
/// which fields are missing. `None` if the meeting id is unknown.
pub async fn load_meta_for_backfill(
    pool: &PgPool,
    meeting_id: &str,
) -> Result<Option<(Option<String>, HashMap<String, String>)>> {
    let row: Option<(Option<String>, String)> =
        sqlx::query_as(r#"SELECT description, metadata FROM meetings WHERE id = $1"#)
            .bind(meeting_id)
            .fetch_optional(pool)
            .await
            .with_context(|| format!("load_meta_for_backfill({meeting_id})"))?;
    Ok(row.map(|(description, metadata_json)| {
        (description, parse_metadata_json(meeting_id, &metadata_json))
    }))
}

/// Overwrite a meeting's `description` column. Used by the finalize-time
/// backfill worker to fill a description generated from the transcript
/// when the meeting started without one. No-op if `meeting_id` is unknown.
pub async fn set_meeting_description(
    pool: &PgPool,
    meeting_id: &str,
    description: &str,
) -> Result<()> {
    sqlx::query(r#"UPDATE meetings SET description = $1 WHERE id = $2"#)
        .bind(description)
        .bind(meeting_id)
        .execute(pool)
        .await
        .with_context(|| format!("set_meeting_description({meeting_id})"))?;
    Ok(())
}

/// Set the `title` tag inside a meeting's `metadata` JSON blob,
/// preserving any other tags (read-modify-write). Used by the
/// finalize-time backfill worker. No-op if `meeting_id` is unknown.
pub async fn set_meeting_title(pool: &PgPool, meeting_id: &str, title: &str) -> Result<()> {
    let row: Option<(String,)> = sqlx::query_as(r#"SELECT metadata FROM meetings WHERE id = $1"#)
        .bind(meeting_id)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("set_meeting_title read({meeting_id})"))?;
    let Some((metadata_json,)) = row else {
        return Ok(()); // unknown meeting — silent no-op, like end_meeting
    };
    // Read-modify-write: preserve any other tags (e.g. project).
    let mut obj: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&metadata_json).unwrap_or_default();
    obj.insert(
        "title".to_string(),
        serde_json::Value::String(title.to_string()),
    );
    let updated = serde_json::Value::Object(obj).to_string();
    sqlx::query(r#"UPDATE meetings SET metadata = $1 WHERE id = $2"#)
        .bind(&updated)
        .bind(meeting_id)
        .execute(pool)
        .await
        .with_context(|| format!("set_meeting_title write({meeting_id})"))?;
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

/// Slim meeting metadata used by the agent's `[attached meetings]`
/// bootstrap block. Title is computed server-side via the same
/// fallback chain the clients apply (`pickMeetingTitle`).
#[derive(Debug, Clone)]
pub struct AttachedMeetingMeta {
    pub id: String,
    pub title: String,
    pub ended_at: Option<DateTime<Utc>>,
}

/// Single-meeting variant of `list_attached_meetings_for_agent` —
/// used by the kick handler for `MeetingAttached` to format the
/// [event] block when one meeting gets attached mid-fire.
pub async fn get_meeting_summary_for_user(
    pool: &PgPool,
    meeting_id: &str,
    user_id: &str,
) -> Result<Option<AttachedMeetingMeta>> {
    // sqlx tuple maps 1:1 to the SELECT below; a named alias would
    // hide the column order from the query site. `metadata` is a
    // JSON-as-TEXT column — decode as String, never Json<_>.
    #[allow(clippy::type_complexity)]
    let row: Option<(String, Option<String>, String, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"SELECT id, description, metadata, ended_at
                 FROM meetings WHERE id = $1 AND user_id = $2"#,
    )
    .bind(meeting_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("get_meeting_summary_for_user({meeting_id})"))?;
    Ok(row.map(|(id, description, metadata_json, ended_at)| {
        let metadata_obj = parse_metadata_json(&id, &metadata_json);
        let title = pick_meeting_title(description.as_deref(), &metadata_obj);
        AttachedMeetingMeta {
            id,
            title,
            ended_at,
        }
    }))
}

/// Load slim metadata for each attached meeting. Returns rows in
/// attach order (matching `list_attached_meeting_ids`) joined to
/// the meetings table so the agent context can include human titles
/// without a second roundtrip per id.
pub async fn list_attached_meetings_for_agent(
    pool: &PgPool,
    parent_meeting_id: &str,
    user_id: &str,
) -> Result<Vec<AttachedMeetingMeta>> {
    // Belt-and-suspenders ownership join: only return attached
    // meetings the user actually owns. A malicious attach attempt
    // would have failed at REST, but the join keeps invariants
    // local to the read path too.
    // `m.metadata` is a JSON-as-TEXT column — decode as String,
    // never Json<_> (Json<_> rejects TEXT and fails every row).
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, Option<String>, String, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"SELECT m.id, m.description, m.metadata, m.ended_at
                 FROM meeting_attachments ma
                 JOIN meetings m ON m.id = ma.attached_meeting_id
                WHERE ma.parent_meeting_id = $1 AND m.user_id = $2
                ORDER BY ma.attached_at ASC"#,
    )
    .bind(parent_meeting_id)
    .bind(user_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("list_attached_meetings_for_agent({parent_meeting_id})"))?;
    Ok(rows
        .into_iter()
        .map(|(id, description, metadata_json, ended_at)| {
            let metadata_obj = parse_metadata_json(&id, &metadata_json);
            let title = pick_meeting_title(description.as_deref(), &metadata_obj);
            AttachedMeetingMeta {
                id,
                title,
                ended_at,
            }
        })
        .collect())
}

/// Server-side mirror of the clients' `pickMeetingTitle` fallback
/// chain: metadata.title → first non-empty description line clipped
/// to 80 chars → "Untitled meeting".
fn pick_meeting_title(
    description: Option<&str>,
    metadata: &std::collections::HashMap<String, String>,
) -> String {
    if let Some(title) = metadata
        .get("title")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return title.to_string();
    }
    if let Some(desc) = description {
        if let Some(first_line) = desc.lines().find(|l| !l.trim().is_empty()) {
            let trimmed = first_line.trim();
            if trimmed.len() <= 80 {
                return trimmed.to_string();
            }
            return format!("{}…", &trimmed[..79]);
        }
    }
    "Untitled meeting".to_string()
}

/// Attach a past meeting to a parent meeting so the agent's
/// `recall_meeting` tool can recall against
/// it. Idempotent (re-attach is a no-op). The DB schema's CHECK
/// constraint rejects self-attaches; callers should validate
/// `parent != attached` higher up for a nicer error message.
pub async fn attach_meeting_to_meeting(
    pool: &PgPool,
    parent_meeting_id: &str,
    attached_meeting_id: &str,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO meeting_attachments (parent_meeting_id, attached_meeting_id)
           VALUES ($1, $2)
           ON CONFLICT (parent_meeting_id, attached_meeting_id) DO NOTHING"#,
    )
    .bind(parent_meeting_id)
    .bind(attached_meeting_id)
    .execute(pool)
    .await
    .with_context(|| {
        format!("attach_meeting_to_meeting({parent_meeting_id}, {attached_meeting_id})")
    })?;
    Ok(())
}

/// Detach one attached meeting from a parent. No-op if not attached.
pub async fn detach_meeting_from_meeting(
    pool: &PgPool,
    parent_meeting_id: &str,
    attached_meeting_id: &str,
) -> Result<()> {
    sqlx::query(
        r#"DELETE FROM meeting_attachments
            WHERE parent_meeting_id = $1 AND attached_meeting_id = $2"#,
    )
    .bind(parent_meeting_id)
    .bind(attached_meeting_id)
    .execute(pool)
    .await
    .with_context(|| {
        format!("detach_meeting_from_meeting({parent_meeting_id}, {attached_meeting_id})")
    })?;
    Ok(())
}

/// Attached past-meeting ids for one parent, attach order. Returns
/// just the ids — the snapshot and `AttachedMeetingsChanged` wire
/// shape carries strings only (clients hydrate to full meetings via
/// the existing `GET /meetings/:id` if they need title / timing).
pub async fn list_attached_meeting_ids(
    pool: &PgPool,
    parent_meeting_id: &str,
) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"SELECT attached_meeting_id
             FROM meeting_attachments
            WHERE parent_meeting_id = $1
            ORDER BY attached_at ASC"#,
    )
    .bind(parent_meeting_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("list_attached_meeting_ids({parent_meeting_id})"))?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::users::upsert_user_by_auth0_sub;

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
        let now = chrono::Utc::now();
        insert_meeting(&pool, &id, &uid, now, Some("daily standup"), "{}", None)
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
    async fn find_interrupted_wrap_ups_returns_only_ended_running(pool: PgPool) {
        let uid = test_user(&pool).await;
        let now = chrono::Utc::now();

        // Interrupted finalize: ended, status stuck at 'running'.
        // This is the state a process death mid-finalize leaves behind
        // (finalize marks 'running' up front; extract owns the terminal
        // write and never got to it). MUST be returned.
        let interrupted = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &interrupted, &uid, now, None, "{}", None)
            .await
            .unwrap();
        end_meeting(&pool, &interrupted, now).await.unwrap();
        set_wrap_up_status(&pool, &interrupted, "running")
            .await
            .unwrap();

        // Completed wrap-up: ended + 'success'. Excluded.
        let succeeded = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &succeeded, &uid, now, None, "{}", None)
            .await
            .unwrap();
        end_meeting(&pool, &succeeded, now).await.unwrap();
        set_wrap_up_status(&pool, &succeeded, "success")
            .await
            .unwrap();

        // Still-active meeting (ended_at IS NULL): belongs to live-meeting
        // boot recovery (`recover_active_meetings`), NOT the wrap-up
        // re-kick. Excluded even with a 'running' status.
        let live = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &live, &uid, now, None, "{}", None)
            .await
            .unwrap();
        set_wrap_up_status(&pool, &live, "running").await.unwrap();

        // Legacy meeting: ended, wrap_up_status NULL (predates the
        // extractor). Excluded — nothing was interrupted.
        let legacy = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &legacy, &uid, now, None, "{}", None)
            .await
            .unwrap();
        end_meeting(&pool, &legacy, now).await.unwrap();

        let rows = find_interrupted_wrap_ups(&pool).await.unwrap();
        assert_eq!(
            rows,
            vec![(uid.clone(), interrupted.clone())],
            "only the ended+running meeting is an interrupted wrap-up"
        );
    }

    #[sqlx::test]
    async fn set_meeting_description_overwrites_the_column(pool: PgPool) {
        let uid = test_user(&pool).await;
        let id = uuid::Uuid::new_v4().to_string();
        // Started with no description (the backfill trigger case).
        insert_meeting(&pool, &id, &uid, chrono::Utc::now(), None, "{}", None)
            .await
            .unwrap();

        set_meeting_description(&pool, &id, "A generated one-liner.")
            .await
            .unwrap();

        let row: (Option<String>,) =
            sqlx::query_as("SELECT description FROM meetings WHERE id = $1")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0.as_deref(), Some("A generated one-liner."));
    }

    #[sqlx::test]
    async fn set_meeting_title_preserves_other_metadata_tags(pool: PgPool) {
        let uid = test_user(&pool).await;
        let id = uuid::Uuid::new_v4().to_string();
        insert_meeting(
            &pool,
            &id,
            &uid,
            chrono::Utc::now(),
            None,
            r#"{"project":"Phoenix"}"#,
            None,
        )
        .await
        .unwrap();

        set_meeting_title(&pool, &id, "Sprint planning")
            .await
            .unwrap();

        // `metadata` is a TEXT column holding JSON; read as String then parse.
        let row: (String,) = sqlx::query_as("SELECT metadata FROM meetings WHERE id = $1")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let map = parse_metadata_map(&id, serde_json::from_str(&row.0).unwrap());
        assert_eq!(
            map.get("title").map(String::as_str),
            Some("Sprint planning")
        );
        assert_eq!(
            map.get("project").map(String::as_str),
            Some("Phoenix"),
            "existing tags must survive the title write"
        );
    }

    #[sqlx::test]
    async fn get_meeting_summary_for_user_picks_title_from_metadata(pool: PgPool) {
        let uid = test_user(&pool).await;
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        insert_meeting(
            &pool,
            &id,
            &uid,
            now,
            Some("quarterly business review with Acme"),
            r#"{"title":"Q1 review"}"#,
            None,
        )
        .await
        .unwrap();
        end_meeting(&pool, &id, now).await.unwrap();

        // Reproduces the prod bug: `metadata` is a TEXT column, so a
        // `sqlx::types::Json<_>` tuple element fails ColumnDecode on
        // every row that exists. This must decode cleanly.
        let meta = get_meeting_summary_for_user(&pool, &id, &uid)
            .await
            .expect("decode must succeed against the TEXT metadata column")
            .expect("row exists and is owned by uid");
        assert_eq!(meta.id, id);
        assert_eq!(
            meta.title, "Q1 review",
            "metadata.title must win over description"
        );
        assert!(meta.ended_at.is_some(), "ended_at column must round-trip");
    }

    #[sqlx::test]
    async fn get_meeting_summary_for_user_returns_none_for_other_users_meeting(pool: PgPool) {
        let owner = test_user(&pool).await;
        let other = test_user(&pool).await;
        let id = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &id, &owner, chrono::Utc::now(), None, "{}", None)
            .await
            .unwrap();

        // Contract pin (passes even pre-fix, because zero-row results
        // skip the decode path): the ownership guard must hide other
        // users' meetings as Ok(None), not an error.
        let meta = get_meeting_summary_for_user(&pool, &id, &other)
            .await
            .unwrap();
        assert!(
            meta.is_none(),
            "ownership guard must hide other users' meetings"
        );
    }

    #[sqlx::test]
    async fn list_attached_meetings_for_agent_returns_titles_in_attach_order(pool: PgPool) {
        let uid = test_user(&pool).await;
        let now = chrono::Utc::now();

        let parent = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &parent, &uid, now, Some("live meeting"), "{}", None)
            .await
            .unwrap();

        // Past meeting A: title comes from metadata.title.
        let past_a = uuid::Uuid::new_v4().to_string();
        insert_meeting(
            &pool,
            &past_a,
            &uid,
            now,
            None,
            r#"{"title":"Q1 review"}"#,
            None,
        )
        .await
        .unwrap();
        // Past meeting B: no metadata title — exercises the
        // pick_meeting_title fallback to the first description line.
        let past_b = uuid::Uuid::new_v4().to_string();
        insert_meeting(
            &pool,
            &past_b,
            &uid,
            now,
            Some("Vendor sync\nsecond line is ignored"),
            "{}",
            None,
        )
        .await
        .unwrap();

        attach_meeting_to_meeting(&pool, &parent, &past_a)
            .await
            .unwrap();
        // attached_at defaults to NOW(); sleep so the two rows get
        // distinct timestamps and ORDER BY attached_at is deterministic.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        attach_meeting_to_meeting(&pool, &parent, &past_b)
            .await
            .unwrap();

        let metas = list_attached_meetings_for_agent(&pool, &parent, &uid)
            .await
            .expect("decode must succeed against the TEXT metadata column");
        assert_eq!(metas.len(), 2);
        assert_eq!(metas[0].id, past_a, "attach order must be preserved");
        assert_eq!(metas[0].title, "Q1 review");
        assert_eq!(metas[1].id, past_b);
        assert_eq!(
            metas[1].title, "Vendor sync",
            "no metadata.title → first non-empty description line"
        );
    }

    #[sqlx::test]
    async fn insert_recovered_meeting_is_idempotent(pool: PgPool) {
        let uid = test_user(&pool).await;
        let id = uuid::Uuid::new_v4().to_string();
        let ended = chrono::Utc::now();
        let started = ended - chrono::Duration::hours(1);

        insert_recovered_meeting(&pool, &id, &uid, started, ended, Some("recovered"), "{}")
            .await
            .unwrap();
        // Re-run must be a clean no-op — recover-meeting relies on
        // ON CONFLICT (id) DO NOTHING for safe re-runs (--force).
        insert_recovered_meeting(&pool, &id, &uid, started, ended, Some("recovered"), "{}")
            .await
            .unwrap();

        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM meetings WHERE id = $1")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.0, 1, "second insert must not duplicate or error");

        let ended_row: (Option<DateTime<Utc>>,) =
            sqlx::query_as("SELECT ended_at FROM meetings WHERE id = $1")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            ended_row.0.is_some(),
            "recovered meetings are inserted already-ended"
        );
    }

    #[sqlx::test]
    async fn insert_recovered_meeting_rejects_unknown_user(pool: PgPool) {
        // Pins the FK invariant that broke the recovery tool:
        // meetings.user_id REFERENCES users(id) — the internal UUID —
        // so binding a raw Auth0 sub must fail, not silently succeed.
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        let err = insert_recovered_meeting(
            &pool,
            &id,
            "google-oauth2|1234567890", // an Auth0 sub, NOT a users.id
            now,
            now,
            None,
            "{}",
        )
        .await
        .expect_err("inserting with a non-users.id user_id must FK-fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("foreign key"),
            "expected a foreign-key violation, got: {msg}"
        );
    }

    #[sqlx::test]
    async fn list_meeting_llm_usage_round_trips_pool_rows_ordered(pool: PgPool) {
        let uid = test_user(&pool).await;
        let id = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &id, &uid, chrono::Utc::now(), None, "{}", None)
            .await
            .unwrap();
        // Insert deliberately out of alphabetical order to prove the
        // ORDER BY pool: "chat" first, then "background".
        insert_meeting_llm_usage(&pool, &id, "chat", "xai", "grok-4.3", 3, 100, 50, 20)
            .await
            .unwrap();
        insert_meeting_llm_usage(
            &pool,
            &id,
            "background",
            "xai",
            "grok-4.1-fast",
            5,
            200,
            80,
            0,
        )
        .await
        .unwrap();

        let rows = list_meeting_llm_usage(&pool, &id).await.unwrap();
        assert_eq!(rows.len(), 2);
        // Sorted by pool ASC — "background" < "chat".
        assert_eq!(rows[0].pool, "background");
        assert_eq!(rows[0].provider, "xai");
        assert_eq!(rows[0].model_id, "grok-4.1-fast");
        assert_eq!(rows[0].calls, 5);
        assert_eq!(rows[0].input_tokens, 200);
        assert_eq!(rows[0].output_tokens, 80);
        assert_eq!(rows[0].cached_input_tokens, 0);
        assert_eq!(rows[1].pool, "chat");
        assert_eq!(rows[1].model_id, "grok-4.3");
        assert_eq!(rows[1].calls, 3);
        assert_eq!(rows[1].input_tokens, 100);
        assert_eq!(rows[1].output_tokens, 50);
        assert_eq!(rows[1].cached_input_tokens, 20);

        // Unknown meeting → empty vec, not an error.
        let none = list_meeting_llm_usage(&pool, "no-such-meeting")
            .await
            .unwrap();
        assert!(none.is_empty());
    }

    /// Improvement #24: a meeting that ENDED but still reads
    /// wrap_up_status='running' was orphaned by a restart that killed
    /// the detached finalize/retry task. The boot sweep must flip it
    /// to 'failed' so the clients' failed banner + retry paths apply.
    #[sqlx::test]
    async fn fail_orphaned_wrap_ups_flips_running_on_ended_meeting(pool: PgPool) {
        let uid = test_user(&pool).await;
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        insert_meeting(&pool, &id, &uid, now, None, "{}", None)
            .await
            .unwrap();
        end_meeting(&pool, &id, now).await.unwrap();
        set_wrap_up_status(&pool, &id, "running").await.unwrap();

        let swept = fail_orphaned_wrap_ups(&pool).await.unwrap();

        assert_eq!(swept, 1, "exactly the one orphaned row should be swept");
        let row: (Option<String>,) =
            sqlx::query_as("SELECT wrap_up_status FROM meetings WHERE id = $1")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0.as_deref(), Some("failed"));
    }

    /// An ACTIVE meeting (ended_at IS NULL) at 'running' belongs to
    /// `recover_active_meetings` — its rehydrated pipeline will run a
    /// fresh finalize that overwrites the status. The sweep must not
    /// touch it.
    #[sqlx::test]
    async fn fail_orphaned_wrap_ups_spares_active_meeting(pool: PgPool) {
        let uid = test_user(&pool).await;
        let id = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &id, &uid, chrono::Utc::now(), None, "{}", None)
            .await
            .unwrap();
        // No end_meeting: ended_at stays NULL.
        set_wrap_up_status(&pool, &id, "running").await.unwrap();

        let swept = fail_orphaned_wrap_ups(&pool).await.unwrap();

        assert_eq!(swept, 0);
        let row: (Option<String>,) =
            sqlx::query_as("SELECT wrap_up_status FROM meetings WHERE id = $1")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            row.0.as_deref(),
            Some("running"),
            "active meeting must be left for boot recovery, not swept"
        );
    }

    /// Terminal ('success' / 'failed') and legacy (NULL) statuses on
    /// ended meetings are untouched by the sweep.
    #[sqlx::test]
    async fn fail_orphaned_wrap_ups_ignores_terminal_and_null(pool: PgPool) {
        let uid = test_user(&pool).await;
        let now = chrono::Utc::now();

        let mut ids: Vec<(String, Option<&str>)> = Vec::new();
        for status in [Some("success"), Some("failed"), None] {
            let id = uuid::Uuid::new_v4().to_string();
            insert_meeting(&pool, &id, &uid, now, None, "{}", None)
                .await
                .unwrap();
            end_meeting(&pool, &id, now).await.unwrap();
            if let Some(s) = status {
                set_wrap_up_status(&pool, &id, s).await.unwrap();
            }
            ids.push((id, status));
        }

        let swept = fail_orphaned_wrap_ups(&pool).await.unwrap();
        assert_eq!(swept, 0);

        for (id, expected) in &ids {
            let row: (Option<String>,) =
                sqlx::query_as("SELECT wrap_up_status FROM meetings WHERE id = $1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(
                row.0.as_deref(),
                *expected,
                "status {expected:?} on ended meeting {id} must be untouched"
            );
        }
    }
}
