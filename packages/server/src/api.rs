//! REST API for browsing meetings + screenshot transport.
//!
//! All endpoints are auth'd by `Authorization: Bearer <token>`
//! against the same `MEETING_COMPANION_TOKEN` the WS protocol uses.
//!
//!   GET    /meetings                                       → summaries (newest first)
//!   GET    /meetings/:id                                   → meeting + transcript + moments
//!   DELETE /meetings/:id                                   → cascade-delete + blob cleanup
//!   GET    /meetings/:id/moments/:moment_id/screenshot     → PNG bytes
//!   POST   /meetings/:id/moments/:moment_id/screenshot     → upload PNG (raw image/png)
//!   DELETE /moments/:moment_id                             → drop one moment
//!
//! Moment *creation* lives on the WS path (`Intent::MarkMoment`); this
//! module only handles read paths plus the screenshot transport (the
//! Mac uploads a PNG here in response to `Event::CaptureMomentScreenshot`).

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;

use crate::contract::Item;

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
}

#[derive(Clone)]
pub struct ApiState {
    pub db: SqlitePool,
    pub token: Arc<String>,
    /// Internal broadcast: each freshly-inserted moment is published
    /// here. The async summary worker subscribes; nothing else does
    /// today. Sender is held in `ServerHandle`; this is the cloned
    /// view used by API handlers.
    pub moment_created_tx: broadcast::Sender<MomentCreated>,
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
    require_bearer(&headers, &state.token)?;
    let rows: Vec<MeetingRow> = sqlx::query_as(
        r#"SELECT id, description, metadata, started_at, ended_at
           FROM meetings
           ORDER BY started_at DESC"#,
    )
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
    require_bearer(&headers, &state.token)?;
    let row: Option<MeetingRow> = sqlx::query_as(
        r#"SELECT id, description, metadata, started_at, ended_at
           FROM meetings WHERE id = ?1"#,
    )
    .bind(&id)
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
    require_bearer(&headers, &state.token)?;
    let removed = crate::db::delete_meeting(&state.db, &meeting_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    if !removed {
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
    require_bearer(&headers, &state.token)?;
    let result = crate::db::delete_moment(&state.db, &moment_id)
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
    require_bearer(&headers, &state.token)?;
    let row: Option<(String, Option<String>)> =
        sqlx::query_as(r#"SELECT meeting_id, asset_path FROM moments WHERE id = ?1"#)
            .bind(&moment_id)
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
    require_bearer(&headers, &state.token)?;
    if bytes.is_empty() {
        return Err(ApiError::BadRequest("empty screenshot body".into()));
    }
    // Confirm the (meeting, moment) pair exists. Without this, a typo
    // would silently write a PNG that no row ever points at.
    let row: Option<(String,)> = sqlx::query_as(r#"SELECT meeting_id FROM moments WHERE id = ?1"#)
        .bind(&moment_id)
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

/// Validate the `Authorization: Bearer <token>` header against the
/// configured server token. Constant-time compare to keep timing
/// attacks off the table.
fn require_bearer(headers: &HeaderMap, token: &str) -> Result<(), ApiError> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;
    let provided = auth.strip_prefix("Bearer ").ok_or(ApiError::Unauthorized)?;
    use subtle::ConstantTimeEq;
    if provided.as_bytes().ct_eq(token.as_bytes()).into() {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
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
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use tower::ServiceExt;

    async fn pool() -> SqlitePool {
        let opts = SqliteConnectOptions::new()
            .filename(":memory:")
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    fn router(pool: SqlitePool, token: &str) -> Router {
        // Tests don't observe the moment-created broadcast — a
        // detached sender keeps the type contract satisfied without
        // a worker subscriber on the other end.
        let (moment_created_tx, _) = broadcast::channel::<MomentCreated>(8);
        make_router(ApiState {
            db: pool,
            token: Arc::new(token.to_string()),
            moment_created_tx,
        })
    }

    async fn body_string(resp: Response) -> String {
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn unauthorized_without_bearer() {
        let pool = pool().await;
        let app = router(pool, "secret");
        let resp = app
            .oneshot(Request::get("/meetings").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn unauthorized_on_wrong_token() {
        let pool = pool().await;
        let app = router(pool, "secret");
        let resp = app
            .oneshot(
                Request::get("/meetings")
                    .header("authorization", "Bearer nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn list_returns_meetings_newest_first() {
        let pool = pool().await;
        let earlier = Utc::now() - chrono::Duration::minutes(60);
        let later = Utc::now();
        crate::db::insert_meeting(&pool, "older", earlier, Some("first"), "{}")
            .await
            .unwrap();
        crate::db::insert_meeting(&pool, "newer", later, Some("second"), "{}")
            .await
            .unwrap();
        let app = router(pool, "secret");
        let resp = app
            .oneshot(
                Request::get("/meetings")
                    .header("authorization", "Bearer secret")
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

    #[tokio::test]
    async fn detail_404_for_missing_meeting() {
        let pool = pool().await;
        let app = router(pool, "secret");
        let resp = app
            .oneshot(
                Request::get("/meetings/no-such-id")
                    .header("authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn detail_returns_meeting_with_empty_transcript() {
        let pool = pool().await;
        crate::db::insert_meeting(&pool, "m1", Utc::now(), Some("hi"), r#"{"a":"b"}"#)
            .await
            .unwrap();
        let app = router(pool, "secret");
        let resp = app
            .oneshot(
                Request::get("/meetings/m1")
                    .header("authorization", "Bearer secret")
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
}
