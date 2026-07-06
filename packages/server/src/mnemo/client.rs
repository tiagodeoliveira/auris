//! HTTP client for mnemo's `/events` and `/recall` endpoints.
//!
//! Reads `AURIS_MNEMO_URL` from the environment to gate the integration.
//! Per-request authentication comes from `MnemoTokenStore`: each call
//! looks up the user's cached JWT (deposited at WS handshake, refreshed
//! by `Intent::SetAuthToken`) and sends it as `Authorization: Bearer …`.
//! Auris and mnemo share an Auth0 audience, so the user's own auris
//! session token authenticates against mnemo unchanged. Mnemo extracts
//! the actor from the JWT — there is no shared API key.
//!
//! Failed pushes are queued in the token store rather than dropped:
//! auth gaps (no cached token, or mnemo returns 401) queue inside
//! `push_event` itself, and transient failures (network errors,
//! 5xx/429, open circuit breaker) queue via `push_event_or_queue`.
//! Two triggers drain the queue, both through the order-preserving
//! `drain_pending`: the next handshake / SetAuthToken for that user,
//! and a periodic 30s retry task (`mnemo::spawn_tasks`). The queue is
//! in-memory — an auris restart during a mnemo outage still loses it
//! (accepted; the offline recover-meeting binary is the manual
//! backstop). Reads (`recall`) never queue — stale recall data has no
//! value after the meeting it was for has moved on.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use thiserror::Error;
use tracing::{debug, info, warn};

use super::payload::IngestEvent;
use super::recall::{RecallParams, RecalledContext};
use super::token_store::MnemoTokenStore;
use crate::util::circuit_breaker::CircuitBreaker;

/// Diagnostic peek at a JWT's payload claims WITHOUT verifying the
/// signature. Only used to log details when mnemo rejects a token
/// (401) — surfaces the most common rejection reasons:
///   - audience mismatch (token's `aud` doesn't match what mnemo
///     expects)
///   - token expired (negative seconds-until-expiry)
///   - wrong issuer (different Auth0 tenant / dev vs prod)
///
/// Returns None when the token isn't a 3-part JWS or the payload
/// isn't valid JSON. Never panics, never errors — diagnostics only.
fn peek_jwt_claims(token: &str) -> Option<JwtPeek> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let _sig = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&decoded).ok()
}

#[derive(Debug, serde::Deserialize)]
struct JwtPeek {
    /// `aud` claim. Auth0 emits either a string (one audience) or
    /// an array (token has access to multiple). serde_json::Value
    /// captures both shapes for logging.
    #[serde(default)]
    aud: Option<serde_json::Value>,
    /// Token expiry as unix-epoch seconds.
    #[serde(default)]
    exp: Option<i64>,
    /// Issuer URL — distinguishes dev vs prod Auth0 tenants when
    /// the wrong one ends up cached.
    #[serde(default)]
    iss: Option<String>,
}

/// Format the JWT peek as a single log-friendly string with the
/// fields we care about for a 401 root-cause: `aud`, `iss`, and
/// time-until-expiry. None of the actual signature is logged.
fn jwt_diag_line(token: &str) -> String {
    match peek_jwt_claims(token) {
        Some(p) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let exp_status = match p.exp {
                Some(exp) => {
                    let delta = exp - now;
                    if delta < 0 {
                        format!("EXPIRED ({}s ago)", -delta)
                    } else {
                        format!("valid for {}s", delta)
                    }
                }
                None => "no exp".into(),
            };
            let aud = p
                .aud
                .as_ref()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "(missing)".into());
            let iss = p.iss.as_deref().unwrap_or("(missing)");
            format!("aud={aud}, iss={iss}, exp={exp_status}")
        }
        None => "(unparseable JWT)".into(),
    }
}

const PUSH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub enum MnemoClient {
    Disabled,
    Enabled(EnabledClient),
}

