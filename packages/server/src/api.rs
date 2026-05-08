//! REST API for browsing meetings + artifact subsystem (PLAN.md §3.7).
//!
//! All endpoints are auth'd by `Authorization: Bearer <token>` (Auth0
//! JWT or the dev-bypass synthetic user when `MEETING_COMPANION_AUTH_DISABLED=1`).
//!
//!   GET    /meetings                                       → summaries (newest first)
//!   GET    /meetings/:id                                   → meeting + transcript + moments
//!   DELETE /meetings/:id                                   → cascade-delete + blob cleanup
//!   GET    /meetings/:id/moments/:moment_id/screenshot     → PNG bytes
//!   POST   /meetings/:id/moments/:moment_id/screenshot     → upload PNG (raw image/png)
//!   DELETE /moments/:moment_id                             → drop one moment
//!
//!   GET    /artifacts                                      → user's library (newest first)
//!   POST   /artifacts                                      → multipart upload (`file` field)
//!   GET    /artifacts/:id                                  → one artifact's metadata
//!   DELETE /artifacts/:id                                  → remove from library + blob
//!   POST   /meetings/:id/artifacts                         → attach (body: { artifact_id })
//!   DELETE /meetings/:id/artifacts/:artifact_id            → detach
//!
//! Moment *creation* lives on the WS path (`Intent::MarkMoment`); this
//! module handles read paths, the screenshot transport, and the
//! artifact subsystem in full (no WS path for artifacts — they're a
//! pre-meeting library).

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;

use crate::contract::Item;

/// MIME types we accept for artifact uploads. Matches PLAN.md §3.7
/// "Format support (v1)" — text formats land via rig's `Document`,
/// PDFs via `document_raw`, images via `image_base64`. Anything else
/// is 415.
const ALLOWED_ARTIFACT_MIMES: &[&str] = &[
    "text/plain",
    "text/markdown",
    "text/html",
    "text/csv",
    "application/json",
    "application/pdf",
    "image/png",
    "image/jpeg",
];

/// Cap individual artifact uploads. Personal-use ceiling — most docs
/// fit in MB, not GB; very large PDFs still go through.
const MAX_ARTIFACT_BYTES: usize = 50 * 1024 * 1024;

/// Internal signal published by `POST /meetings/:id/moments` and
/// consumed by the moment-summary worker (`summarizer/moment.rs`).
/// Not on the WS wire today — kept private to the server crate so
/// future moment kinds (interview etc.) can extend it without a
/// client-facing protocol change.
#[derive(Debug, Clone)]
pub struct MomentCreated {
    pub meeting_id: String,
    pub moment_id: String,
    pub kind: String,
    pub t_ms: i64,
    /// Owning user — needed by the summary worker so the LLM-usage
    /// counter increments under the right per-meeting key.
    pub user_id: String,
}

/// Internal signal published by `POST /artifacts` and consumed by
/// the artifact-summary worker (`summarizer/artifact.rs`). Carries
/// everything the worker needs to read the bytes off disk and call
/// the LLM — the DB lookup is avoided so the worker doesn't race
/// against a not-yet-committed transaction.
#[derive(Debug, Clone)]
pub struct ArtifactCreated {
    pub artifact_id: String,
    pub user_id: String,
    pub name: String,
    pub mime_type: String,
    /// Relative path under `<DATA_DIR>/blobs/` — same shape stored
    /// in `artifacts.asset_path`.
    pub asset_path: String,
}

#[derive(Clone)]
pub struct ApiState {
    pub db: PgPool,
    /// Auth mode chosen at boot. Each handler resolves the request's
    /// bearer token to a local `users.id` via this — either a real
    /// JWT validation against Auth0 or the dev-bypass synthetic user.
    pub auth: Arc<crate::ws::AuthMode>,
    /// Internal broadcast: each freshly-inserted moment is published
    /// here. The async summary worker subscribes; nothing else does
    /// today. Sender is held in `ServerHandle`; this is the cloned
    /// view used by API handlers.
    pub moment_created_tx: broadcast::Sender<MomentCreated>,
    /// Mirror of `moment_created_tx` for artifacts. Each upload to
    /// `POST /artifacts` publishes here; the async summary worker
    /// subscribes.
    pub artifact_created_tx: broadcast::Sender<ArtifactCreated>,
    /// Kick the agent loop for a specific user. Sent on artifact
    /// attach so the agent fires immediately and picks up the new
    /// artifact in its next working-context build.
    pub agent_kick_tx: broadcast::Sender<crate::summarizer::agent::AgentKick>,
    /// Broadcast bus for per-user wire events. API handlers that
    /// mutate cross-client visible state (e.g. attach/detach
    /// artifacts) emit `Event::ArtifactsChanged` here so PWA + Mac
    /// stay in sync without polling. Sender is held in
    /// `ServerHandle`; this is the cloned view for API handlers.
    pub events_tx: broadcast::Sender<crate::contract::UserEvent>,
}

