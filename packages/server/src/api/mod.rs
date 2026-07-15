//! REST API for browsing meetings + artifact subsystem (PLAN.md §3.7).
//!
//! All endpoints are auth'd by `Authorization: Bearer <token>` (Auth0
//! JWT or the dev-bypass synthetic user when `AURIS_AUTH_DISABLED=1`).
//!
//!   GET    /meetings                                       → summaries (newest first)
//!   GET    /meetings/:id                                   → meeting + transcript + moments
//!   DELETE /meetings/:id                                   → cascade-delete + blob cleanup
//!   POST   /meetings/:id/retry-wrap-up                     → re-run wrap-up extraction (failed meetings)
//!   GET    /meetings/:id/moments/:moment_id/screenshot     → PNG bytes
//!   POST   /meetings/:id/moments/:moment_id/screenshot     → upload PNG (raw image/png)
//!   POST   /meetings/:id/chat_attachments                  → upload PNG/JPEG to stage for next chat
//!   DELETE /moments/:moment_id                             → drop one moment
//!
//!   GET    /artifacts                                      → user's library (newest first)
//!   POST   /artifacts                                      → multipart upload (`file` field)
//!   GET    /artifacts/:id                                  → one artifact's metadata
//!   DELETE /artifacts/:id                                  → remove from library + blob
//!   GET    /meetings/:id/artifacts                         → list attached artifacts (attach order)
//!   POST   /meetings/:id/artifacts                         → attach (body: { artifact_id })
//!   DELETE /meetings/:id/artifacts/:artifact_id            → detach
//!
//! Moment *creation* lives on the WS path (`Intent::MarkMoment`); this
//! module handles read paths, the screenshot transport, and the
//! artifact subsystem in full (no WS path for artifacts — they're a
//! pre-meeting library).

pub mod artifacts;
pub mod chat_attachments;
pub mod meetings;
pub mod moments;
pub mod pair;

use std::sync::Arc;