#[derive(Debug, Clone)]
pub struct EnabledClient {
    base_url: String,
    http: reqwest::Client,
    tokens: Arc<MnemoTokenStore>,
    /// Hostname for the `context.workstation` field.
    pub workstation: String,
    push_breaker: Option<Arc<CircuitBreaker>>,
    recall_breaker: Option<Arc<CircuitBreaker>>,
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
    #[error("circuit breaker open: {0}")]
    CircuitOpen(String),
}

impl MnemoError {
    /// Whether a later retry could plausibly succeed. Drives the
    /// queue-vs-drop decision in `push_event_or_queue` and
    /// `drain_pending`:
    /// - connection-level failures (refused / timeout / reset) — yes
    /// - open circuit breaker — yes (it will half-open after cooldown)
    /// - 5xx and 429 — yes (mnemo restarting or shedding load)
    /// - any other 4xx — no: the payload itself was rejected and
    ///   replaying it verbatim fails identically. (401 never reaches
    ///   callers as an Err — `push_event` clears the token and queues
    ///   the event internally.)
    pub fn is_transient(&self) -> bool {
        match self {
            MnemoError::Http(_) => true,
            MnemoError::CircuitOpen(_) => true,
            MnemoError::BadStatus { status, .. } => {
                status.is_server_error() || *status == reqwest::StatusCode::TOO_MANY_REQUESTS
            }
        }
    }
}

impl MnemoClient {
    /// Construct from env + a shared token store. Never errors — if
    /// `AURIS_MNEMO_URL` is unset, returns `Disabled` and logs a
    /// one-line debug message so it's visible in dev logs.
    ///
    /// `push_breaker` and `recall_breaker` are optional; pass `None`
    /// to disable circuit-breaking for that operation (e.g. in tests).
    pub fn from_env(
        tokens: Arc<MnemoTokenStore>,
        push_breaker: Option<Arc<CircuitBreaker>>,
        recall_breaker: Option<Arc<CircuitBreaker>>,
    ) -> Self {
        let url = crate::config::var_opt("AURIS_MNEMO_URL");
        match url {
            Some(url) => {
                let workstation = crate::config::var_opt("AURIS_MNEMO_WORKSTATION")
                    .unwrap_or_else(default_workstation);
                info!(%url, %workstation, "mnemo client enabled (per-user JWT from token store)");
                Self::with_base_url(url, workstation, tokens, push_breaker, recall_breaker)
            }
            None => {
                debug!("mnemo client disabled (set AURIS_MNEMO_URL to enable)");
                Self::Disabled
            }
        }
    }

