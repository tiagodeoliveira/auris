//! HTTP client for mnemo's `/events` and `/recall` endpoints.
//!
//! Reads `AURIS_MNEMO_URL`, `AURIS_MNEMO_M2M_CLIENT_ID`, and
//! `AURIS_MNEMO_M2M_CLIENT_SECRET` from the environment. If any of the three
//! is missing, the client returns `MnemoClient::Disabled` and all operations
//! become no-ops. This makes the integration trivially opt-in for dev / tests.
//!
//! Authentication uses the Auth0 client_credentials (M2M) grant. A token is
//! fetched at first use, cached, and refreshed automatically when it is
//! within `TOKEN_SKEW` of expiry or when the server returns 401.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use super::payload::IngestEvent;
use super::recall::{RecallParams, RecalledContext};

const PUSH_TIMEOUT: Duration = Duration::from_secs(5);
// Skew window: refresh tokens 5 minutes before expiry.
const TOKEN_SKEW: Duration = Duration::from_secs(5 * 60);

const DEFAULT_AUTH0_DOMAIN: &str = "dev-jrva0wzk3qkdxcar.us.auth0.com";
const DEFAULT_AUDIENCE: &str = "https://mnemo.tiago.tools";

#[derive(Debug, Clone)]
pub enum MnemoClient {
    Disabled,
    Enabled(EnabledClient),
}

#[derive(Debug, Clone)]
pub struct EnabledClient {
    base_url: String,
    auth0_domain: String,
    audience: String,
    client_id: String,
    client_secret: String,
    http: reqwest::Client,
    /// Hostname for the `context.workstation` field.
    pub workstation: String,
    token_cache: Arc<RwLock<Option<CachedToken>>>,
}

#[derive(Debug, Clone)]
struct CachedToken {
    access_token: String,
    expires_at: SystemTime,
}

#[derive(Debug, Deserialize)]
struct M2mTokenResp {
    access_token: String,
    expires_in: u64,
}

#[derive(Debug, Error)]
pub enum MnemoError {
    #[error("http request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("mnemo returned {status}: {body}")]
    BadStatus {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("auth0 token error: {0}")]
    TokenError(String),
}

impl MnemoClient {
    /// Construct from env. Never errors — if config is missing, returns
    /// `Disabled` and logs a one-line debug message so it's visible in
    /// dev logs.
    pub fn from_env() -> Self {
        let url = crate::env::var_opt("AURIS_MNEMO_URL");
        let client_id = crate::env::var_opt("AURIS_MNEMO_M2M_CLIENT_ID");
        let client_secret = crate::env::var_opt("AURIS_MNEMO_M2M_CLIENT_SECRET");

        match (url, client_id, client_secret) {
            (Some(url), Some(cid), Some(secret)) => {
                let http = reqwest::Client::builder()
                    .timeout(PUSH_TIMEOUT)
                    .build()
                    .expect("reqwest client builder");
                let workstation = crate::env::var_opt("AURIS_MNEMO_WORKSTATION")
                    .unwrap_or_else(default_workstation);
                let auth0_domain = crate::env::var_opt("AURIS_MNEMO_AUTH0_DOMAIN")
                    .unwrap_or_else(|| DEFAULT_AUTH0_DOMAIN.to_string());
                let audience = crate::env::var_opt("AURIS_MNEMO_AUDIENCE")
                    .unwrap_or_else(|| DEFAULT_AUDIENCE.to_string());
                tracing::info!(%url, %workstation, %audience, "mnemo client enabled (M2M)");
                Self::Enabled(EnabledClient {
                    base_url: url.trim_end_matches('/').to_string(),
                    auth0_domain,
                    audience,
                    client_id: cid,
                    client_secret: secret,
                    http,
                    workstation,
                    token_cache: Arc::new(RwLock::new(None)),
                })
            }
            _ => {
                debug!(
                    "mnemo client disabled (set AURIS_MNEMO_URL, \
                     AURIS_MNEMO_M2M_CLIENT_ID, and AURIS_MNEMO_M2M_CLIENT_SECRET to enable)"
                );
                Self::Disabled
            }
        }
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled(_))
    }

    pub fn workstation(&self) -> &str {
        match self {
            Self::Disabled => "",
            Self::Enabled(c) => &c.workstation,
        }
    }

    /// Push an event. No-op when disabled. On error, logs a warning and
    /// returns the error so callers can decide whether to retry — the
    /// pusher task currently chooses not to.
    pub async fn push_event(&self, event: &IngestEvent) -> Result<(), MnemoError> {
        let client = match self {
            Self::Disabled => return Ok(()),
            Self::Enabled(c) => c,
        };
        let token = client.access_token().await?;
        let url = format!("{}/events", client.base_url);
        let resp = client
            .http
            .post(&url)
            .bearer_auth(&token)
            .json(event)
            .send()
            .await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            // Token might be stale despite our skew. Invalidate and retry once.
            client.invalidate_token().await;
            let token = client.access_token().await?;
            let resp = client
                .http
                .post(&url)
                .bearer_auth(&token)
                .json(event)
                .send()
                .await?;
            return process_push_response(resp, event).await;
        }
        process_push_response(resp, event).await
    }

    /// Query mnemo for prior memories. Returns `Ok(empty)` when the
    /// client is disabled or no dimensions were requested, so callers can
    /// always trust the result type.
    pub async fn recall(&self, params: &RecallParams) -> Result<RecalledContext, MnemoError> {
        let client = match self {
            Self::Disabled => return Ok(RecalledContext::default()),
            Self::Enabled(c) => c,
        };
        if !params.has_any() {
            return Ok(RecalledContext::default());
        }
        let url = format!("{}/recall?{}", client.base_url, params.to_query());
        let token = client.access_token().await?;
        let resp = client.http.get(&url).bearer_auth(&token).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            client.invalidate_token().await;
            let token = client.access_token().await?;
            let resp = client.http.get(&url).bearer_auth(&token).send().await?;
            return process_recall_response(resp).await;
        }
        process_recall_response(resp).await
    }
}

