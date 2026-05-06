//! REST API for browsing meetings + capturing moments.
//!
//! All endpoints are auth'd by `Authorization: Bearer <token>`
//! against the same `MEETING_COMPANION_TOKEN` the WS protocol uses.
//!
//!   GET  /meetings                                 → summaries (newest first)
//!   GET  /meetings/:id                             → meeting + transcript + moments
//!   GET  /meetings/:id/moments                     → just the moments list
//!   POST /meetings/:id/moments                     → multipart (t, note?, screenshot?)
//!   GET  /meetings/:id/moments/:moment_id/screenshot → PNG bytes
//!
//! Multipart form fields on POST: `t` (string-encoded i64 ms),
//! `note` (optional text), `screenshot` (optional PNG file). The
//! moment id is server-minted and returned in the JSON response.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Multipart, Path, State},
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
    Router::new()
        .route("/meetings", get(list_meetings))
        .route("/meetings/:id", get(get_meeting))
        .route(
            "/meetings/:id/moments",
            get(list_moments).post(create_moment),
        )
        .route(
            "/meetings/:id/moments/:moment_id/screenshot",
            get(get_moment_screenshot),
        )
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

/// `GET /meetings/:id/moments` — same data as `MeetingDetail.moments`
/// but without the transcript payload. Useful when a client just
/// wants to refresh the moments list without the heavier detail fetch.
async fn list_moments(
    State(state): State<ApiState>,
    Path(meeting_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<MomentDto>>, ApiError> {
    require_bearer(&headers, &state.token)?;
    let rows = crate::db::list_moments_for_meeting(&state.db, &meeting_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    let dtos = rows
        .into_iter()
        .map(|r| MomentDto::from_row(r, &meeting_id))
        .collect();
    Ok(Json(dtos))
}

/// `POST /meetings/:id/moments` — multipart form. Required field
/// `t` is the millisecond offset from meeting start. Optional fields
/// `note` (text) and `screenshot` (PNG bytes). Server mints the
/// moment id, persists the row, writes the screenshot to disk under
/// `<DATA_DIR>/blobs/meetings/<meeting_id>/screenshots/<moment_id>.png`,
/// and publishes a `MomentCreated` event so the async summary worker
/// picks it up. Returns the new moment as JSON.
async fn create_moment(
    State(state): State<ApiState>,
    Path(meeting_id): Path<String>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<MomentDto>, ApiError> {
    require_bearer(&headers, &state.token)?;

    // Confirm the meeting exists before doing any disk I/O. Avoids
    // orphan screenshot files if the client typo'd a meeting id.
    let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM meetings WHERE id = ?1")
        .bind(&meeting_id)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::Db)?;
    if exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Parse multipart fields.
    let mut t_ms: Option<i64> = None;
    let mut note: Option<String> = None;
    let mut screenshot_bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("multipart parse failed: {e}")))?
    {
        match field.name().unwrap_or_default() {
            "t" => {
                let s = field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("read 't' field: {e}")))?;
                t_ms = Some(
                    s.parse::<i64>()
                        .map_err(|_| ApiError::BadRequest(format!("'t' is not an i64: {s:?}")))?,
                );
            }
            "note" => {
                let s = field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("read 'note' field: {e}")))?;
                if !s.is_empty() {
                    note = Some(s);
                }
            }
            "screenshot" => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("read screenshot bytes: {e}")))?;
                if !bytes.is_empty() {
                    screenshot_bytes = Some(bytes.to_vec());
                }
            }
            _ => {} // ignore unknown fields — forward-compat
        }
    }
    let Some(t_ms) = t_ms else {
        return Err(ApiError::BadRequest("missing 't' field".into()));
    };

    // Mint id first so the screenshot path is deterministic before
    // the row exists. Worst case: orphan PNG on disk if the DB write
    // fails. Acceptable; future cleanup task can reap.
    let moment_id = uuid::Uuid::new_v4().to_string();
    let kind = "manual";
    let asset_path = if let Some(bytes) = screenshot_bytes.as_ref() {
        let rel = format!("blobs/meetings/{meeting_id}/screenshots/{moment_id}.png");
        let dir = crate::db::data_dir().map_err(|e| ApiError::Internal(e.to_string()))?;
        let abs = dir.join(&rel);
        if let Some(parent) = abs.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ApiError::Internal(format!("mkdir: {e}")))?;
        }
        tokio::fs::write(&abs, bytes)
            .await
            .map_err(|e| ApiError::Internal(format!("write screenshot: {e}")))?;
        Some(rel)
    } else {
        None
    };

    crate::db::insert_moment(
        &state.db,
        &moment_id,
        &meeting_id,
        kind,
        t_ms,
        note.as_deref(),
        asset_path.as_deref(),
    )
    .await
    .map_err(|e| ApiError::Db(downcast_db(e)))?;

    // Publish to the internal channel so the summary worker picks
    // it up. Receiver count of zero (worker not running) is fine —
    // the row is persisted; a future restart-with-retry mechanism
    // could pick up `pending` rows. Out of scope for now.
    let _ = state.moment_created_tx.send(MomentCreated {
        meeting_id: meeting_id.clone(),
        moment_id: moment_id.clone(),
        kind: kind.to_string(),
        t_ms,
    });

    // Re-read the row so the response reflects whatever defaults
    // the DB applied (created_at, summary_status='pending').
    let rows = crate::db::list_moments_for_meeting(&state.db, &meeting_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    let row = rows
        .into_iter()
        .find(|r| r.id == moment_id)
        .ok_or_else(|| ApiError::Internal("moment vanished after insert".into()))?;
    Ok(Json(MomentDto::from_row(row, &meeting_id)))
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
            ApiError::BadRequest(d) => (StatusCode::BAD_REQUEST, "bad_request", Some(d)),
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