    /// Construct an Enabled client against an explicit base URL,
    /// bypassing the environment. This is the testing seam — unit
    /// tests point it at an ephemeral local stub without racing on
    /// process-global env vars. `from_env` delegates here.
    pub fn with_base_url(
        base_url: impl Into<String>,
        workstation: impl Into<String>,
        tokens: Arc<MnemoTokenStore>,
        push_breaker: Option<Arc<CircuitBreaker>>,
        recall_breaker: Option<Arc<CircuitBreaker>>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(PUSH_TIMEOUT)
            .build()
            .expect("reqwest client builder");
        Self::Enabled(EnabledClient {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http,
            tokens,
            workstation: workstation.into(),
            push_breaker,
            recall_breaker,
        })
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

    /// Access the shared token store. Returns `None` when disabled.
    /// Used by the WS intent handler to deposit tokens + drain queues.
    pub fn tokens(&self) -> Option<Arc<MnemoTokenStore>> {
        match self {
            Self::Disabled => None,
            Self::Enabled(c) => Some(c.tokens.clone()),
        }
    }

    /// Push an event to `/events` on behalf of `user_id`.
    ///
    /// Returns Ok in three situations: success, no-cached-token (event
    /// queued), and 401 (token cleared, event queued). Transient
    /// failures (network error, 5xx/429, open breaker) return Err
    /// WITHOUT queueing: fire-and-forget callers should use
    /// `push_event_or_queue`, which queues those for the periodic
    /// drain; replay paths (`drain_pending`, the offline
    /// recover-meeting binary) call this directly, so a failed replay
    /// can never double-enqueue and hard errors stay visible.
    pub async fn push_event(&self, user_id: &str, event: &IngestEvent) -> Result<(), MnemoError> {
        let client = match self {
            Self::Disabled => return Ok(()),
            Self::Enabled(c) => c,
        };
        // RAII breaker guard (improvement #16): Drop records a failure
        // unless succeed() is called, so a caller wrapping this future
        // in `timeout`/`select!` can never leak a HalfOpen probe and
        // wedge the breaker until restart.
        let mut guard = None;
        if let Some(b) = &client.push_breaker {
            match b.try_acquire() {
                Some(g) => guard = Some(g),
                None => return Err(MnemoError::CircuitOpen("mnemo.push".into())),
            }
        }
        let result = self.push_event_inner(client, user_id, event).await;
        if result.is_ok() {
            if let Some(g) = guard.as_mut() {
                g.succeed();
            }
        }
        drop(guard);
        result
    }

    /// Fire-and-forget push: like `push_event`, but a transient
    /// failure (network, 5xx/429, open breaker) enqueues the event
    /// for the periodic drain instead of surfacing an Err the caller
    /// would drop. Permanent rejections (non-401 4xx) are dropped
    /// with a warn — retrying an identical payload fails identically.
    pub async fn push_event_or_queue(&self, user_id: &str, event: &IngestEvent) {
        let Self::Enabled(c) = self else { return };
        match self.push_event(user_id, event).await {
            Ok(()) => {}
            Err(e) if e.is_transient() => {
                warn!(
                    user_id,
                    session_id = %event.session_id,
                    error = %e,
                    queued = true,
                    "mnemo: transient push failure — event queued for retry"
                );
                c.tokens.enqueue(user_id, event.clone());
            }
            Err(e) => {
                warn!(
                    user_id,
                    session_id = %event.session_id,
                    error = %e,
                    queued = false,
                    "mnemo: permanent push failure — event dropped"
                );
            }
        }
    }

    /// Replay this user's queued events, FIFO. Stops at the first
    /// transient failure and puts the failed event plus the untried
    /// remainder back at the FRONT of the queue (mnemo stamps
    /// `created_at` at ingest, so per-user order is load-bearing).
    /// Permanently rejected events are dropped with a warn.
    ///
    /// No-op when disabled or when the user has no cached token —
    /// without a token, every replay would just rotate the queue
    /// through `push_event`'s internal no-token re-queue.
    ///
    /// Deliberately does NOT pre-check the circuit breaker:
    /// `CircuitBreaker::allow()` is side-effecting (it claims the
    /// half-open probe slot), so the first real `push_event` here IS
    /// the post-cooldown probe; a `CircuitOpen` Err stops the drain
    /// immediately at zero HTTP cost.
    pub async fn drain_pending(&self, user_id: &str) {
        let Self::Enabled(c) = self else { return };
        if c.tokens.get(user_id).is_none() {
            return;
        }
        let mut events: VecDeque<IngestEvent> = c.tokens.take_pending(user_id).into();
        if events.is_empty() {
            return;
        }
        debug!(
            user_id,
            count = events.len(),
            "mnemo: draining pending events"
        );
        while let Some(ev) = events.pop_front() {
            match self.push_event(user_id, &ev).await {
                // Ok covers clean pushes AND push_event's internal
                // 401/no-token deferral (re-queued at the back; order
                // holds because every later event in this drain takes
                // the same path).
                Ok(()) => {}
                Err(e) if e.is_transient() => {
                    warn!(
                        user_id,
                        error = %e,
                        requeued = events.len() + 1,
                        "mnemo: drain hit transient failure — requeueing remainder for next tick"
                    );
                    events.push_front(ev);
                    c.tokens
                        .requeue_front(user_id, events.into_iter().collect());
                    return;
                }
                Err(e) => {
                    warn!(
                        user_id,
                        session_id = %ev.session_id,
                        error = %e,
                        "mnemo: dropping permanently rejected event"
                    );
                }
            }
        }
    }

    async fn push_event_inner(
        &self,
        client: &EnabledClient,
        user_id: &str,
        event: &IngestEvent,
    ) -> Result<(), MnemoError> {
        let Some(token) = client.tokens.get(user_id) else {
            warn!(user_id, "mnemo: no token cached — queueing event");
            client.tokens.enqueue(user_id, event.clone());
            return Ok(());
        };
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
            // Pull the response body BEFORE consuming the response —
            // mnemo's reject reason ("token expired", "audience
            // mismatch", "user not found") lives there. Without
            // this body in the log there's no way to know which
            // class of 401 we're hitting, and the answer changes
            // the fix entirely (auth refresh vs config vs DB).
            let body = resp.text().await.unwrap_or_default();
            warn!(
                user_id,
                session_id = %event.session_id,
                token_diag = %jwt_diag_line(&token),
                body = %body,
                "mnemo: 401 — cached token cleared, event queued"
            );
            client.tokens.clear(user_id);
            client.tokens.enqueue(user_id, event.clone());
            return Ok(());
        }
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
        info!(
            user_id,
            session_id = %event.session_id,
            turn_count = event.turns.len(),
            "mnemo push ok"
        );
        Ok(())
    }