async fn process_push_response(
    resp: reqwest::Response,
    event: &IngestEvent,
) -> Result<(), MnemoError> {
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        warn!(
            %status,
            session_id = %event.session_id,
            turn_count = event.turns.len(),
            body = %body,
            "mnemo push failed"
        );
        return Err(MnemoError::BadStatus { status, body });
    }
    debug!(
        session_id = %event.session_id,
        turn_count = event.turns.len(),
        "mnemo push ok"
    );
    Ok(())
}

async fn process_recall_response(resp: reqwest::Response) -> Result<RecalledContext, MnemoError> {
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        warn!(%status, body = %body, "mnemo recall failed");
        return Err(MnemoError::BadStatus { status, body });
    }
    let parsed: RecalledContext = resp.json().await?;
    debug!(
        preferences = parsed.preferences.len(),
        facts = parsed.facts.len(),
        episodes = parsed.episodes.len(),
        has_project = parsed.project.is_some(),
        "mnemo recall ok"
    );
    Ok(parsed)
}

impl EnabledClient {
    /// Returns a valid access token, fetching one if the cache is empty or
    /// the cached token is within `TOKEN_SKEW` of expiry.
    async fn access_token(&self) -> Result<String, MnemoError> {
        // Fast path: read lock.
        {
            let guard = self.token_cache.read().await;
            if let Some(cached) = guard.as_ref() {
                if cached.expires_at > SystemTime::now() + TOKEN_SKEW {
                    return Ok(cached.access_token.clone());
                }
            }
        }
        // Slow path: fetch + write.
        let mut guard = self.token_cache.write().await;
        // Re-check under write lock to avoid duplicate fetches when multiple
        // tasks race to the slow path simultaneously.
        if let Some(cached) = guard.as_ref() {
            if cached.expires_at > SystemTime::now() + TOKEN_SKEW {
                return Ok(cached.access_token.clone());
            }
        }
        let fresh = self.fetch_token().await?;
        *guard = Some(fresh.clone());
        Ok(fresh.access_token)
    }

    async fn invalidate_token(&self) {
        let mut guard = self.token_cache.write().await;
        *guard = None;
    }

    async fn fetch_token(&self) -> Result<CachedToken, MnemoError> {
        let url = format!("https://{}/oauth/token", self.auth0_domain);
        let body = serde_json::json!({
            "grant_type": "client_credentials",
            "client_id": self.client_id,
            "client_secret": self.client_secret,
            "audience": self.audience,
        });
        let resp = self.http.post(&url).json(&body).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let txt = resp.text().await.unwrap_or_default();
            return Err(MnemoError::TokenError(format!("status {status}: {txt}")));
        }
        let parsed: M2mTokenResp = resp.json().await?;
        let expires_at = SystemTime::now() + Duration::from_secs(parsed.expires_in);
        tracing::info!(expires_in = parsed.expires_in, "mnemo M2M token refreshed");
        Ok(CachedToken {
            access_token: parsed.access_token,
            expires_at,
        })
    }
}

fn default_workstation() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}
