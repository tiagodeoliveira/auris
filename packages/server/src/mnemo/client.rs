//! HTTP client for mnemo's `/events` endpoint.
//!
//! Reads `AURIS_MNEMO_URL` and `AURIS_MNEMO_API_KEY`
//! from the environment. If either is missing, the client returns
//! `MnemoClient::Disabled` and `push_event` becomes a no-op. This makes the
//! integration trivially opt-in for dev / tests.

use std::time::Duration;

use thiserror::Error;
use tracing::{debug, warn};

use super::payload::IngestEvent;
use super::recall::{RecallParams, RecalledContext};

const PUSH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub enum MnemoClient {
    Disabled,
    Enabled(EnabledClient),
}

#[derive(Debug, Clone)]
pub struct EnabledClient {
    base_url: String,
    api_key: String,
    http: reqwest::Client,
    /// Hostname for the `context.workstation` field.
    pub workstation: String,
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
}

impl MnemoClient {
    /// Construct from env. Never errors — if config is missing, returns
    /// `Disabled` and logs a one-line debug message so it's visible in
    /// dev logs.
    pub fn from_env() -> Self {
        let url = crate::env::var_opt("AURIS_MNEMO_URL");
        let api_key = crate::env::var_opt("AURIS_MNEMO_API_KEY");
        match (url, api_key) {
            (Some(url), Some(api_key)) => {
                let http = reqwest::Client::builder()
                    .timeout(PUSH_TIMEOUT)
                    .build()
                    .expect("reqwest client builder");
                let workstation = crate::env::var_opt("AURIS_MNEMO_WORKSTATION")
                    .unwrap_or_else(default_workstation);
                tracing::info!(%url, %workstation, "mnemo client enabled");
                Self::Enabled(EnabledClient {
                    base_url: url.trim_end_matches('/').to_string(),
                    api_key,
                    http,
                    workstation,
                })
            }
            _ => {
                debug!(
                    "mnemo client disabled (set AURIS_MNEMO_URL and \
                     AURIS_MNEMO_API_KEY to enable)"
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
        let url = format!("{}/events", client.base_url);
        let resp = client
            .http
            .post(&url)
            .header("x-api-key", &client.api_key)
            .json(event)
            .send()
            .await?;
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
        let resp = client
            .http
            .get(&url)
            .header("x-api-key", &client.api_key)
            .send()
            .await?;
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
}

fn default_workstation() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}