    /// Query `/recall` on behalf of `user_id`. Returns empty when
    /// disabled, no params requested, no cached JWT, or mnemo returns
    /// 401. Reads deliberately don't queue: a recall is only useful
    /// in the moment its caller wants it, replaying it later is
    /// meaningless.
    pub async fn recall(
        &self,
        user_id: &str,
        params: &RecallParams,
    ) -> Result<RecalledContext, MnemoError> {
        let client = match self {
            Self::Disabled => return Ok(RecalledContext::default()),
            Self::Enabled(c) => c,
        };
        if !params.has_any() {
            return Ok(RecalledContext::default());
        }
        // RAII breaker guard (improvement #16) — see push_event.
        let mut guard = None;
        if let Some(b) = &client.recall_breaker {
            match b.try_acquire() {
                Some(g) => guard = Some(g),
                None => return Err(MnemoError::CircuitOpen("mnemo.recall".into())),
            }
        }
        let result = self.recall_inner(client, user_id, params).await;
        if result.is_ok() {
            if let Some(g) = guard.as_mut() {
                g.succeed();
            }
        }
        drop(guard);
        result
    }

    async fn recall_inner(
        &self,
        client: &EnabledClient,
        user_id: &str,
        params: &RecallParams,
    ) -> Result<RecalledContext, MnemoError> {
        let Some(token) = client.tokens.get(user_id) else {
            warn!(user_id, "mnemo: no token cached — recall returns empty");
            return Ok(RecalledContext::default());
        };
        let url = format!("{}/recall?{}", client.base_url, params.to_query());
        let resp = client.http.get(&url).bearer_auth(&token).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            let body = resp.text().await.unwrap_or_default();
            warn!(
                user_id,
                token_diag = %jwt_diag_line(&token),
                body = %body,
                "mnemo: recall 401 — cached token cleared"
            );
            client.tokens.clear(user_id);
            return Ok(RecalledContext::default());
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(%status, body = %body, "mnemo recall failed");
            return Err(MnemoError::BadStatus { status, body });
        }
        let parsed: RecalledContext = resp.json().await?;
        let total_items: usize = parsed.dimensions.iter().map(|d| d.items.len()).sum();
        debug!(
            user_id,
            dimensions = parsed.dimensions.len(),
            total_items,
            "mnemo recall ok"
        );
        Ok(parsed)
    }
}

