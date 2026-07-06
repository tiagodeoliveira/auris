use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{downcast_db, require_user, ApiError, ApiState, ArtifactDto, MomentDto};

#[derive(Debug, Serialize)]
pub(crate) struct MeetingSummary {
    pub id: String,
    pub description: Option<String>,
    pub metadata: serde_json::Value,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct MeetingDetail {
    id: String,
    description: Option<String>,
    metadata: serde_json::Value,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    /// Full transcript, hydrated from the per-meeting jsonl blob.
    /// Empty when the meeting has no committed transcript items
    /// (e.g., a meeting that started but ended before any speech).
    transcript: Vec<crate::protocol::Item>,
    /// Moments captured during this meeting, oldest first.
    moments: Vec<MomentDto>,
    /// Library artifacts attached to this meeting, in attach order.
    /// PLAN.md §3.7. Empty when none were attached.
    artifacts: Vec<ArtifactDto>,
    /// Persisted items for every non-transcript mode, grouped by
    /// mode id (highlights / actions / open_questions / summary /
    /// chat). Empty arrays for modes that produced no items.
    /// Transcript items are NOT in this map — they live in the
    /// `transcript` field above, sourced from the JSONL blob.
    items_by_mode: std::collections::HashMap<String, Vec<crate::protocol::Item>>,
    /// LLM usage for this meeting (recorded at stop). Aggregated
    /// across the per-pool rows in `meeting_llm_usage` (migration
    /// 0011) when any exist; otherwise read from the legacy
    /// single-pool `meetings.llm_*` columns (migration 0004) so
    /// pre-split meetings keep their history. All zero + null
    /// `provider`/`model_id` on meetings that predate 0004 or hit a
    /// failure path that bypassed the usage persist. `provider` /
    /// `model_id` are also null when the pools disagree (they
    /// intentionally run different models) — see
    /// `llm_usage_by_pool` for exact per-pool attribution.
    llm_usage: MeetingLlmUsage,
    /// Raw per-pool usage rows ("background" / "chat", sorted by
    /// pool name), each with its own provider + model so per-token
    /// rates can be applied per pool. Empty for meetings finalized
    /// before the pool split (their usage only exists in the legacy
    /// aggregate above) and for meetings with no recorded usage.
    llm_usage_by_pool: Vec<PoolLlmUsageDto>,
    /// Post-meeting wrap-up extractor state. One of:
    ///   - `null` — legacy meeting (predates the extractor) or one
    ///     that never had a transcript to extract from
    ///   - `"running"` — the extractor task is still in flight; UI
    ///     can show a subtle "still extracting…" hint
    ///   - `"success"` — extractor finished cleanly. Zero items is
    ///     a legitimate success outcome (nothing extractable)
    ///   - `"failed"` — extractor errored (LLM timeout, quota, etc.);
    ///     UI renders a banner with a retry option
    wrap_up_status: Option<String>,
}

#[derive(Debug, Serialize)]
struct MeetingLlmUsage {
    calls: i64,
    input_tokens: i64,
    output_tokens: i64,
    cached_input_tokens: i64,
    /// "bedrock" / "openai" / "anthropic". `None` for meetings
    /// from before the migration.
    provider: Option<String>,
    /// e.g. "claude-opus-4-7". `None` for meetings from before
    /// the migration.
    model_id: Option<String>,
}

/// One per-pool usage row, mirroring `meeting_llm_usage` (and
/// `storage::meetings::MeetingLlmUsageRow`) on the wire.
#[derive(Debug, Serialize)]
struct PoolLlmUsageDto {
    /// Stable pool id — "chat" or "background" (`LlmPool::as_str`).
    pool: String,
    provider: String,
    model_id: String,
    calls: i64,
    input_tokens: i64,
    output_tokens: i64,
    cached_input_tokens: i64,
}

impl From<crate::storage::meetings::MeetingLlmUsageRow> for PoolLlmUsageDto {
    fn from(r: crate::storage::meetings::MeetingLlmUsageRow) -> Self {
        PoolLlmUsageDto {
            pool: r.pool,
            provider: r.provider,
            model_id: r.model_id,
            calls: r.calls,
            input_tokens: r.input_tokens,
            output_tokens: r.output_tokens,
            cached_input_tokens: r.cached_input_tokens,
        }
    }
}

/// `Some(first)` iff every item in the iterator equals the first;
/// `None` on disagreement (mixed-model meeting) or an empty
/// iterator. Used to decide whether the per-pool rows can be
/// collapsed into the aggregate's single `provider` / `model_id`.
fn uniform_value<'a, I: Iterator<Item = &'a str>>(mut values: I) -> Option<String> {
    let first = values.next()?;
    if values.all(|v| v == first) {
        Some(first.to_string())
    } else {
        None
    }
}

/// Row shape that maps 1-to-1 with the SELECT in both handlers.
/// Extracting the conversion keeps the handlers focused on routing.
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct MeetingRow {
    pub id: String,
    pub description: Option<String>,
    pub metadata: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

impl MeetingRow {
    pub(crate) fn into_summary(self) -> MeetingSummary {
        let metadata: serde_json::Value =
            serde_json::from_str(&self.metadata).unwrap_or(serde_json::json!({}));
        MeetingSummary {
            id: self.id,
            description: self.description,
            metadata,
            started_at: self.started_at,
            ended_at: self.ended_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct AttachMeetingBody {
    attached_meeting_id: String,
}

pub(crate) async fn list_meetings(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Vec<MeetingSummary>>, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    let rows: Vec<MeetingRow> = sqlx::query_as(
        r#"SELECT id, description, metadata, started_at, ended_at
           FROM meetings
           WHERE user_id = $1
           ORDER BY started_at DESC"#,
    )
    .bind(&user_id)
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::Db)?;

    let summaries = rows.into_iter().map(MeetingRow::into_summary).collect();
    Ok(Json(summaries))
}

pub(crate) async fn get_meeting(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<MeetingDetail>, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    Ok(Json(build_meeting_detail(&state, &user_id, &id).await?))
}

/// Assemble the full `MeetingDetail` for an owned meeting: the row,
/// wrap-up status, LLM usage, transcript blob, moments, artifacts, and
/// per-mode items. `NotFound` when the id isn't owned by `user_id` (no
/// existence leak). Shared by `get_meeting` and `retry_wrap_up` so the
/// retry response is byte-for-byte the shape the detail view already
/// renders.
async fn build_meeting_detail(
    state: &ApiState,
    user_id: &str,
    id: &str,
) -> Result<MeetingDetail, ApiError> {
    let row: Option<MeetingRow> = sqlx::query_as(
        r#"SELECT id, description, metadata, started_at, ended_at
           FROM meetings WHERE id = $1 AND user_id = $2"#,
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::Db)?;
    let Some(row) = row else {
        return Err(ApiError::NotFound);
    };
    // Wrap-up extractor status — sourced from the `wrap_up_status`
    // column on the meetings row. Separate query so the existing
    // tuple type for usage doesn't grow / refactor unrelated rows.
    let wrap_up_status: Option<String> =
        sqlx::query_scalar(r#"SELECT wrap_up_status FROM meetings WHERE id = $1"#)
            .bind(&row.id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::Db)?
            .flatten();
    // LLM usage. Per-pool rows in `meeting_llm_usage` (migration
    // 0011) are the source of truth for meetings finalized after the
    // chat/background pool split: aggregate them into the original
    // single-object wire shape so existing clients keep rendering,
    // and surface the raw rows in `llm_usage_by_pool`. Meetings that
    // predate 0011 have no pool rows; fall back to the single-pool
    // `meetings.llm_*` columns (migration 0004). Pool rows win
    // outright whenever any exist so the two sources are never
    // double-counted.
    let pool_rows = crate::storage::meetings::list_meeting_llm_usage(&state.db, &row.id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    let llm_usage = if pool_rows.is_empty() {
        // Legacy fallback (pre-pool-split meetings). sqlx tuple type
        // matches the column list verbatim; carving it into a named
        // alias would add indirection without clarifying the shape,
        // since this query only runs in one place.
        #[allow(clippy::type_complexity)]
        let usage_row: Option<(i64, i64, i64, i64, Option<String>, Option<String>)> =
            sqlx::query_as(
                r#"SELECT llm_calls, llm_input_tokens, llm_output_tokens,
                          llm_cached_input_tokens, llm_provider, llm_model_id
                     FROM meetings WHERE id = $1"#,
            )
            .bind(&row.id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::Db)?;
        usage_row
            .map(
                |(calls, input_tokens, output_tokens, cached_input_tokens, provider, model_id)| {
                    MeetingLlmUsage {
                        calls,
                        input_tokens,
                        output_tokens,
                        cached_input_tokens,
                        provider,
                        model_id,
                    }
                },
            )
            .unwrap_or(MeetingLlmUsage {
                calls: 0,
                input_tokens: 0,
                output_tokens: 0,
                cached_input_tokens: 0,
                provider: None,
                model_id: None,
            })
    } else {
        MeetingLlmUsage {
            calls: pool_rows.iter().map(|r| r.calls).sum(),
            input_tokens: pool_rows.iter().map(|r| r.input_tokens).sum(),
            output_tokens: pool_rows.iter().map(|r| r.output_tokens).sum(),
            cached_input_tokens: pool_rows.iter().map(|r| r.cached_input_tokens).sum(),
            // The pools intentionally run different models, so a
            // single (provider, model_id) pair only represents the
            // meeting when every pool agrees. Mixed → None, a state
            // clients already render-or-skip (pre-0004 meetings).
            provider: uniform_value(pool_rows.iter().map(|r| r.provider.as_str())),
            model_id: uniform_value(pool_rows.iter().map(|r| r.model_id.as_str())),
        }
    };
    let llm_usage_by_pool: Vec<PoolLlmUsageDto> =
        pool_rows.into_iter().map(PoolLlmUsageDto::from).collect();
    let transcript = read_transcript(&row.id).await;
    let moments = crate::storage::moments::list_moments_for_meeting(&state.db, &row.id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?
        .into_iter()
        .map(|r| MomentDto::from_row(r, &row.id))
        .collect();
    let artifacts: Vec<ArtifactDto> =
        crate::storage::artifacts::list_artifacts_for_meeting(&state.db, &row.id)
            .await
            .map_err(|e| ApiError::Db(downcast_db(e)))?
            .into_iter()
            .map(ArtifactDto::from)
            .collect();
    let items_by_mode = crate::storage::items::list_items_for_meeting_grouped(&state.db, &row.id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    let MeetingSummary {
        id,
        description,
        metadata,
        started_at,
        ended_at,
    } = row.into_summary();
    Ok(MeetingDetail {
        id,
        description,
        metadata,
        started_at,
        ended_at,
        transcript,
        moments,
        artifacts,
        items_by_mode,
        llm_usage,
        llm_usage_by_pool,
        wrap_up_status,
    })
}

/// `POST /meetings/:id/retry-wrap-up` — (re)generate the post-meeting
/// extractors (summary + highlights AND actions + open_questions) for a
/// FINISHED meeting. Flips `wrap_up_status` to `running` and hands the
/// job to the retry worker, which reads the persisted transcript and
/// re-runs the same extractors the live finalize path uses. Returns the
/// refreshed `MeetingDetail` (now `running`) so the client can show the
/// in-flight banner without a follow-up GET.
///
/// Usable at any time, not just after a failure: the worker is
/// idempotent (replace-by-mode), so regenerating a meeting that already
/// has a wrap-up overwrites it cleanly rather than duplicating. The Mac
/// UI exposes this as "Regenerate wrap-up" on any past meeting, plus the
/// failed-banner "Try again".
///
/// State guard:
///   - 404 — unknown id or owned by another user (no existence leak).
///   - 400 `in_progress` — meeting hasn't ended (`ended_at` is NULL);
///     there's no complete transcript to (re)extract from yet.
///   - 400 `already_running` — a regeneration is already in flight; a
///     second would race the worker.
///   - otherwise (`failed` / `success` / legacy `null`) → accepted.
pub(crate) async fn retry_wrap_up(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<MeetingDetail>, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    // Ownership + current status + ended-ness in one query. `None` →
    // unknown / owned by someone else (404, no existence leak).
    let row: Option<(Option<String>, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"SELECT wrap_up_status, ended_at FROM meetings WHERE id = $1 AND user_id = $2"#,
    )
    .bind(&id)
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::Db)?;
    let Some((status, ended_at)) = row else {
        return Err(ApiError::NotFound);
    };
    // Only finished meetings: the retry worker extracts from the
    // persisted transcript blob, which is only complete once finalize
    // has run. Regenerating a live meeting would extract a partial
    // transcript and race the live pipeline.
    if ended_at.is_none() {
        return Err(ApiError::BadRequest(
            "meeting is still in progress; wrap-up can only be (re)generated once it has ended"
                .to_string(),
        ));
    }
    // Don't stack regenerations — a second would race the worker
    // (both replace the same item slices).
    if status.as_deref() == Some("running") {
        return Err(ApiError::BadRequest(
            "wrap-up regeneration is already running for this meeting".to_string(),
        ));
    }
    // Flip to `running` up front so the returned detail — and any
    // client polling GET /meetings/:id — shows the in-flight banner
    // immediately, before the worker picks the job up.
    crate::storage::meetings::set_wrap_up_status(&state.db, &id, "running")
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    // Hand off to the retry worker. Closed-channel send is silent —
    // a router built without the worker (tests) still serves the
    // endpoint and reports the `running` transition.
    let _ = state.wrap_up_retry_tx.send(crate::api::WrapUpRetry {
        user_id: user_id.clone(),
        meeting_id: id.clone(),
    });
    Ok(Json(build_meeting_detail(&state, &user_id, &id).await?))
}

/// `GET /meetings/:id/export.pdf` — render the meeting as a PDF
/// document. Same data the detail view shows, plus larger moment
/// screenshots interleaved with the transcript. Returns
/// `application/pdf` with a `Content-Disposition: attachment;
/// filename="..."` so a plain `<a download>` on the client works.
///
/// The PDF render itself is synchronous (printpdf is sync); we wrap
/// in `spawn_blocking` so a large meeting (long transcript + many
/// moments) doesn't park the async runtime thread.
pub(crate) async fn export_meeting_pdf(
    State(state): State<ApiState>,
    Path(meeting_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    // Reuse the same fetch path as get_meeting — keeps the PDF
    // contents in lockstep with the JSON detail view.
    let row: Option<MeetingRow> = sqlx::query_as(
        r#"SELECT id, description, metadata, started_at, ended_at
           FROM meetings WHERE id = $1 AND user_id = $2"#,
    )
    .bind(&meeting_id)
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::Db)?;
    let Some(row) = row else {
        return Err(ApiError::NotFound);
    };

    let transcript = read_transcript(&row.id).await;
    // Per-mode items (highlights / actions / open_questions /
    // summary / chat). The detail JSON endpoint pulls this same set;
    // PDF renders each non-empty mode as its own section before the
    // transcript.
    let items_by_mode = crate::storage::items::list_items_for_meeting_grouped(&state.db, &row.id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    let moments = crate::storage::moments::list_moments_for_meeting(&state.db, &row.id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    let data_dir = crate::storage::data_dir().map_err(|e| ApiError::Internal(e.to_string()))?;

    // Load screenshots from disk for each moment that has one.
    // Failures (deleted file, IO error) degrade to "no image" —
    // moment header + summary still render. Uses `tokio::fs` so
    // the async runtime thread isn't parked on the IO syscalls
    // for meetings with many screenshots.
    let mut renderable_moments: Vec<crate::pdf::RenderableMoment> =
        Vec::with_capacity(moments.len());
    for m in moments {
        let bytes = match m.asset_path.as_ref() {
            Some(asset) => {
                let abs = data_dir.join(asset);
                match tokio::fs::read(&abs).await {
                    Ok(b) => Some(b),
                    Err(e) => {
                        tracing::warn!(moment = %m.id, path = ?abs, error = %e, "moment screenshot read failed");
                        None
                    }
                }
            }
            None => None,
        };
        renderable_moments.push(crate::pdf::RenderableMoment {
            id: m.id,
            t: m.t,
            note: m.note,
            summary: m.summary,
            screenshot_bytes: bytes,
        });
    }

    // Build the metadata vec (sorted by key for stable ordering).
    let metadata_obj: serde_json::Value =
        serde_json::from_str(&row.metadata).unwrap_or(serde_json::json!({}));
    let mut metadata: Vec<(String, String)> = metadata_obj
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| {
                    let v_str = match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    (k.clone(), v_str)
                })
                .collect()
        })
        .unwrap_or_default();
    metadata.sort_by(|a, b| a.0.cmp(&b.0));

    // Title fallback chain mirrors `pickMeetingTitle` in the clients.
    let title = pick_title(&row.description, &metadata);

    let input = crate::pdf::PdfInput {
        id: row.id.clone(),
        title,
        description: row.description.clone(),
        started_at: row.started_at,
        ended_at: row.ended_at,
        metadata,
        transcript,
        items_by_mode,
        moments: renderable_moments,
    };

    // Spawn the blocking render — printpdf + image-decoding can run
    // for hundreds of ms on a big meeting, and we don't want to park
    // the async runtime thread.
    let pdf_bytes = tokio::task::spawn_blocking(move || crate::pdf::render(&input))
        .await
        .map_err(|e| ApiError::Internal(format!("pdf render task panicked: {e}")))?
        .map_err(|e| ApiError::Internal(format!("pdf render failed: {e}")))?;

    let filename = format!("meeting-{}.pdf", &meeting_id);
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/pdf")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(header::CACHE_CONTROL, "private, max-age=0, must-revalidate")
        .body(Body::from(pdf_bytes))
        .expect("axum Response::builder is infallible for static headers"))
}

/// Pick a human-readable title for the PDF — mirrors the
/// `pickMeetingTitle` helper used on every client so the export
/// title matches what the user saw in the detail view.
fn pick_title(description: &Option<String>, metadata: &[(String, String)]) -> String {
    if let Some((_, v)) = metadata.iter().find(|(k, _)| k == "title") {
        let t = v.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Some(desc) = description {
        let first_line = desc.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        let trimmed = first_line.trim();
        if !trimmed.is_empty() {
            // Char-based, not byte-based: LLM-generated descriptions
            // contain multi-byte UTF-8 (em dashes, curly quotes,
            // accents) and a byte slice can cut mid-character and
            // panic. Mirrors the clients' char/grapheme semantics.
            if trimmed.chars().count() <= 80 {
                return trimmed.to_string();
            }
            let prefix: String = trimmed.chars().take(79).collect();
            return format!("{prefix}…");
        }
    }
    "Untitled meeting".to_string()
}

/// Upper bound on a user-set meeting title. Generous enough for a
/// descriptive sentence, tight enough to keep list rows sane.
const MAX_TITLE_LEN: usize = 200;

/// Set the `title` tag inside a meeting's metadata JSON blob, returning
/// the serialized JSON to persist. Errors (→ 400) when the title is
/// empty after trimming or longer than `MAX_TITLE_LEN`. A malformed or
/// empty stored blob is tolerated by starting from an empty object, so
/// renaming never fails on a meeting whose metadata predates a schema
/// the parser expects.
fn set_title(metadata_json: &str, title: &str) -> Result<String, String> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err("title must not be empty".to_string());
    }
    if trimmed.chars().count() > MAX_TITLE_LEN {
        return Err(format!("title must be at most {MAX_TITLE_LEN} characters"));
    }
    let mut obj: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(metadata_json).unwrap_or_default();
    obj.insert(
        "title".to_string(),
        serde_json::Value::String(trimmed.to_string()),
    );
    serde_json::to_string(&serde_json::Value::Object(obj)).map_err(|e| e.to_string())
}

/// `DELETE /meetings/:id` — remove a meeting plus its moments
/// (FK cascade) and the on-disk blob directory holding the
/// transcript JSONL and any screenshots. 204 on success, 404 if
/// the id is unknown. Disk cleanup runs after the DB delete; if
/// that step fails we still return 204 — the row is gone, the
/// blobs are orphans that future cleanup can reap.
pub(crate) async fn delete_meeting(
    State(state): State<ApiState>,
    Path(meeting_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    let removed =
        crate::storage::meetings::delete_meeting_for_user(&state.db, &meeting_id, &user_id)
            .await
            .map_err(|e| ApiError::Db(downcast_db(e)))?;
    if !removed {
        // 404 covers both "no such id" and "owned by someone else" —
        // we don't distinguish so we don't leak existence.
        return Err(ApiError::NotFound);
    }
    if let Ok(dir) = crate::storage::data_dir() {
        let blob_dir = dir.join("blobs").join("meetings").join(&meeting_id);
        if blob_dir.exists() {
            if let Err(e) = tokio::fs::remove_dir_all(&blob_dir).await {
                tracing::warn!(
                    error = ?e, meeting_id = %meeting_id, path = %blob_dir.display(),
                    "delete_meeting: blob cleanup failed (row already removed)"
                );
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub(crate) struct RenameRequest {
    pub title: String,
}

/// `PATCH /meetings/:id` — rename a meeting by setting its `title`
/// metadata tag. 204 on success, 404 if the id isn't owned by the
/// caller (no existence leak), 400 on an empty / over-long title.
/// Read-modify-write of the metadata JSON blob; safe on a finished
/// meeting since nothing else writes `metadata.title` after a meeting
/// ends (the auto title is extracted from the description at start).
pub(crate) async fn rename_meeting(
    State(state): State<ApiState>,
    Path(meeting_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<RenameRequest>,
) -> Result<StatusCode, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    let row: Option<(String,)> =
        sqlx::query_as(r#"SELECT metadata FROM meetings WHERE id = $1 AND user_id = $2"#)
            .bind(&meeting_id)
            .bind(&user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::Db)?;
    let Some((metadata,)) = row else {
        return Err(ApiError::NotFound);
    };
    let updated = set_title(&metadata, &body.title).map_err(ApiError::BadRequest)?;
    sqlx::query(r#"UPDATE meetings SET metadata = $1 WHERE id = $2 AND user_id = $3"#)
        .bind(&updated)
        .bind(&meeting_id)
        .bind(&user_id)
        .execute(&state.db)
        .await
        .map_err(ApiError::Db)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /meetings/:id/attached_meetings` — attach a past meeting to
/// the parent so the agent's `recall_meeting` tool can recall scoped
/// to that past meeting's mnemo namespace.
/// Both meetings must belong to the caller. Self-attach is rejected.
/// Idempotent — re-attaching the same pair is a silent no-op.
pub(crate) async fn attach_meeting(
    State(state): State<ApiState>,
    Path(parent_meeting_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AttachMeetingBody>,
) -> Result<StatusCode, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    if parent_meeting_id == body.attached_meeting_id {
        return Err(ApiError::BadRequest(
            "a meeting cannot attach itself".into(),
        ));
    }
    // Ownership: both parent and attached must belong to the caller.
    // Two separate lookups so a 404 doesn't leak which side missed.
    let parent_owned: Option<(String,)> =
        sqlx::query_as(r#"SELECT id FROM meetings WHERE id = $1 AND user_id = $2"#)
            .bind(&parent_meeting_id)
            .bind(&user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::Db)?;
    if parent_owned.is_none() {
        return Err(ApiError::NotFound);
    }
    let attached_owned: Option<(String,)> =
        sqlx::query_as(r#"SELECT id FROM meetings WHERE id = $1 AND user_id = $2"#)
            .bind(&body.attached_meeting_id)
            .bind(&user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::Db)?;
    if attached_owned.is_none() {
        return Err(ApiError::NotFound);
    }
    crate::storage::meetings::attach_meeting_to_meeting(
        &state.db,
        &parent_meeting_id,
        &body.attached_meeting_id,
    )
    .await
    .map_err(|e| ApiError::Db(downcast_db(e)))?;
    // Kick the agent so it picks up the attached meeting on its next
    // fire (same UX as artifact attach).
    let _ = state.agent_kick_tx.send(crate::agent::AgentKick {
        user_id: user_id.clone(),
        reason: crate::agent::AgentKickReason::MeetingAttached {
            attached_meeting_id: body.attached_meeting_id.clone(),
        },
    });
    broadcast_attached_meetings_changed(&state, &parent_meeting_id, &user_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Read the canonical attached-meetings set from DB and broadcast
/// `Event::AttachedMeetingsChanged` to the user's WS clients. Same
/// best-effort shape as the artifact mirror.
async fn broadcast_attached_meetings_changed(
    state: &ApiState,
    parent_meeting_id: &str,
    user_id: &str,
) {
    let ids =
        match crate::storage::meetings::list_attached_meeting_ids(&state.db, parent_meeting_id)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    error = ?e,
                    parent_meeting_id,
                    "broadcast_attached_meetings_changed: list failed",
                );
                return;
            }
        };
    state
        .bus
        .emit(
            user_id.to_string(),
            crate::protocol::Event::AttachedMeetingsChanged { meeting_ids: ids },
        )
        .await;
}

/// `DELETE /meetings/:id/attached_meetings/:attached_id` — drop the
/// attachment row + broadcast the new set. No agent kick on detach
/// (matches the artifact detach shape).
pub(crate) async fn detach_meeting(
    State(state): State<ApiState>,
    Path((parent_meeting_id, attached_meeting_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    let parent_owned: Option<(String,)> =
        sqlx::query_as(r#"SELECT id FROM meetings WHERE id = $1 AND user_id = $2"#)
            .bind(&parent_meeting_id)
            .bind(&user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::Db)?;
    if parent_owned.is_none() {
        return Err(ApiError::NotFound);
    }
    crate::storage::meetings::detach_meeting_from_meeting(
        &state.db,
        &parent_meeting_id,
        &attached_meeting_id,
    )
    .await
    .map_err(|e| ApiError::Db(downcast_db(e)))?;
    broadcast_attached_meetings_changed(&state, &parent_meeting_id, &user_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Best-effort read of the per-meeting transcription jsonl. Lines
/// that fail to parse are skipped silently — better to surface a
/// partial transcript than to fail the whole meeting view because
/// of a single malformed row.
async fn read_transcript(meeting_id: &str) -> Vec<crate::protocol::Item> {
    let path = match crate::storage::persistence_loop::transcription_path(meeting_id) {
        Ok(p) => p,
        Err(_) => return vec![],
    };
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<crate::protocol::Item>(line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sqlx::PgPool;
    use std::sync::Arc;
    use tokio::sync::broadcast;
    use tower::ServiceExt;

    /// Build a router in `AuthMode::Disabled` (every request maps
    /// to the synthetic `dev|local` user). Returns the router *and*
    /// the local `users.id` so test fixtures can insert meetings
    /// under the right owner.
    async fn router_with_dev_user(pool: PgPool) -> (axum::Router, String) {
        let (moment_created_tx, _) = broadcast::channel::<crate::api::MomentCreated>(8);
        let (artifact_created_tx, _) = broadcast::channel::<crate::api::ArtifactCreated>(8);
        let (wrap_up_retry_tx, _) = broadcast::channel::<crate::api::WrapUpRetry>(8);
        let (agent_kick_tx, _) = broadcast::channel::<crate::agent::AgentKick>(8);
        let (fanout, _) = broadcast::channel::<crate::protocol::UserEvent>(8);
        let (durable_tx, _durable_rx) = tokio::sync::mpsc::channel::<crate::protocol::UserEvent>(8);
        let bus = crate::context::EventBus::new(fanout, durable_tx);
        let dev_user = crate::storage::users::upsert_user_by_auth0_sub(
            &pool,
            crate::auth::DEV_AUTH0_SUB,
            Some("dev@local"),
            Some("Local Dev"),
        )
        .await
        .unwrap();
        let router = crate::api::make_router(ApiState {
            db: pool,
            auth: Arc::new(crate::auth::AuthMode::Disabled),
            moment_created_tx,
            artifact_created_tx,
            wrap_up_retry_tx,
            agent_kick_tx,
            bus,
        });
        (router, dev_user.id)
    }

    async fn body_string(resp: axum::response::Response) -> String {
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    #[sqlx::test]
    async fn list_returns_meetings_newest_first(pool: PgPool) {
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        let earlier = Utc::now() - chrono::Duration::minutes(60);
        let later = Utc::now();
        crate::storage::meetings::insert_meeting(
            &pool,
            "older",
            &uid,
            earlier,
            Some("first"),
            "{}",
            None,
        )
        .await
        .unwrap();
        crate::storage::meetings::insert_meeting(
            &pool,
            "newer",
            &uid,
            later,
            Some("second"),
            "{}",
            None,
        )
        .await
        .unwrap();
        let resp = app
            .oneshot(
                Request::get("/meetings")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let v: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0]["id"], "newer");
        assert_eq!(v[1]["id"], "older");
    }

    #[sqlx::test]
    async fn detail_404_for_missing_meeting(pool: PgPool) {
        let (app, _uid) = router_with_dev_user(pool).await;
        let resp = app
            .oneshot(
                Request::get("/meetings/no-such-id")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn detail_returns_meeting_with_empty_transcript(pool: PgPool) {
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::storage::meetings::insert_meeting(
            &pool,
            "m1",
            &uid,
            Utc::now(),
            Some("hi"),
            r#"{"a":"b"}"#,
            None,
        )
        .await
        .unwrap();
        let resp = app
            .oneshot(
                Request::get("/meetings/m1")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(v["id"], "m1");
        assert_eq!(v["description"], "hi");
        assert_eq!(v["metadata"]["a"], "b");
        assert!(v["transcript"].as_array().unwrap().is_empty());
        // No pool rows AND untouched legacy columns → all-zero
        // aggregate with an empty per-pool list (improvement #17).
        assert_eq!(v["llm_usage"]["calls"], 0);
        assert!(v["llm_usage"]["provider"].is_null());
        assert!(v["llm_usage_by_pool"].as_array().unwrap().is_empty());
    }

    #[sqlx::test]
    async fn detail_404_when_meeting_belongs_to_other_user(pool: PgPool) {
        // Insert a meeting under a *different* user.
        let other =
            crate::storage::users::upsert_user_by_auth0_sub(&pool, "other|user", None, None)
                .await
                .unwrap();
        crate::storage::meetings::insert_meeting(
            &pool,
            "other-m",
            &other.id,
            Utc::now(),
            None,
            "{}",
            None,
        )
        .await
        .unwrap();
        // Query as the dev user — the meeting exists but belongs to
        // `other`, so we surface 404 (no existence leak).
        let (app, _uid) = router_with_dev_user(pool).await;
        let resp = app
            .oneshot(
                Request::get("/meetings/other-m")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn retry_wrap_up_flips_failed_to_running(pool: PgPool) {
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::storage::meetings::insert_meeting(&pool, "m1", &uid, Utc::now(), None, "{}", None)
            .await
            .unwrap();
        crate::storage::meetings::end_meeting(&pool, "m1", Utc::now())
            .await
            .unwrap();
        crate::storage::meetings::set_wrap_up_status(&pool, "m1", "failed")
            .await
            .unwrap();
        let resp = app
            .oneshot(
                Request::post("/meetings/m1/retry-wrap-up")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // The response reflects the freshly-flipped status…
        let v: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(v["wrap_up_status"], "running");
        // …and it's persisted, so a client polling GET /meetings/:id agrees.
        let persisted: Option<String> =
            sqlx::query_scalar(r#"SELECT wrap_up_status FROM meetings WHERE id = $1"#)
                .bind("m1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(persisted.as_deref(), Some("running"));
    }

    #[sqlx::test]
    async fn retry_wrap_up_allows_regenerate_on_finished_success(pool: PgPool) {
        // "Regenerate at any time": a finished meeting that already
        // succeeded can be re-run (the worker is idempotent). Flips to
        // running.
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::storage::meetings::insert_meeting(&pool, "m1", &uid, Utc::now(), None, "{}", None)
            .await
            .unwrap();
        crate::storage::meetings::end_meeting(&pool, "m1", Utc::now())
            .await
            .unwrap();
        crate::storage::meetings::set_wrap_up_status(&pool, "m1", "success")
            .await
            .unwrap();
        let resp = app
            .oneshot(
                Request::post("/meetings/m1/retry-wrap-up")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(v["wrap_up_status"], "running");
    }

    #[sqlx::test]
    async fn retry_wrap_up_rejects_in_progress_meeting(pool: PgPool) {
        // A meeting that hasn't ended (ended_at NULL) has no complete
        // transcript to (re)extract from — 400, not a regeneration.
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::storage::meetings::insert_meeting(&pool, "m1", &uid, Utc::now(), None, "{}", None)
            .await
            .unwrap();
        let resp = app
            .oneshot(
                Request::post("/meetings/m1/retry-wrap-up")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[sqlx::test]
    async fn retry_wrap_up_rejects_already_running(pool: PgPool) {
        // Don't stack regenerations — a second would race the worker.
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::storage::meetings::insert_meeting(&pool, "m1", &uid, Utc::now(), None, "{}", None)
            .await
            .unwrap();
        crate::storage::meetings::end_meeting(&pool, "m1", Utc::now())
            .await
            .unwrap();
        crate::storage::meetings::set_wrap_up_status(&pool, "m1", "running")
            .await
            .unwrap();
        let resp = app
            .oneshot(
                Request::post("/meetings/m1/retry-wrap-up")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[sqlx::test]
    async fn retry_wrap_up_404_for_unknown_meeting(pool: PgPool) {
        let (app, _uid) = router_with_dev_user(pool).await;
        let resp = app
            .oneshot(
                Request::post("/meetings/no-such-id/retry-wrap-up")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn retry_wrap_up_404_for_other_users_meeting(pool: PgPool) {
        // Even a `failed` meeting is invisible to a non-owner — 404,
        // not 400, so we don't leak that the meeting exists.
        let other =
            crate::storage::users::upsert_user_by_auth0_sub(&pool, "other|user", None, None)
                .await
                .unwrap();
        crate::storage::meetings::insert_meeting(
            &pool,
            "other-m",
            &other.id,
            Utc::now(),
            None,
            "{}",
            None,
        )
        .await
        .unwrap();
        crate::storage::meetings::set_wrap_up_status(&pool, "other-m", "failed")
            .await
            .unwrap();
        let (app, _uid) = router_with_dev_user(pool).await;
        let resp = app
            .oneshot(
                Request::post("/meetings/other-m/retry-wrap-up")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn retry_wrap_up_publishes_retry_signal(pool: PgPool) {
        // Build a router holding a captured retry receiver so we can
        // assert the handler actually enqueues the re-run job — not
        // just that it flips the DB status. This is the "it triggers a
        // re-run" guarantee the feature exists for; without the signal
        // the worker never runs and the banner stays stuck on running.
        let (moment_created_tx, _) = broadcast::channel::<crate::api::MomentCreated>(8);
        let (artifact_created_tx, _) = broadcast::channel::<crate::api::ArtifactCreated>(8);
        let (wrap_up_retry_tx, mut retry_rx) = broadcast::channel::<crate::api::WrapUpRetry>(8);
        let (agent_kick_tx, _) = broadcast::channel::<crate::agent::AgentKick>(8);
        let (fanout, _) = broadcast::channel::<crate::protocol::UserEvent>(8);
        let (durable_tx, _durable_rx) = tokio::sync::mpsc::channel::<crate::protocol::UserEvent>(8);
        let bus = crate::context::EventBus::new(fanout, durable_tx);
        let uid = crate::storage::users::upsert_user_by_auth0_sub(
            &pool,
            crate::auth::DEV_AUTH0_SUB,
            None,
            None,
        )
        .await
        .unwrap()
        .id;
        crate::storage::meetings::insert_meeting(&pool, "m1", &uid, Utc::now(), None, "{}", None)
            .await
            .unwrap();
        crate::storage::meetings::end_meeting(&pool, "m1", Utc::now())
            .await
            .unwrap();
        crate::storage::meetings::set_wrap_up_status(&pool, "m1", "failed")
            .await
            .unwrap();
        let app = crate::api::make_router(ApiState {
            db: pool,
            auth: Arc::new(crate::auth::AuthMode::Disabled),
            moment_created_tx,
            artifact_created_tx,
            wrap_up_retry_tx,
            agent_kick_tx,
            bus,
        });
        let resp = app
            .oneshot(
                Request::post("/meetings/m1/retry-wrap-up")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let signal = retry_rx.try_recv().expect("retry signal published");
        assert_eq!(signal.meeting_id, "m1");
        assert_eq!(signal.user_id, uid);
    }

    #[sqlx::test]
    async fn detail_llm_usage_aggregates_pool_rows(pool: PgPool) {
        // Regression for improvement #17: meetings finalized after the
        // chat/background pool split (migration 0011) write usage to
        // `meeting_llm_usage`, but the detail endpoint kept reading the
        // orphaned `meetings.llm_*` columns and reported calls = 0.
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::storage::meetings::insert_meeting(&pool, "m1", &uid, Utc::now(), None, "{}", None)
            .await
            .unwrap();
        crate::storage::meetings::insert_meeting_llm_usage(
            &pool, "m1", "chat", "xai", "grok-4.3", 3, 100, 50, 20,
        )
        .await
        .unwrap();
        crate::storage::meetings::insert_meeting_llm_usage(
            &pool,
            "m1",
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
        let resp = app
            .oneshot(
                Request::get("/meetings/m1")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        // Aggregate sums across both pools.
        assert_eq!(v["llm_usage"]["calls"], 8);
        assert_eq!(v["llm_usage"]["input_tokens"], 300);
        assert_eq!(v["llm_usage"]["output_tokens"], 130);
        assert_eq!(v["llm_usage"]["cached_input_tokens"], 20);
        // Provider is uniform across pools → surfaced; models differ → null.
        assert_eq!(v["llm_usage"]["provider"], "xai");
        assert!(v["llm_usage"]["model_id"].is_null());
        // Raw per-pool rows, sorted by pool ("background" < "chat").
        let by_pool = v["llm_usage_by_pool"].as_array().unwrap();
        assert_eq!(by_pool.len(), 2);
        assert_eq!(by_pool[0]["pool"], "background");
        assert_eq!(by_pool[0]["provider"], "xai");
        assert_eq!(by_pool[0]["model_id"], "grok-4.1-fast");
        assert_eq!(by_pool[0]["calls"], 5);
        assert_eq!(by_pool[0]["input_tokens"], 200);
        assert_eq!(by_pool[0]["output_tokens"], 80);
        assert_eq!(by_pool[0]["cached_input_tokens"], 0);
        assert_eq!(by_pool[1]["pool"], "chat");
        assert_eq!(by_pool[1]["model_id"], "grok-4.3");
        assert_eq!(by_pool[1]["calls"], 3);
        assert_eq!(by_pool[1]["input_tokens"], 100);
        assert_eq!(by_pool[1]["output_tokens"], 50);
        assert_eq!(by_pool[1]["cached_input_tokens"], 20);
    }

    #[sqlx::test]
    async fn detail_llm_usage_falls_back_to_legacy_columns(pool: PgPool) {
        // Meetings finalized before migration 0011 only have the
        // single-pool `meetings.llm_*` columns (migration 0004). With
        // zero pool rows, the detail endpoint must surface those.
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::storage::meetings::insert_meeting(&pool, "m1", &uid, Utc::now(), None, "{}", None)
            .await
            .unwrap();
        sqlx::query(
            r#"UPDATE meetings
                  SET llm_calls = 4, llm_input_tokens = 10,
                      llm_output_tokens = 7, llm_cached_input_tokens = 2,
                      llm_provider = 'bedrock', llm_model_id = 'claude-opus-4-7'
                WHERE id = $1"#,
        )
        .bind("m1")
        .execute(&pool)
        .await
        .unwrap();
        let resp = app
            .oneshot(
                Request::get("/meetings/m1")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(v["llm_usage"]["calls"], 4);
        assert_eq!(v["llm_usage"]["input_tokens"], 10);
        assert_eq!(v["llm_usage"]["output_tokens"], 7);
        assert_eq!(v["llm_usage"]["cached_input_tokens"], 2);
        assert_eq!(v["llm_usage"]["provider"], "bedrock");
        assert_eq!(v["llm_usage"]["model_id"], "claude-opus-4-7");
        // Legacy meeting → no per-pool rows, serialized as [].
        assert!(v["llm_usage_by_pool"].as_array().unwrap().is_empty());
    }

    #[sqlx::test]
    async fn detail_llm_usage_prefers_pool_rows_over_legacy(pool: PgPool) {
        // A meeting straddling the 0011 cutover could theoretically
        // have both. Pool rows win outright — no double counting.
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::storage::meetings::insert_meeting(&pool, "m1", &uid, Utc::now(), None, "{}", None)
            .await
            .unwrap();
        sqlx::query(
            r#"UPDATE meetings
                  SET llm_calls = 99, llm_input_tokens = 999,
                      llm_output_tokens = 999, llm_cached_input_tokens = 999,
                      llm_provider = 'bedrock', llm_model_id = 'legacy-model'
                WHERE id = $1"#,
        )
        .bind("m1")
        .execute(&pool)
        .await
        .unwrap();
        crate::storage::meetings::insert_meeting_llm_usage(
            &pool, "m1", "chat", "xai", "grok-4.3", 3, 100, 50, 20,
        )
        .await
        .unwrap();
        let resp = app
            .oneshot(
                Request::get("/meetings/m1")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        // Pool sums (3 / 100 / 50 / 20), NOT legacy (99/999/…), and
        // NOT pool + legacy combined (102/1099/…).
        assert_eq!(v["llm_usage"]["calls"], 3);
        assert_eq!(v["llm_usage"]["input_tokens"], 100);
        assert_eq!(v["llm_usage"]["output_tokens"], 50);
        assert_eq!(v["llm_usage"]["cached_input_tokens"], 20);
        // Single pool row → provider AND model are uniform → surfaced.
        assert_eq!(v["llm_usage"]["provider"], "xai");
        assert_eq!(v["llm_usage"]["model_id"], "grok-4.3");
        assert_eq!(v["llm_usage_by_pool"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn uniform_value_surfaces_agreement_and_nulls_disagreement() {
        assert_eq!(
            uniform_value(["xai", "xai"].into_iter()),
            Some("xai".to_string())
        );
        assert_eq!(uniform_value(["a", "b"].into_iter()), None);
        assert_eq!(
            uniform_value(["solo"].into_iter()),
            Some("solo".to_string())
        );
        assert_eq!(uniform_value(std::iter::empty::<&str>()), None);
    }

    fn title_of(metadata_json: &str) -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(metadata_json).unwrap();
        v.get("title").and_then(|t| t.as_str()).map(str::to_string)
    }

    #[test]
    fn set_title_on_empty_blob_creates_the_tag() {
        let out = set_title("{}", "Quarterly review").unwrap();
        assert_eq!(title_of(&out).as_deref(), Some("Quarterly review"));
    }

    #[test]
    fn set_title_preserves_other_tags() {
        let out = set_title(r#"{"project":"helix"}"#, "Kickoff").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v.get("project").and_then(|p| p.as_str()), Some("helix"));
        assert_eq!(v.get("title").and_then(|t| t.as_str()), Some("Kickoff"));
    }

    #[test]
    fn set_title_overrides_existing_title() {
        let out = set_title(r#"{"title":"auto title"}"#, "My name").unwrap();
        assert_eq!(title_of(&out).as_deref(), Some("My name"));
    }

    #[test]
    fn set_title_trims_surrounding_whitespace() {
        let out = set_title("{}", "  spaced  ").unwrap();
        assert_eq!(title_of(&out).as_deref(), Some("spaced"));
    }

    #[test]
    fn set_title_rejects_empty_and_whitespace() {
        assert!(set_title("{}", "").is_err());
        assert!(set_title("{}", "   ").is_err());
    }

    #[test]
    fn set_title_rejects_over_length() {
        let long = "a".repeat(MAX_TITLE_LEN + 1);
        assert!(set_title("{}", &long).is_err());
        let ok = "a".repeat(MAX_TITLE_LEN);
        assert!(set_title("{}", &ok).is_ok());
    }

    #[test]
    fn set_title_recovers_from_malformed_blob() {
        let out = set_title("not json", "Recovered").unwrap();
        assert_eq!(title_of(&out).as_deref(), Some("Recovered"));
    }

    // ---- pick_title (PDF export title fallback chain) ----
    //
    // `pick_title` mirrors `pickMeetingTitle` on the clients
    // (mobile: packages/mobile/src/lib/meetings.ts, char-based;
    // mac: SettingsView.swift, grapheme-based). These tests pin the
    // char-based semantics and, critically, that multi-byte UTF-8 in
    // LLM-generated descriptions can never panic the export handler.

    #[test]
    fn pick_title_prefers_metadata_title_tag() {
        let metadata = vec![("title".to_string(), "  Quarterly sync  ".to_string())];
        let out = pick_title(&Some("Some description line".to_string()), &metadata);
        assert_eq!(out, "Quarterly sync");
    }

    #[test]
    fn pick_title_uses_first_nonempty_description_line() {
        let desc = "\n   \nFirst real line\nsecond line".to_string();
        let out = pick_title(&Some(desc), &[]);
        assert_eq!(out, "First real line");
    }

    #[test]
    fn pick_title_returns_exactly_80_char_line_unchanged() {
        let line = "b".repeat(80);
        let out = pick_title(&Some(line.clone()), &[]);
        assert_eq!(out, line);
    }

    #[test]
    fn pick_title_truncates_long_ascii_line_to_79_chars_plus_ellipsis() {
        let line = "a".repeat(81);
        let out = pick_title(&Some(line), &[]);
        assert_eq!(out, format!("{}…", "a".repeat(79)));
        assert_eq!(out.chars().count(), 80);
    }

    #[test]
    fn pick_title_does_not_panic_on_multibyte_char_straddling_byte_79() {
        // 78 ASCII bytes, then a 3-byte em dash occupying bytes 78–80:
        // today `&trimmed[..79]` cuts mid-dash and panics with
        // "byte index 79 is not a char boundary".
        let line = format!("{}—tail and more text", "a".repeat(78));
        let out = pick_title(&Some(line), &[]);
        // 79 chars kept (78 a's + the em dash) + ellipsis.
        assert_eq!(out, format!("{}—…", "a".repeat(78)));
        assert_eq!(out.chars().count(), 80);
    }

    #[test]
    fn pick_title_keeps_short_accented_line_intact() {
        // 60 chars but 120 bytes: today the byte-counted comparison
        // sends this into the slice (panic); a char-counted comparison
        // must return it whole — matching what every client displays.
        let line = "é".repeat(60);
        let out = pick_title(&Some(line.clone()), &[]);
        assert_eq!(out, line);
    }

    #[test]
    fn pick_title_falls_back_to_untitled() {
        assert_eq!(pick_title(&None, &[]), "Untitled meeting");
        assert_eq!(
            pick_title(&Some("   \n  ".to_string()), &[]),
            "Untitled meeting"
        );
        // An empty/whitespace metadata title doesn't count either.
        let metadata = vec![("title".to_string(), "   ".to_string())];
        assert_eq!(pick_title(&None, &metadata), "Untitled meeting");
    }
}
