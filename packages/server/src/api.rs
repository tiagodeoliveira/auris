//! REST API for past meetings.
//!
//! Two endpoints, both auth'd by `Authorization: Bearer <token>`
//! against the same `MEETING_COMPANION_TOKEN` the WS protocol uses:
//!   - `GET /meetings`        → list of meeting summaries (no transcripts)
//!   - `GET /meetings/:id`    → one meeting + its inlined transcript
//!                              (read from the per-meeting jsonl file)
//!
//! Runs on a separate axum listener alongside the WS server (default
//! `ws_port + 1`). Two ports today; consolidating onto a single port
//! is a follow-up that requires migrating the WS layer to axum's
//! `WebSocketUpgrade` extractor.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::SqlitePool;
use tower_http::cors::CorsLayer;

use crate::contract::Item;

#[derive(Clone)]
pub struct ApiState {
    pub db: SqlitePool,
    pub token: Arc<String>,
}

/// Build the axum Router with both endpoints wired up. CORS is
/// permissive so a future PWA history view served from a different
/// origin can fetch without server-side allowlisting.
pub fn make_router(state: ApiState) -> Router {
    Router::new()
        .route("/meetings", get(list_meetings))
        .route("/meetings/:id", get(get_meeting))
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
    }))
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
    Db(sqlx::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            ApiError::Db(e) => {
                tracing::warn!(error = ?e, "db error in api");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
        };
        (status, Json(serde_json::json!({"error": msg}))).into_response()
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
        make_router(ApiState {
            db: pool,
            token: Arc::new(token.to_string()),
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