fn default_workstation() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mnemo::token_store::MnemoTokenStore;

    /// Build a 3-part JWT (header.payload.sig) with the given JSON
    /// payload. Header and signature are fixed placeholders — the
    /// peek-only path doesn't validate them, so any well-formed
    /// segments work for testing.
    fn jwt_with_payload(payload: serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"RS256\",\"typ\":\"JWT\"}");
        let body = URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
        let sig = URL_SAFE_NO_PAD.encode(b"signature-stub");
        format!("{header}.{body}.{sig}")
    }

    #[test]
    fn peek_extracts_aud_iss_exp_from_well_formed_jwt() {
        let token = jwt_with_payload(serde_json::json!({
            "aud": "https://mnemo.example/",
            "iss": "https://dev-jrva0wzk3qkdxcar.us.auth0.com/",
            "exp": 1_900_000_000_i64,
            "sub": "auth0|abc"
        }));
        let p = peek_jwt_claims(&token).unwrap();
        assert_eq!(
            p.aud.as_ref().and_then(|v| v.as_str()),
            Some("https://mnemo.example/")
        );
        assert_eq!(
            p.iss.as_deref(),
            Some("https://dev-jrva0wzk3qkdxcar.us.auth0.com/")
        );
        assert_eq!(p.exp, Some(1_900_000_000));
    }

    #[test]
    fn peek_handles_aud_as_array() {
        // Auth0 sometimes emits `aud` as a JSON array when the
        // token has multiple audiences. We capture it as
        // serde_json::Value so logging shows the raw shape.
        let token = jwt_with_payload(serde_json::json!({
            "aud": ["https://a.example/", "https://b.example/"],
            "exp": 1_900_000_000_i64
        }));
        let p = peek_jwt_claims(&token).unwrap();
        let arr = p.aud.unwrap();
        assert!(arr.is_array());
        assert_eq!(arr.as_array().unwrap().len(), 2);
    }

    #[test]
    fn peek_returns_none_for_non_jwt_strings() {
        assert!(peek_jwt_claims("not.a.jwt.too.many.parts").is_none());
        assert!(peek_jwt_claims("not-a-jwt").is_none());
        assert!(peek_jwt_claims("").is_none());
    }

    #[test]
    fn diag_line_flags_expired_tokens() {
        // exp set to 1 second after the epoch — definitely expired.
        let token = jwt_with_payload(serde_json::json!({
            "aud": "x",
            "exp": 1_i64
        }));
        let line = jwt_diag_line(&token);
        assert!(line.contains("EXPIRED"), "got: {line}");
    }

    #[test]
    fn diag_line_for_unparseable_token() {
        assert_eq!(jwt_diag_line("garbage"), "(unparseable JWT)");
    }

    #[test]
    fn transient_classification_5xx_429_breaker_yes_other_4xx_no() {
        let bad = |code: u16| MnemoError::BadStatus {
            status: reqwest::StatusCode::from_u16(code).unwrap(),
            body: String::new(),
        };
        // Retry could plausibly succeed:
        assert!(bad(500).is_transient());
        assert!(bad(502).is_transient());
        assert!(bad(503).is_transient());
        assert!(bad(429).is_transient());
        assert!(MnemoError::CircuitOpen("mnemo.push".into()).is_transient());
        // Permanent rejections — replaying the same payload fails the
        // same way (401 never reaches callers; push_event queues it
        // internally):
        assert!(!bad(400).is_transient());
        assert!(!bad(403).is_transient());
        assert!(!bad(404).is_transient());
        assert!(!bad(422).is_transient());
    }

    #[test]
    fn with_base_url_constructs_enabled_client_without_env() {
        let tokens = Arc::new(MnemoTokenStore::new());
        let c = MnemoClient::with_base_url("http://localhost:9999/", "ws-test", tokens, None, None);
        assert!(c.is_enabled());
        assert_eq!(c.workstation(), "ws-test");
    }

    use crate::mnemo::payload::{Turn, TurnRole};
    use crate::util::circuit_breaker::CircuitBreaker as TestBreaker;

    fn test_event(session_id: &str) -> IngestEvent {
        IngestEvent {
            session_id: session_id.into(),
            source: "auris".into(),
            workstation: "h".into(),
            workdir: "/auris".into(),
            project: None,
            turns: vec![Turn {
                role: TurnRole::User,
                content: "x".into(),
            }],
            attributes: std::collections::HashMap::new(),
        }
    }

    /// Spin up a local `/events` stub on an OS-assigned port that
    /// always answers `status` and records every request body.
    /// Returns (base_url, recorded_bodies).
    async fn spawn_events_stub(
        status: u16,
    ) -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    ) {
        let bodies: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = bodies.clone();
        let code = axum::http::StatusCode::from_u16(status).unwrap();
        let app = axum::Router::new().route(
            "/events",
            axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                let recorded = recorded.clone();
                async move {
                    recorded.lock().unwrap().push(body);
                    code
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), bodies)
    }

    #[tokio::test]
    async fn push_event_or_queue_enqueues_on_connection_refused() {
        // Bind to grab a free port, then drop the listener so the
        // connect is refused — a reqwest connection error, i.e.
        // MnemoError::Http, the transient class the breaker exists for.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let tokens = Arc::new(MnemoTokenStore::new());
        tokens.store("u1", "tok".into());
        let client =
            MnemoClient::with_base_url(format!("http://{addr}"), "h", tokens.clone(), None, None);
        client.push_event_or_queue("u1", &test_event("s-1")).await;
        assert_eq!(
            tokens.pending_len("u1"),
            1,
            "transient network failure must queue the event, not drop it"
        );
    }

    #[tokio::test]
    async fn push_event_or_queue_drops_on_permanent_4xx() {
        let (url, bodies) = spawn_events_stub(422).await;
        let tokens = Arc::new(MnemoTokenStore::new());
        tokens.store("u1", "tok".into());
        let client = MnemoClient::with_base_url(url, "h", tokens.clone(), None, None);
        client.push_event_or_queue("u1", &test_event("s-1")).await;
        assert_eq!(bodies.lock().unwrap().len(), 1);
        assert_eq!(
            tokens.pending_len("u1"),
            0,
            "a permanently rejected payload must not be retried forever"
        );
    }

    #[tokio::test]
    async fn drain_pending_pushes_in_order_and_empties_queue() {
        let (url, bodies) = spawn_events_stub(200).await;
        let tokens = Arc::new(MnemoTokenStore::new());
        tokens.store("u1", "tok".into());
        tokens.enqueue("u1", test_event("s-1"));
        tokens.enqueue("u1", test_event("s-2"));
        tokens.enqueue("u1", test_event("s-3"));
        let client = MnemoClient::with_base_url(url, "h", tokens.clone(), None, None);
        client.drain_pending("u1").await;
        assert_eq!(tokens.pending_len("u1"), 0);
        let seen: Vec<String> = bodies
            .lock()
            .unwrap()
            .iter()
            .map(|b| b["session_id"].as_str().unwrap().to_string())
            .collect();
        // mnemo stamps created_at at ingest — FIFO is load-bearing.
        assert_eq!(seen, vec!["s-1", "s-2", "s-3"]);
    }

    #[tokio::test]
    async fn drain_pending_requeues_failed_and_remainder_in_order_on_5xx() {
        let (url, bodies) = spawn_events_stub(503).await;
        let tokens = Arc::new(MnemoTokenStore::new());
        tokens.store("u1", "tok".into());
        tokens.enqueue("u1", test_event("s-1"));
        tokens.enqueue("u1", test_event("s-2"));
        let client = MnemoClient::with_base_url(url, "h", tokens.clone(), None, None);
        client.drain_pending("u1").await;
        // The drain stops at the FIRST transient failure — exactly one
        // HTTP attempt, no pointless hammering of a down service.
        assert_eq!(bodies.lock().unwrap().len(), 1);
        // The failed event AND the untried remainder are back, in order.
        let requeued = tokens.take_pending("u1");
        let ids: Vec<&str> = requeued.iter().map(|e| e.session_id.as_str()).collect();
        assert_eq!(ids, vec!["s-1", "s-2"]);
    }

    #[tokio::test]
    async fn drain_pending_noop_without_token() {
        let (url, bodies) = spawn_events_stub(200).await;
        let tokens = Arc::new(MnemoTokenStore::new());
        tokens.enqueue("u1", test_event("s-1"));
        let client = MnemoClient::with_base_url(url, "h", tokens.clone(), None, None);
        client.drain_pending("u1").await;
        assert_eq!(
            bodies.lock().unwrap().len(),
            0,
            "no token: replaying would just rotate the queue through push_event's no-token path"
        );
        assert_eq!(
            tokens.pending_len("u1"),
            1,
            "queue stays intact until a token arrives"
        );
    }

    #[tokio::test]
    async fn drain_pending_with_open_breaker_requeues_without_http() {
        let (url, bodies) = spawn_events_stub(200).await;
        let breaker = Arc::new(TestBreaker::new(
            "mnemo.push",
            1,
            Duration::from_secs(60),
            None,
        ));
        assert!(breaker.allow());
        breaker.failure(); // threshold 1 → Open, 60s cooldown
        let tokens = Arc::new(MnemoTokenStore::new());
        tokens.store("u1", "tok".into());
        tokens.enqueue("u1", test_event("s-1"));
        let client = MnemoClient::with_base_url(url, "h", tokens.clone(), Some(breaker), None);
        client.drain_pending("u1").await;
        assert_eq!(
            bodies.lock().unwrap().len(),
            0,
            "open breaker must short-circuit the drain at zero HTTP cost"
        );
        assert_eq!(tokens.pending_len("u1"), 1);
    }

    #[tokio::test]
    async fn drain_pending_on_401_requeues_in_order_after_clearing_token() {
        // 401 is handled INSIDE push_event (clear token + enqueue +
        // Ok), so the drain loop keeps going; every later event then
        // takes the no-token path and lands behind it — order holds.
        let (url, bodies) = spawn_events_stub(401).await;
        let tokens = Arc::new(MnemoTokenStore::new());
        tokens.store("u1", jwt_with_payload(serde_json::json!({"exp": 1_i64})));
        tokens.enqueue("u1", test_event("s-1"));
        tokens.enqueue("u1", test_event("s-2"));
        let client = MnemoClient::with_base_url(url, "h", tokens.clone(), None, None);
        client.drain_pending("u1").await;
        assert_eq!(
            bodies.lock().unwrap().len(),
            1,
            "only the first event reaches HTTP"
        );
        assert!(
            tokens.get("u1").is_none(),
            "401 must clear the cached token"
        );
        let requeued = tokens.take_pending("u1");
        let ids: Vec<&str> = requeued.iter().map(|e| e.session_id.as_str()).collect();
        assert_eq!(ids, vec!["s-1", "s-2"]);
    }

    #[tokio::test]
    async fn drain_all_pending_covers_every_user_with_backlog() {
        let (url, bodies) = spawn_events_stub(200).await;
        let tokens = Arc::new(MnemoTokenStore::new());
        tokens.store("u1", "tok".into());
        tokens.store("u2", "tok".into());
        tokens.enqueue("u1", test_event("s-u1"));
        tokens.enqueue("u2", test_event("s-u2"));
        let client = MnemoClient::with_base_url(url, "h", tokens.clone(), None, None);
        crate::mnemo::drain_all_pending(&client).await;
        assert_eq!(tokens.pending_len("u1"), 0);
        assert_eq!(tokens.pending_len("u2"), 0);
        assert_eq!(bodies.lock().unwrap().len(), 2);
    }
}

