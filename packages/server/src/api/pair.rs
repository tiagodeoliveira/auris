// ─────────────────────────────────────────────────────────────────────
// Device pairing
// ─────────────────────────────────────────────────────────────────────
//
// Mint goes over WS (`Intent::MintPairCode` → `Event::PairCodeMinted`).
// The PWA can't reach the WS pre-auth, so redeem + refresh remain HTTP.

use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{require_auris_issuer, require_user, ApiError, ApiState};
use crate::auth::rate_limit::{client_ip, pair_endpoint_allows};

#[derive(Debug, Deserialize)]
pub(crate) struct PairRedeemRequest {
    /// User-typed (or pasted-from-mobile) code. Either form accepted —
    /// `K7M2-4XQ9` or `k7m24xq9`. Normalized server-side.
    code: String,
    /// Optional human label, e.g. "Living room G2". Defaults to
    /// "G2 glasses" if absent.
    #[serde(default)]
    device_label: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PairTokenResponse {
    access_token: String,
    refresh_token: String,
    device_id: String,
    /// Seconds until access-token expiry. Conventional OAuth shape.
    expires_in: i64,
}

pub(crate) async fn pair_redeem(
    State(state): State<ApiState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<PairRedeemRequest>,
) -> Result<Json<PairTokenResponse>, ApiError> {
    // Shed before any issuer/DB work — a flood of bad codes must not
    // get to spend CPU. See `crate::auth::rate_limit`.
    if !pair_endpoint_allows(client_ip(&headers, peer)) {
        return Err(ApiError::TooManyRequests);
    }
    let issuer = require_auris_issuer(&state)?;
    let result =
        crate::auth::pairing::redeem_code(&state.db, issuer, &body.code, body.device_label)
            .await
            .map_err(|e| {
                // `invalid_code` is user-facing (wrong / expired / used).
                // Anything else is an unexpected server fault.
                if e.to_string().contains("invalid_code") {
                    ApiError::BadRequest("invalid_code".to_string())
                } else {
                    ApiError::Internal(e.to_string())
                }
            })?;
    // Notify the user's other open WS connections (typically mobile,
    // sitting on the pair sheet). They'll re-fetch `/pair/devices` to
    // show the freshly-paired device + flip the pair sheet to its
    // success state.
    state
        .bus
        .emit(
            result.user_id.clone(),
            crate::protocol::Event::PairedDevicesChanged,
        )
        .await;
    Ok(Json(PairTokenResponse {
        access_token: result.access_token,
        refresh_token: result.refresh_token,
        device_id: result.device_id,
        expires_in: crate::auth::pairing::ACCESS_TTL.num_seconds(),
    }))
}

#[derive(Debug, Deserialize)]
pub(crate) struct PairRefreshRequest {
    refresh_token: String,
}

pub(crate) async fn pair_refresh(
    State(state): State<ApiState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<PairRefreshRequest>,
) -> Result<Json<PairTokenResponse>, ApiError> {
    // Shed before the argon2 fan-out over every active device hash.
    if !pair_endpoint_allows(client_ip(&headers, peer)) {
        return Err(ApiError::TooManyRequests);
    }
    let issuer = require_auris_issuer(&state)?;
    let rotated = crate::auth::pairing::rotate_refresh(&state.db, issuer, &body.refresh_token)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::Unauthorized)?;
    // Note: `device_id` isn't echoed here — the client already has it
    // from the original redeem and stores it alongside the tokens.
    // Returning it would leak it across the wire on every refresh.
    Ok(Json(PairTokenResponse {
        access_token: rotated.access_token,
        refresh_token: rotated.refresh_token,
        device_id: String::new(),
        expires_in: crate::auth::pairing::ACCESS_TTL.num_seconds(),
    }))
}

#[derive(Debug, Serialize)]
pub(crate) struct DeviceListItem {
    device_id: String,
    device_label: String,
    paired_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
}