/// Build the axum Router with all endpoints wired up. CORS is
/// permissive so a future PWA history view served from a different
/// origin can fetch without server-side allowlisting.
pub fn make_router(state: ApiState) -> Router {
    // Full-display PNG screenshots routinely exceed axum's 2 MiB
    // default; bump to 64 MiB on routes that accept image bytes.
    // Read-only routes keep the default — there's no client request
    // body to enforce against on those.
    const SCREENSHOT_BODY_LIMIT: usize = 64 * 1024 * 1024;

    Router::new()
        .route("/meetings", get(list_meetings))
        .route("/meetings/:id", get(get_meeting).delete(delete_meeting))
        .route(
            "/meetings/:id/moments/:moment_id/screenshot",
            get(get_moment_screenshot).post(upload_moment_screenshot),
        )
        .route("/moments/:moment_id", axum::routing::delete(delete_moment))
        .route("/artifacts", get(list_artifacts).post(upload_artifact))
        .route("/artifacts/:id", get(get_artifact).delete(delete_artifact))
        .route("/meetings/:meeting_id/artifacts", post(attach_artifact))
        .route(
            "/meetings/:meeting_id/artifacts/:artifact_id",
            axum::routing::delete(detach_artifact),
        )
        .layer(DefaultBodyLimit::max(SCREENSHOT_BODY_LIMIT))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

#[derive(Debug, Serialize)]
struct MeetingSummary {
    id: String,
    description: Option<String>,
    metadata: serde_json::Value,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
}

async fn list_meetings(
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

#[derive(Debug, Serialize)]
struct MeetingDetail {
    id: String,
    description: Option<String>,
    metadata: serde_json::Value,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    /// Full transcript, hydrated from the per-meeting jsonl blob.
    /// Empty when the meeting has no committed transcript items
    /// (e.g., a meeting that started but ended before any speech).
    transcript: Vec<Item>,
    /// Moments captured during this meeting, oldest first.
    moments: Vec<MomentDto>,
    /// Library artifacts attached to this meeting, in attach order.
    /// PLAN.md §3.7. Empty when none were attached.
    artifacts: Vec<ArtifactDto>,
}

/// Wire shape for a moment. Mirrors `db::MomentRow` minus internal
/// fields, with `screenshot_url` derived from `asset_path` (clients
/// never see the on-disk path).
#[derive(Debug, Serialize)]
struct MomentDto {
    id: String,
    kind: String,
    t: i64,
    note: Option<String>,
    summary: Option<String>,
    summary_status: String,
    /// `Some` when the moment has a screenshot on disk. Absolute
    /// path on this server (relative to its origin); clients fetch
    /// it directly. `None` when no screenshot was captured.
    screenshot_url: Option<String>,
    created_at: DateTime<Utc>,
}

impl MomentDto {
    fn from_row(row: crate::db::MomentRow, meeting_id: &str) -> Self {
        let screenshot_url = row
            .asset_path
            .as_ref()
            .map(|_| format!("/meetings/{}/moments/{}/screenshot", meeting_id, row.id));
        Self {
            id: row.id,
            kind: row.kind,
            t: row.t,
            note: row.note,
            summary: row.summary,
            summary_status: row.summary_status,
            screenshot_url,
            created_at: row.created_at,
        }
    }
}

async fn get_meeting(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<MeetingDetail>, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    let row: Option<MeetingRow> = sqlx::query_as(
        r#"SELECT id, description, metadata, started_at, ended_at
           FROM meetings WHERE id = $1 AND user_id = $2"#,
    )
    .bind(&id)
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::Db)?;
    let Some(row) = row else {
        return Err(ApiError::NotFound);
    };
    let transcript = read_transcript(&row.id).await;
    let moments = crate::db::list_moments_for_meeting(&state.db, &row.id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?
        .into_iter()
        .map(|r| MomentDto::from_row(r, &row.id))
        .collect();
    let artifacts: Vec<ArtifactDto> = crate::db::list_artifacts_for_meeting(&state.db, &row.id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?
        .into_iter()
        .map(ArtifactDto::from)
        .collect();
    let MeetingSummary {
        id,
        description,
        metadata,
        started_at,
        ended_at,
    } = row.into_summary();
    Ok(Json(MeetingDetail {
        id,
        description,
        metadata,
        started_at,
        ended_at,
        transcript,
        moments,
        artifacts,
    }))
}

/// `DELETE /meetings/:id` — remove a meeting plus its moments
/// (FK cascade) and the on-disk blob directory holding the
/// transcript JSONL and any screenshots. 204 on success, 404 if
/// the id is unknown. Disk cleanup runs after the DB delete; if
/// that step fails we still return 204 — the row is gone, the
/// blobs are orphans that future cleanup can reap.
async fn delete_meeting(
    State(state): State<ApiState>,
    Path(meeting_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    let removed = crate::db::delete_meeting_for_user(&state.db, &meeting_id, &user_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    if !removed {
        // 404 covers both "no such id" and "owned by someone else" —
        // we don't distinguish so we don't leak existence.
        return Err(ApiError::NotFound);
    }
    if let Ok(dir) = crate::db::data_dir() {
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

/// `DELETE /moments/:moment_id` — drop a single moment row. Best-
/// effort screenshot cleanup runs after the DB delete; if it fails
/// we still return 204 (the row is gone, the file is an orphan).
async fn delete_moment(
    State(state): State<ApiState>,
    Path(moment_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    let result = crate::db::delete_moment_for_user(&state.db, &moment_id, &user_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    let Some(asset_path) = result else {
        return Err(ApiError::NotFound);
    };
    if let Some(rel) = asset_path {
        if let Ok(dir) = crate::db::data_dir() {
            let abs = dir.join(&rel);
            if abs.exists() {
                if let Err(e) = tokio::fs::remove_file(&abs).await {
                    tracing::warn!(
                        error = ?e, moment_id = %moment_id, path = %abs.display(),
                        "delete_moment: screenshot cleanup failed (row already removed)"
                    );
                }
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /meetings/:id/moments/:moment_id/screenshot` — serves the
/// PNG bytes from disk. 404 when the row has no `asset_path` or
/// the file is missing (would happen if `<DATA_DIR>` was wiped).
async fn get_moment_screenshot(
    State(state): State<ApiState>,
    Path((meeting_id, moment_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    // Join to meetings so we only return screenshots the caller owns.
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        r#"SELECT m.meeting_id, m.asset_path
             FROM moments m
             JOIN meetings me ON me.id = m.meeting_id
            WHERE m.id = $1 AND me.user_id = $2"#,
    )
    .bind(&moment_id)
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::Db)?;
    let Some((row_meeting, Some(asset))) = row else {
        return Err(ApiError::NotFound);
    };
    if row_meeting != meeting_id {
        // Path-mismatch: the moment exists but under a different
        // meeting. Treat as not-found rather than leaking shape.
        return Err(ApiError::NotFound);
    }
    let dir = crate::db::data_dir().map_err(|e| ApiError::Internal(e.to_string()))?;
    let abs = dir.join(&asset);
    let bytes = match tokio::fs::read(&abs).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ApiError::NotFound),
        Err(e) => return Err(ApiError::Internal(format!("read screenshot: {e}"))),
    };
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/png")
        .header(header::CACHE_CONTROL, "private, max-age=86400")
        .body(Body::from(bytes))
        .unwrap())
}

/// `POST /meetings/:id/moments/:moment_id/screenshot` — late-binding
/// screenshot upload for moments created via the WS `mark_moment`
/// intent. The Mac with `screen_capture` capability that's bound as
/// the audio source receives `Event::CaptureMomentScreenshot` and
/// posts the resulting PNG here. Body is raw `image/png` (the same
/// shape as a multipart `screenshot` field, but without the multipart
/// envelope — keeps the upload small and the client-side encode trivial).
async fn upload_moment_screenshot(
    State(state): State<ApiState>,
    Path((meeting_id, moment_id)): Path<(String, String)>,
    headers: HeaderMap,
    bytes: axum::body::Bytes,
) -> Result<StatusCode, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    if bytes.is_empty() {
        return Err(ApiError::BadRequest("empty screenshot body".into()));
    }
    // Confirm the (meeting, moment) pair exists *and* belongs to
    // this user — otherwise reject with 404 (don't leak ownership).
    let row: Option<(String,)> = sqlx::query_as(
        r#"SELECT m.meeting_id
             FROM moments m
             JOIN meetings me ON me.id = m.meeting_id
            WHERE m.id = $1 AND me.user_id = $2"#,
    )
    .bind(&moment_id)
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::Db)?;
    let Some((row_meeting,)) = row else {
        return Err(ApiError::NotFound);
    };
    if row_meeting != meeting_id {
        return Err(ApiError::NotFound);
    }

    let rel = format!("blobs/meetings/{meeting_id}/screenshots/{moment_id}.png");
    let dir = crate::db::data_dir().map_err(|e| ApiError::Internal(e.to_string()))?;
    let abs = dir.join(&rel);
    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::Internal(format!("mkdir: {e}")))?;
    }
    tokio::fs::write(&abs, &bytes)
        .await
        .map_err(|e| ApiError::Internal(format!("write screenshot: {e}")))?;

    crate::db::update_moment_asset_path(&state.db, &moment_id, &rel)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Cast an `anyhow::Error` back into a `sqlx::Error` for our
/// `ApiError::Db` variant. The db helpers wrap `sqlx::Error` in
/// `anyhow::Error` for context; this unwraps it for the API layer.
fn downcast_db(e: anyhow::Error) -> sqlx::Error {
    e.downcast::<sqlx::Error>()
        .unwrap_or_else(|orig| sqlx::Error::Protocol(format!("non-sqlx db error: {orig}")))
}

// ─── Artifacts (PLAN.md §3.7) ────────────────────────────────────────

/// Wire shape for an artifact row. Hides `asset_path` (clients don't
/// need it — they reference artifacts by `id`) but exposes both
/// summary fields so the meeting compose UI can render the short
/// summary as a chip preview.
#[derive(Debug, Serialize)]
struct ArtifactDto {
    id: String,
    name: String,
    mime_type: String,
    short_summary: Option<String>,
    long_summary: Option<String>,
    summary_status: String,
    size_bytes: i64,
    created_at: DateTime<Utc>,
}

impl From<crate::db::ArtifactRow> for ArtifactDto {
    fn from(row: crate::db::ArtifactRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            mime_type: row.mime_type,
            short_summary: row.short_summary,
            long_summary: row.long_summary,
            summary_status: row.summary_status,
            size_bytes: row.size_bytes,
            created_at: row.created_at,
        }
    }
}

async fn list_artifacts(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ArtifactDto>>, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    let rows = crate::db::list_artifacts_for_user(&state.db, &user_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    Ok(Json(rows.into_iter().map(ArtifactDto::from).collect()))
}

async fn get_artifact(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ArtifactDto>, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    let row = crate::db::get_artifact_for_user(&state.db, &id, &user_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(ArtifactDto::from(row)))
}

/// Multipart upload. Expects exactly one field named `file` carrying
/// the bytes; `filename` becomes the artifact's display name and
/// `Content-Type` its mime. Returns the freshly-inserted row with
/// `summary_status: pending` — the async summarizer worker (PLAN.md
/// §3.12 step 3d) populates the summaries.
///
/// On any failure after the blob is on disk, we leave it: a future
/// cleanup task can scan for orphan blobs (rows with `summary_status
/// = 'failed'` that never got a paired DB row). For personal-use v1,
/// this is rare enough that a manual rm is fine.
async fn upload_artifact(
    State(state): State<ApiState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<ArtifactDto>, ApiError> {
    let user_id = require_user(&headers, &state).await?;

    let mut file_bytes: Option<axum::body::Bytes> = None;
    let mut file_name: Option<String> = None;
    let mut mime_type: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("multipart: {e}")))?
    {
        let field_name = field.name().unwrap_or("").to_string();
        if field_name != "file" {
            // Ignore unknown fields — don't fail the upload over a
            // stray client-side metadata field.
            continue;
        }
        file_name = field.file_name().map(|s| s.to_string());
        mime_type = field
            .content_type()
            .map(|s| s.to_string())
            .or_else(|| Some("application/octet-stream".to_string()));
        let bytes = field
            .bytes()
            .await
            .map_err(|e| ApiError::BadRequest(format!("read bytes: {e}")))?;
        file_bytes = Some(bytes);
    }

    let bytes = file_bytes.ok_or_else(|| ApiError::BadRequest("missing file field".into()))?;
    let name = file_name.ok_or_else(|| ApiError::BadRequest("missing filename".into()))?;
    let mime = mime_type.ok_or_else(|| ApiError::BadRequest("missing content type".into()))?;
    if bytes.is_empty() {
        return Err(ApiError::BadRequest("empty file".into()));
    }
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(ApiError::BadRequest(format!(
            "file exceeds {} bytes",
            MAX_ARTIFACT_BYTES
        )));
    }
    if !ALLOWED_ARTIFACT_MIMES.iter().any(|m| *m == mime) {
        return Err(ApiError::BadRequest(format!(
            "unsupported mime type: {mime}"
        )));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let rel = format!("blobs/artifacts/{user_id}/{id}");
    let dir = crate::db::data_dir().map_err(|e| ApiError::Internal(e.to_string()))?;
    let abs = dir.join(&rel);
    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::Internal(format!("mkdir: {e}")))?;
    }
    tokio::fs::write(&abs, &bytes)
        .await
        .map_err(|e| ApiError::Internal(format!("write artifact: {e}")))?;

    let size = bytes.len() as i64;
    crate::db::insert_artifact(&state.db, &id, &user_id, &name, &mime, &rel, size)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;

    let row = crate::db::get_artifact_for_user(&state.db, &id, &user_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?
        .ok_or_else(|| ApiError::Internal("artifact vanished after insert".into()))?;

    // Wake the summary worker. `summary_status` stays `pending`
    // until the worker writes back via `update_artifact_summaries`.
    // Send is `let _ =` because a closed channel (e.g. tests with no
    // worker spawned) shouldn't fail the upload.
    let _ = state.artifact_created_tx.send(ArtifactCreated {
        artifact_id: row.id.clone(),
        user_id: user_id.clone(),
        name: row.name.clone(),
        mime_type: row.mime_type.clone(),
        asset_path: row.asset_path.clone(),
    });

    Ok(Json(ArtifactDto::from(row)))
}

async fn delete_artifact(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    // Read the row first so we know the on-disk path; the DB cascade
    // drops join rows automatically, but we own the blob lifecycle.
    let row = crate::db::get_artifact_for_user(&state.db, &id, &user_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?
        .ok_or(ApiError::NotFound)?;
    crate::db::delete_artifact_for_user(&state.db, &id, &user_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    let dir = crate::db::data_dir().map_err(|e| ApiError::Internal(e.to_string()))?;
    let abs = dir.join(&row.asset_path);
    if let Err(e) = tokio::fs::remove_file(&abs).await {
        // Best-effort: blob may already be gone (manual cleanup, OS
        // restart wiped /tmp, etc.). DB row is the source of truth.
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(error = ?e, path = %abs.display(), "artifact blob delete failed");
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct AttachArtifactBody {
    artifact_id: String,
}

async fn attach_artifact(
    State(state): State<ApiState>,
    Path(meeting_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AttachArtifactBody>,
) -> Result<StatusCode, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    // Belt-and-suspenders ownership: both the meeting and the
    // artifact must belong to this caller. 404 (not 403) on miss so
    // we don't leak existence.
    let meeting_owned: Option<(String,)> =
        sqlx::query_as(r#"SELECT id FROM meetings WHERE id = $1 AND user_id = $2"#)
            .bind(&meeting_id)
            .bind(&user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::Db)?;
    if meeting_owned.is_none() {
        return Err(ApiError::NotFound);
    }
    let artifact = crate::db::get_artifact_for_user(&state.db, &body.artifact_id, &user_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?
        .ok_or(ApiError::NotFound)?;
    // PLAN.md §3.7 — only `done` artifacts are attachable. Pending
    // ones aren't fully indexed yet; failed ones never will be.
    if artifact.summary_status != "done" {
        return Err(ApiError::BadRequest(format!(
            "artifact summary_status is {}; only `done` artifacts can be attached",
            artifact.summary_status
        )));
    }
    crate::db::attach_artifact_to_meeting(&state.db, &meeting_id, &body.artifact_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    // Kick the agent so it sees the new artifact on its next fire
    // immediately, rather than waiting for the next transcript
    // trigger. Closed-channel send is silent (test routers without
    // a worker still serve attaches).
    let _ = state
        .agent_kick_tx
        .send(crate::summarizer::agent::AgentKick {
            user_id: user_id.clone(),
            reason: crate::summarizer::agent::AgentKickReason::ArtifactAttached {
                artifact_id: body.artifact_id.clone(),
            },
        });
    broadcast_artifacts_changed(&state, &meeting_id, &user_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Read the current attached-artifact set from DB and broadcast it
/// to all WS clients of `user_id` as `Event::ArtifactsChanged`.
/// Called from attach + detach handlers so Mac and PWA stay in
/// sync without either polling or trusting their own local mirror.
/// DB miss is logged and dropped — the broadcast was best-effort
/// anyway, and the next snapshot path will eventually carry the
/// truth.
async fn broadcast_artifacts_changed(state: &ApiState, meeting_id: &str, user_id: &str) {
    let attached = match crate::db::list_artifacts_for_meeting(&state.db, meeting_id).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = ?e, meeting_id, "broadcast_artifacts_changed: list failed");
            return;
        }
    };
    let artifact_ids: Vec<String> = attached.into_iter().map(|a| a.id).collect();
    let _ = state.events_tx.send(crate::contract::UserEvent::new(
        user_id.to_string(),
        crate::contract::Event::ArtifactsChanged { artifact_ids },
    ));
}

async fn detach_artifact(
    State(state): State<ApiState>,
    Path((meeting_id, artifact_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    // Ownership check on the meeting side is sufficient — the join
    // row only exists if both pieces were owned at attach time, and
    // the artifact_id path param can't bypass that.
    let meeting_owned: Option<(String,)> =
        sqlx::query_as(r#"SELECT id FROM meetings WHERE id = $1 AND user_id = $2"#)
            .bind(&meeting_id)
            .bind(&user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::Db)?;
    if meeting_owned.is_none() {
        return Err(ApiError::NotFound);
    }
    crate::db::detach_artifact_from_meeting(&state.db, &meeting_id, &artifact_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    broadcast_artifacts_changed(&state, &meeting_id, &user_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Best-effort read of the per-meeting transcription jsonl. Lines
/// that fail to parse are skipped silently — better to surface a
/// partial transcript than to fail the whole meeting view because
/// of a single malformed row.
async fn read_transcript(meeting_id: &str) -> Vec<Item> {
    let path = match crate::persistence::transcription_path(meeting_id) {
        Ok(p) => p,
        Err(_) => return vec![],
    };
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<Item>(line).ok())
        .collect()
}

/// Row shape that maps 1-to-1 with the SELECT in both handlers.
/// Extracting the conversion keeps the handlers focused on routing.
#[derive(Debug, sqlx::FromRow)]
struct MeetingRow {
    id: String,
    description: Option<String>,
    metadata: String,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
}

impl MeetingRow {
    fn into_summary(self) -> MeetingSummary {
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

/// Validate the `Authorization: Bearer <token>` header against
/// Auth0 (or short-circuit through the dev bypass) and return the
/// caller's local `users.id`. Every authenticated handler calls
/// this as its first step.
async fn require_user(headers: &HeaderMap, state: &ApiState) -> Result<String, ApiError> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));
    crate::auth::resolve_user_id(&state.auth, &state.db, auth_header)
        .await
        .map_err(|e| {
            tracing::debug!(error = %e, "REST auth failed");
            ApiError::Unauthorized
        })
}

#[derive(Debug)]
enum ApiError {
    Unauthorized,
    NotFound,
    BadRequest(String),
    Internal(String),
    Db(sqlx::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg, detail) = match self {
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized", None),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not_found", None),
            ApiError::BadRequest(d) => {
                tracing::warn!(detail = %d, "bad request in api");
                (StatusCode::BAD_REQUEST, "bad_request", Some(d))
            }
            ApiError::Internal(d) => {
                tracing::warn!(detail = %d, "internal error in api");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", None)
            }
            ApiError::Db(e) => {
                tracing::warn!(error = ?e, "db error in api");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", None)
            }
        };
        let body = match detail {
            Some(d) => Json(serde_json::json!({"error": msg, "detail": d})),
            None => Json(serde_json::json!({"error": msg})),
        };
        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// Build a router in `AuthMode::Disabled` (every request maps
    /// to the synthetic `dev|local` user). Returns the router *and*
    /// the local `users.id` so test fixtures can insert meetings
    /// under the right owner.
    async fn router_with_dev_user(pool: PgPool) -> (Router, String) {
        let (moment_created_tx, _) = broadcast::channel::<MomentCreated>(8);
        let (artifact_created_tx, _) = broadcast::channel::<ArtifactCreated>(8);
        let (agent_kick_tx, _) = broadcast::channel::<crate::summarizer::agent::AgentKick>(8);
        let (events_tx, _) = broadcast::channel::<crate::contract::UserEvent>(8);
        let dev_user = crate::db::upsert_user_by_auth0_sub(
            &pool,
            crate::ws::DEV_AUTH0_SUB,
            Some("dev@local"),
            Some("Local Dev"),
        )
        .await
        .unwrap();
        let router = make_router(ApiState {
            db: pool,
            auth: Arc::new(crate::ws::AuthMode::Disabled),
            moment_created_tx,
            artifact_created_tx,
            agent_kick_tx,
            events_tx,
        });
        (router, dev_user.id)
    }

    async fn body_string(resp: Response) -> String {
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
        crate::db::insert_meeting(&pool, "older", &uid, earlier, Some("first"), "{}")
            .await
            .unwrap();
        crate::db::insert_meeting(&pool, "newer", &uid, later, Some("second"), "{}")
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
        crate::db::insert_meeting(&pool, "m1", &uid, Utc::now(), Some("hi"), r#"{"a":"b"}"#)
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
    }

    #[sqlx::test]
    async fn detail_404_when_meeting_belongs_to_other_user(pool: PgPool) {
        // Insert a meeting under a *different* user.
        let other = crate::db::upsert_user_by_auth0_sub(&pool, "other|user", None, None)
            .await
            .unwrap();
        crate::db::insert_meeting(&pool, "other-m", &other.id, Utc::now(), None, "{}")
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

    // ─── Artifact endpoints (PLAN.md §3.7) ──────────────────────────────

    /// Build a minimal `multipart/form-data` body with a single `file`
    /// field. Boundary chosen to be unambiguous against the payload.
    fn multipart_body(filename: &str, mime: &str, content: &[u8]) -> (String, Vec<u8>) {
        let boundary = "----meetingcompaniontest";
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(format!("Content-Type: {mime}\r\n\r\n").as_bytes());
        body.extend_from_slice(content);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        (format!("multipart/form-data; boundary={boundary}"), body)
    }

    /// Point `MEETING_COMPANION_DATA_DIR` at a unique per-test path so
    /// blob writes don't leak between concurrent runs. Avoids a
    /// `tempfile` dep — the dir lives under `/tmp` and gets recycled
    /// by the OS. Tests don't bother cleaning it up; the volume is
    /// small (a handful of bytes per artifact).
    fn scoped_data_dir() {
        let path =
            std::env::temp_dir().join(format!("meeting-companion-test-{}", uuid::Uuid::new_v4()));
        std::env::set_var("MEETING_COMPANION_DATA_DIR", &path);
    }

    #[sqlx::test]
    async fn upload_artifact_creates_pending_row(pool: PgPool) {
        scoped_data_dir();
        let (app, _uid) = router_with_dev_user(pool).await;
        let (ct, body) = multipart_body("agenda.md", "text/markdown", b"# Agenda\n- A\n- B\n");
        let resp = app
            .oneshot(
                Request::post("/artifacts")
                    .header("authorization", "Bearer dev")
                    .header("content-type", ct)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(v["name"], "agenda.md");
        assert_eq!(v["mime_type"], "text/markdown");
        assert_eq!(v["summary_status"], "pending");
        assert!(v["short_summary"].is_null());
    }

    #[sqlx::test]
    async fn upload_artifact_rejects_unsupported_mime(pool: PgPool) {
        scoped_data_dir();
        let (app, _uid) = router_with_dev_user(pool).await;
        let (ct, body) = multipart_body("evil.exe", "application/x-msdownload", b"MZ");
        let resp = app
            .oneshot(
                Request::post("/artifacts")
                    .header("authorization", "Bearer dev")
                    .header("content-type", ct)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[sqlx::test]
    async fn list_artifacts_returns_users_library(pool: PgPool) {
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::db::insert_artifact(&pool, "a1", &uid, "x.md", "text/markdown", "p1", 10)
            .await
            .unwrap();
        crate::db::insert_artifact(&pool, "a2", &uid, "y.md", "text/markdown", "p2", 20)
            .await
            .unwrap();
        let resp = app
            .oneshot(
                Request::get("/artifacts")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: Vec<serde_json::Value> = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(v.len(), 2);
    }

    #[sqlx::test]
    async fn get_artifact_404_for_other_users_artifact(pool: PgPool) {
        let other = crate::db::upsert_user_by_auth0_sub(&pool, "other|user", None, None)
            .await
            .unwrap();
        crate::db::insert_artifact(&pool, "a1", &other.id, "x.md", "text/markdown", "p", 1)
            .await
            .unwrap();
        let (app, _uid) = router_with_dev_user(pool).await;
        let resp = app
            .oneshot(
                Request::get("/artifacts/a1")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn delete_artifact_removes_row(pool: PgPool) {
        scoped_data_dir();
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::db::insert_artifact(&pool, "a1", &uid, "x.md", "text/markdown", "p", 1)
            .await
            .unwrap();
        let resp = app
            .clone()
            .oneshot(
                Request::delete("/artifacts/a1")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        // Row gone.
        let resp2 = app
            .oneshot(
                Request::get("/artifacts/a1")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn attach_artifact_rejects_pending_status(pool: PgPool) {
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::db::insert_meeting(&pool, "m1", &uid, Utc::now(), None, "{}")
            .await
            .unwrap();
        crate::db::insert_artifact(&pool, "a1", &uid, "x.md", "text/markdown", "p", 1)
            .await
            .unwrap();
        // Status is `pending` by default — attach should reject.
        let resp = app
            .oneshot(
                Request::post("/meetings/m1/artifacts")
                    .header("authorization", "Bearer dev")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"artifact_id":"a1"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[sqlx::test]
    async fn attach_then_detach_round_trips(pool: PgPool) {
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::db::insert_meeting(&pool, "m1", &uid, Utc::now(), None, "{}")
            .await
            .unwrap();
        crate::db::insert_artifact(&pool, "a1", &uid, "x.md", "text/markdown", "p", 1)
            .await
            .unwrap();
        // Mark as done so attach is allowed.
        crate::db::update_artifact_summaries(&pool, "a1", "short", "long", "done")
            .await
            .unwrap();
        // Attach.
        let resp = app
            .clone()
            .oneshot(
                Request::post("/meetings/m1/artifacts")
                    .header("authorization", "Bearer dev")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"artifact_id":"a1"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let attached = crate::db::list_artifacts_for_meeting(&pool, "m1")
            .await
            .unwrap();
        assert_eq!(attached.len(), 1);
        // Detach.
        let resp2 = app
            .oneshot(
                Request::delete("/meetings/m1/artifacts/a1")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::NO_CONTENT);
        let attached2 = crate::db::list_artifacts_for_meeting(&pool, "m1")
            .await
            .unwrap();
        assert!(attached2.is_empty());
    }

    #[sqlx::test]
    async fn attach_404s_when_meeting_belongs_to_other_user(pool: PgPool) {
        let other = crate::db::upsert_user_by_auth0_sub(&pool, "other|user", None, None)
            .await
            .unwrap();
        crate::db::insert_meeting(&pool, "other-m", &other.id, Utc::now(), None, "{}")
            .await
            .unwrap();
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::db::insert_artifact(&pool, "a1", &uid, "x.md", "text/markdown", "p", 1)
            .await
            .unwrap();
        crate::db::update_artifact_summaries(&pool, "a1", "s", "l", "done")
            .await
            .unwrap();
        let resp = app
            .oneshot(
                Request::post("/meetings/other-m/artifacts")
                    .header("authorization", "Bearer dev")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"artifact_id":"a1"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