#[cfg(test)]
mod breaker_drop_tests {
    //! Regression tests for improvement #16 at the mnemo layer:
    //! push/recall breaker outcome recording must survive the call's
    //! future being dropped mid-await. Today all mnemo callers run to
    //! completion, but the raw allow→await→record pattern was one
    //! `timeout()`/`select!` away from the same HalfOpen wedge the
    //! LLM gate had.

    use super::*;

    fn test_event() -> IngestEvent {
        IngestEvent {
            session_id: "sess-1".into(),
            source: "auris".into(),
            workstation: "test".into(),
            workdir: String::new(),
            project: None,
            turns: Vec::new(),
            attributes: std::collections::HashMap::new(),
        }
    }

    /// A TCP listener that is never `accept`ed: connects complete via
    /// the kernel backlog, the HTTP request is written, but no response
    /// ever arrives — so the reqwest future parks until dropped.
    fn hanging_listener() -> (std::net::TcpListener, String) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        (l, url)
    }

    /// Build an Enabled client pointed at `url`. Env var is set and
    /// removed inline; the suite runs `--test-threads=1`.
    fn enabled_client(
        url: &str,
        push_breaker: Option<Arc<CircuitBreaker>>,
        recall_breaker: Option<Arc<CircuitBreaker>>,
    ) -> (MnemoClient, Arc<MnemoTokenStore>) {
        std::env::set_var("AURIS_MNEMO_URL", url);
        let tokens = Arc::new(MnemoTokenStore::new());
        let client = MnemoClient::from_env(tokens.clone(), push_breaker, recall_breaker);
        std::env::remove_var("AURIS_MNEMO_URL");
        assert!(client.is_enabled(), "client must be enabled for this test");
        (client, tokens)
    }

    #[test]
    fn push_dropped_mid_await_records_breaker_failure() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (_listener, url) = hanging_listener();
            let cb = Arc::new(CircuitBreaker::new(
                "mnemo.push-test",
                1,
                Duration::from_millis(20),
                None,
            ));
            let (client, tokens) = enabled_client(&url, Some(cb.clone()), None);
            // Cache a token so push actually issues the HTTP call
            // (without one it queues the event and returns Ok).
            tokens.store("u1", "dummy-token".into());
            let event = test_event();

            // Drop the push future mid-await.
            let timed =
                tokio::time::timeout(Duration::from_millis(50), client.push_event("u1", &event))
                    .await;
            assert!(
                timed.is_err(),
                "push must still be awaiting the response when dropped"
            );

            // The drop must have recorded a failure (threshold 1 →
            // Open): the next push is rejected without touching the
            // network.
            let r = client.push_event("u1", &event).await;
            assert!(
                matches!(r, Err(MnemoError::CircuitOpen(_))),
                "breaker must be open after the dropped push, got {r:?}"
            );

            // Self-heal: a fresh probe is admitted after cooldown.
            tokio::time::sleep(Duration::from_millis(30)).await;
            assert!(cb.allow(), "fresh probe admitted after cooldown");
            cb.success();
        });
    }

    #[test]
    fn recall_dropped_mid_await_records_breaker_failure() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (_listener, url) = hanging_listener();
            let cb = Arc::new(CircuitBreaker::new(
                "mnemo.recall-test",
                1,
                Duration::from_millis(20),
                None,
            ));
            let (client, tokens) = enabled_client(&url, None, Some(cb.clone()));
            // Cache a token so recall actually issues the HTTP call
            // (without one it returns Ok(default) immediately).
            tokens.store("u1", "dummy-token".into());
            let params = RecallParams::for_meeting(None);

            let timed =
                tokio::time::timeout(Duration::from_millis(50), client.recall("u1", &params)).await;
            assert!(
                timed.is_err(),
                "recall must still be awaiting the response when dropped"
            );

            let r = client.recall("u1", &params).await;
            assert!(
                matches!(r, Err(MnemoError::CircuitOpen(_))),
                "breaker must be open after the dropped recall, got {r:?}"
            );

            tokio::time::sleep(Duration::from_millis(30)).await;
            assert!(cb.allow(), "fresh probe admitted after cooldown");
            cb.success();
        });
    }

    /// Guard wiring sanity: the Ok path must call succeed(). A push
    /// with no cached token queues the event and returns Ok without
    /// touching the network; with a threshold-1 breaker, a missing
    /// succeed() would open the breaker on the first call and turn the
    /// second into CircuitOpen.
    #[test]
    fn push_ok_path_records_success_not_failure() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let cb = Arc::new(CircuitBreaker::new(
                "mnemo.push-test2",
                1,
                Duration::from_millis(20),
                None,
            ));
            // URL is never contacted: no cached token → queue + Ok.
            let (client, _tokens) = enabled_client("http://127.0.0.1:1", Some(cb), None);
            let event = test_event();
            assert!(client.push_event("u1", &event).await.is_ok());
            assert!(
                client.push_event("u1", &event).await.is_ok(),
                "second Ok push must not be CircuitOpen — Ok path must record success"
            );
        });
    }
}