pub(crate) async fn pair_list_devices(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Vec<DeviceListItem>>, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    let rows = crate::auth::pairing::list_devices(&state.db, &user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(
        rows.into_iter()
            .map(|d| DeviceListItem {
                device_id: d.device_id,
                device_label: d.device_label,
                paired_at: d.paired_at,
                last_seen_at: d.last_seen_at,
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
pub(crate) struct PairRevokeRequest {
    device_id: String,
}

pub(crate) async fn pair_revoke(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<PairRevokeRequest>,
) -> Result<StatusCode, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    let affected = crate::auth::pairing::revoke_device(&state.db, &user_id, &body.device_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if affected == 0 {
        // Indistinguishable to the caller — could mean "doesn't
        // exist", "belongs to another user", or "already revoked".
        // All three resolve to "your action wouldn't have done
        // anything anyway", which 404 captures.
        return Err(ApiError::NotFound);
    }
    // Notify the user's WS connections so their "Paired devices"
    // list re-fetches without a full reload.
    state
        .bus
        .emit(user_id, crate::protocol::Event::PairedDevicesChanged)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::{Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;

    use crate::api::{ApiState, ArtifactCreated, MomentCreated, WrapUpRetry};
    use tokio::sync::broadcast;

    /// Disabled-mode router fronted by a fixed peer address, so
    /// `ConnectInfo<SocketAddr>` resolves under `oneshot`. In Disabled
    /// mode `require_auris_issuer` 500s — which is exactly what lets us
    /// prove rate-limit shedding happens *before* issuer/DB work: an
    /// allowed request returns 500, only a shed request returns 429.
    fn disabled_router(pool: PgPool, peer: SocketAddr) -> axum::Router {
        let (moment_created_tx, _) = broadcast::channel::<MomentCreated>(8);
        let (artifact_created_tx, _) = broadcast::channel::<ArtifactCreated>(8);
        let (wrap_up_retry_tx, _) = broadcast::channel::<WrapUpRetry>(8);
        let (agent_kick_tx, _) = broadcast::channel::<crate::agent::AgentKick>(8);
        let (fanout, _) = broadcast::channel::<crate::protocol::UserEvent>(8);
        let (durable_tx, _durable_rx) = tokio::sync::mpsc::channel::<crate::protocol::UserEvent>(8);
        crate::api::make_router(ApiState {
            db: pool,
            auth: std::sync::Arc::new(crate::auth::AuthMode::Disabled),
            moment_created_tx,
            artifact_created_tx,
            wrap_up_retry_tx,
            agent_kick_tx,
            bus: crate::context::EventBus::new(fanout, durable_tx),
        })
        .layer(MockConnectInfo(peer))
    }

    async fn post(app: &axum::Router, path: &str, json: &str) -> StatusCode {
        app.clone()
            .oneshot(
                Request::post(path)
                    .header("content-type", "application/json")
                    .body(Body::from(json.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    #[sqlx::test]
    async fn pair_redeem_sheds_after_per_ip_budget(pool: PgPool) {
        // Regression for improvement #21: the unauthenticated pair
        // endpoints must rate-limit before spending CPU. A flood from a
        // single IP must start returning 429 once the per-IP budget is
        // spent. Uses a TEST-NET-2 peer disjoint from the unit-test IPs
        // so the process-global per-IP bucket starts clean.
        let peer: SocketAddr = "198.51.100.21:55000".parse().unwrap();
        let app = disabled_router(pool, peer);
        let body = r#"{"code":"K7M2-4XQ9"}"#;

        for i in 0..crate::auth::rate_limit::PER_IP_MAX {
            let status = post(&app, "/pair/redeem", body).await;
            assert_ne!(
                status,
                StatusCode::TOO_MANY_REQUESTS,
                "request {i} is within the per-IP budget and must not be shed"
            );
        }

        let resp = app
            .clone()
            .oneshot(
                Request::post("/pair/redeem")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "the request past PER_IP_MAX must be shed with 429"
        );
        assert_eq!(
            resp.headers()
                .get(axum::http::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("60"),
            "429 must advertise Retry-After: 60"
        );
    }

    #[sqlx::test]
    async fn pair_refresh_sheds_after_per_ip_budget(pool: PgPool) {
        // Same contract for /pair/refresh, whose argon2 fan-out over
        // every active device hash is the more expensive of the two.
        let peer: SocketAddr = "203.0.113.21:55000".parse().unwrap();
        let app = disabled_router(pool, peer);
        let body = r#"{"refresh_token":"garbage"}"#;

        for i in 0..crate::auth::rate_limit::PER_IP_MAX {
            let status = post(&app, "/pair/refresh", body).await;
            assert_ne!(
                status,
                StatusCode::TOO_MANY_REQUESTS,
                "request {i} is within the per-IP budget and must not be shed"
            );
        }

        assert_eq!(
            post(&app, "/pair/refresh", body).await,
            StatusCode::TOO_MANY_REQUESTS,
            "the request past PER_IP_MAX must be shed with 429"
        );
    }
}
