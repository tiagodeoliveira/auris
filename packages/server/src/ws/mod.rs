//! HTTP transport: WebSocket (control + /audio) and REST (/meetings…)
//! on a single port via axum. The WS handlers do their own
//! query-string token check; REST handlers use bearer auth via
//! the `crate::api` module. Application boot (DB, recovery, worker
//! spawns, serve loop) lives in `crate::boot`.

pub mod audio;
pub mod control;
mod intent_chat;
mod intent_pairing;
mod intent_quick_asks;

pub use control::auth_failed_response;
pub use control::WsAuthParams;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::time::Duration;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::CorsLayer;

use crate::context::ServerHandle;

// WS close codes per RFC 6455. Tungstenite gave us named variants
// (`CloseCode::Policy` etc.); axum's `CloseFrame` takes raw `u16`.
// Centralised here so the literals don't sprinkle through the file.
// (Auth failures land as plain HTTP 401 before the upgrade — no
// 1008 close-frame path is needed.)
pub(crate) const CLOSE_GOING_AWAY: u16 = 1001;
pub(crate) const CLOSE_INTERNAL: u16 = 1011;

/// Build the unified Router: WS handlers at `/` and `/audio`, REST
/// handlers under `/meetings`. CORS-permissive so a future PWA hosted
/// on a different origin can fetch without server-side allowlisting.
pub(crate) fn make_app_router(handle: ServerHandle) -> Router {
    let api_state = crate::api::ApiState {
        db: handle.db.clone(),
        auth: handle.auth.clone(),
        moment_created_tx: handle.moment_created_tx.clone(),
        artifact_created_tx: handle.artifact_created_tx.clone(),
        wrap_up_retry_tx: handle.wrap_up_retry_tx.clone(),
        agent_kick_tx: handle.agent_kick_tx.clone(),
        bus: handle.bus.clone(),
    };
    let api_router = crate::api::make_router(api_state);
    // Unauthenticated probe: process-up + DB-reachable. Mirrors mnemo's
    // /healthz contract (200 with `{"ok":true,"db":true}` on healthy,
    // 503 with `db:false` when the pool can't acquire). Paired with
    // the `healthz` subcommand in main.rs so the container can probe
    // itself without a shell.
    let health_router = Router::new()
        .route("/healthz", get(healthz_handler))
        .with_state(handle.db.clone());
    let ws_router = Router::new()
        .route("/", get(control::ws_control_handler))
        .route("/audio", get(control::ws_audio_handler))
        .route("/stt", get(crate::ws::audio::ws_handler))
        .with_state(handle);
    api_router
        .merge(health_router)
        .merge(ws_router)
        .layer(crate::observability::axum_metrics_layer())
        // Defense-in-depth: a panicking handler must yield a 500, not
        // a connection reset. Inside CORS so the 500 still carries
        // CORS headers for browser clients (PWA).
        .layer(CatchPanicLayer::new())
        .layer(CorsLayer::permissive())
}

/// `GET /healthz` — DB ping + 200/503. No auth, no logging on the
/// happy path (the healthcheck fires every 10s and would otherwise
/// drown the access log).
async fn healthz_handler(State(db): State<sqlx::PgPool>) -> Response {
    let db_ok = tokio::time::timeout(Duration::from_secs(2), sqlx::query("SELECT 1").execute(&db))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false);
    let status = if db_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        [("content-type", "application/json")],
        format!(r#"{{"ok":{db_ok},"db":{db_ok}}}"#),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// A handler panic must surface as a 500 response, not a dropped
    /// connection. This pins that the `catch-panic` feature is enabled
    /// and the layer behaves; `make_app_router` wires the same layer
    /// onto the real router (asserted by the grep step in the plan —
    /// constructing a full `ServerHandle` needs a live DB + LLM client,
    /// too heavy for a unit test).
    // Named handler with a concrete `IntoResponse` return type. An
    // inline `|| async { panic!() }` closure infers the never type
    // `!`, which trips the rust_2024_compatibility never-type-fallback
    // deny lint (`!: IntoResponse` will fail in edition 2024); a
    // declared `-> StatusCode` return sidesteps that while still
    // diverging via panic at runtime.
    async fn boom() -> StatusCode {
        panic!("synthetic handler panic")
    }

    #[tokio::test]
    async fn catch_panic_layer_turns_handler_panic_into_500() {
        let app = Router::new()
            .route("/boom", get(boom))
            .layer(CatchPanicLayer::new());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/boom")
                    .body(Body::empty())
                    .expect("static request parts are infallible"),
            )
            .await
            .expect("layered router is Infallible");
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