use axum::{
    extract::DefaultBodyLimit,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use sqlx::PgPool;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;

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

/// Internal signal published by `POST /meetings/:id/retry-wrap-up`
/// and consumed by the wrap-up retry worker
/// (`workers::wrap_up::spawn_retry_worker`). The handler has already
/// flipped `wrap_up_status` to `running`; the worker reads the
/// persisted transcript blob and re-runs the extractor. Only the two
/// ids are needed — the transcript lives on disk, keyed by
/// `meeting_id`.
#[derive(Debug, Clone)]
pub struct WrapUpRetry {
    pub user_id: String,
    pub meeting_id: String,
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
    pub auth: Arc<crate::auth::AuthMode>,
    /// Internal broadcast: each freshly-inserted moment is published
    /// here. The async summary worker subscribes; nothing else does
    /// today. Sender is held in `ServerHandle`; this is the cloned
    /// view used by API handlers.
    pub moment_created_tx: broadcast::Sender<MomentCreated>,
    /// Mirror of `moment_created_tx` for artifacts. Each upload to
    /// `POST /artifacts` publishes here; the async summary worker
    /// subscribes.
    pub artifact_created_tx: broadcast::Sender<ArtifactCreated>,
    /// Mirror channel for the wrap-up retry path
    /// (`POST /meetings/:id/retry-wrap-up`). The wrap-up retry worker
    /// subscribes and re-runs the extractor off the persisted
    /// transcript.
    pub wrap_up_retry_tx: broadcast::Sender<WrapUpRetry>,
    /// Kick the agent loop for a specific user. Sent on artifact
    /// attach so the agent fires immediately and picks up the new
    /// artifact in its next working-context build.
    pub agent_kick_tx: broadcast::Sender<crate::agent::AgentKick>,
    /// Two-lane event bus (see `ws::EventBus`). API handlers that
    /// mutate cross-client-visible state (attach/detach artifacts,
    /// pair/revoke devices) emit here so PWA + Mac stay in sync
    /// without polling.
    pub bus: crate::context::EventBus,
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
        .route("/meetings", get(meetings::list_meetings))
        .route(
            "/meetings/:id",
            get(meetings::get_meeting)
                .delete(meetings::delete_meeting)
                .patch(meetings::rename_meeting),
        )
        .route(
            "/meetings/:id/export.pdf",
            get(meetings::export_meeting_pdf),
        )
        .route("/meetings/:id/retry-wrap-up", post(meetings::retry_wrap_up))
        .route(
            "/meetings/:id/moments/:moment_id/screenshot",
            get(moments::get_moment_screenshot).post(moments::upload_moment_screenshot),
        )
        .route(
            "/meetings/:id/chat_attachments",
            post(chat_attachments::upload_chat_attachment),
        )
        .route(
            "/moments/:moment_id",
            axum::routing::delete(moments::delete_moment),
        )
        .route(
            "/artifacts",
            get(artifacts::list_artifacts).post(artifacts::upload_artifact),
        )
        .route(
            "/artifacts/:id",
            get(artifacts::get_artifact).delete(artifacts::delete_artifact),
        )
        .route(
            "/artifacts/:id/retry-summary",
            post(artifacts::retry_artifact_summary),
        )
        .route(
            "/meetings/:meeting_id/artifacts",
            get(artifacts::list_meeting_artifacts).post(artifacts::attach_artifact),
        )
        .route(
            "/meetings/:meeting_id/artifacts/:artifact_id",
            axum::routing::delete(artifacts::detach_artifact),
        )
        .route(
            "/meetings/:meeting_id/attached_meetings",
            post(meetings::attach_meeting),
        )
        .route(
            "/meetings/:meeting_id/attached_meetings/:attached_meeting_id",
            axum::routing::delete(meetings::detach_meeting),
        )
        // Device-pairing flow for the EvenHub PWA. Mint happens over
        // WS (`Intent::MintPairCode`). /pair/devices + /pair/revoke
        // require an authed caller (mobile / Mac); /pair/redeem +
        // /pair/refresh are public so the unpaired PWA can call them.
        .route("/pair/redeem", post(pair::pair_redeem))
        .route("/pair/refresh", post(pair::pair_refresh))
        .route("/pair/devices", get(pair::pair_list_devices))
        .route("/pair/revoke", post(pair::pair_revoke))
        .layer(DefaultBodyLimit::max(SCREENSHOT_BODY_LIMIT))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Extract the Auris JWT issuer from the auth mode. Returns 503 in
/// Disabled mode — there's no signing key, so we can't mint tokens.
pub(crate) fn require_auris_issuer(
    state: &ApiState,
) -> Result<&crate::auth::pairing::AurisJwtIssuer, ApiError> {
    match state.auth.as_ref() {
        crate::auth::AuthMode::Live { auris, .. } => Ok(auris),
        crate::auth::AuthMode::Disabled => Err(ApiError::Internal(
            "pair flow disabled in AURIS_AUTH_DISABLED mode".to_string(),
        )),
    }
}

/// Validate the `Authorization: Bearer <token>` header against
/// Auth0 (or short-circuit through the dev bypass) and return the
/// caller's local `users.id`. Every authenticated handler calls
/// this as its first step.
pub(crate) async fn require_user(
    headers: &HeaderMap,
    state: &ApiState,
) -> Result<String, ApiError> {
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
pub(crate) enum ApiError {
    Unauthorized,
    NotFound,
    BadRequest(String),
    /// The caller exceeded the rate limit on a public endpoint
    /// (see `crate::auth::rate_limit`). Rendered as 429 with a
    /// `Retry-After` hint rather than the JSON-only shape below,
    /// since well-behaved clients back off on the header.
    TooManyRequests,
    Internal(String),
    Db(sqlx::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // 429 carries a header (`Retry-After`), so it can't go through
        // the (status, body) tuple path the other variants share.
        if let ApiError::TooManyRequests = self {
            let body = Json(serde_json::json!({"error": "rate_limited"}));
            let mut resp = (StatusCode::TOO_MANY_REQUESTS, body).into_response();
            resp.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                axum::http::HeaderValue::from_static("60"),
            );
            return resp;
        }
        let (status, msg, detail) = match self {
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized", None),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not_found", None),
            ApiError::BadRequest(d) => {
                tracing::warn!(detail = %d, "bad request in api");
                (StatusCode::BAD_REQUEST, "bad_request", Some(d))
            }
            // Returned early above (needs a Retry-After header).
            ApiError::TooManyRequests => unreachable!("handled before this match"),
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

/// Cast an `anyhow::Error` back into a `sqlx::Error` for our
/// `ApiError::Db` variant. The db helpers wrap `sqlx::Error` in
/// `anyhow::Error` for context; this unwraps it for the API layer.
pub(crate) fn downcast_db(e: anyhow::Error) -> sqlx::Error {
    e.downcast::<sqlx::Error>()
        .unwrap_or_else(|orig| sqlx::Error::Protocol(format!("non-sqlx db error: {orig}")))
}

/// Wire shape for an artifact row. Hides `asset_path` (clients don't
/// need it — they reference artifacts by `id`) but exposes both
/// summary fields so the meeting compose UI can render the short
/// summary as a chip preview.
#[derive(Debug, Serialize)]
pub(crate) struct ArtifactDto {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub short_summary: Option<String>,
    pub long_summary: Option<String>,
    pub summary_status: String,
    pub size_bytes: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<crate::storage::ArtifactRow> for ArtifactDto {
    fn from(row: crate::storage::ArtifactRow) -> Self {
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

/// Wire shape for a moment. Mirrors `db::MomentRow` minus internal
/// fields, with `screenshot_url` derived from `asset_path` (clients
/// never see the on-disk path).
#[derive(Debug, Serialize)]
pub(crate) struct MomentDto {
    pub id: String,
    pub kind: String,
    pub t: i64,
    pub note: Option<String>,
    pub summary: Option<String>,
    pub summary_status: String,
    /// `Some` when the moment has a screenshot on disk. Absolute
    /// path on this server (relative to its origin); clients fetch
    /// it directly. `None` when no screenshot was captured.
    pub screenshot_url: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl MomentDto {
    pub(crate) fn from_row(row: crate::storage::MomentRow, meeting_id: &str) -> Self {
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
