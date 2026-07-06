#![allow(dead_code)]

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;

pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct TestServer {
    pub addr: SocketAddr,
    /// Boot-time `ServerHandle` clone — lets tests reach internals
    /// like the finalize `TaskTracker` (`handle.tasks`).
    pub handle: auris_server::context::ServerHandle,
    /// Join handle for the `run_server_with_listener` task. Tests that
    /// assert on shutdown behavior `take()` this and await it after
    /// dropping the `TestServer`.
    pub join: Option<tokio::task::JoinHandle<()>>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

pub async fn spawn_test_server() -> TestServer {
    spawn_test_server_with_token("test-token").await
}

pub async fn spawn_test_server_with_token(_token: &str) -> TestServer {
    spawn_test_server_with_auth(auris_server::auth::AuthMode::Disabled).await
}

pub async fn spawn_test_server_with_auth(auth: auris_server::auth::AuthMode) -> TestServer {
    // Every legacy helper boots with recovery OFF — identical behavior
    // to before SpawnOpts existed.
    spawn_inner(auth, false).await
}

/// Knobs for `spawn_test_server_with_opts`. Every pre-existing helper
/// delegates with `boot_recovery: false` — identical behavior to the
/// old `spawn_test_server_with_auth`.
pub struct SpawnOpts {
    /// When `true`, UNSET `AURIS_SKIP_BOOT_RECOVERY` before boot so the
    /// server runs `recover_active_meetings` for real. Removal (not
    /// `=0`) is required: `config::flag()` is "set AND non-empty", so
    /// any value — including "0" — keeps the skip gate on. Only the
    /// dedicated `boot_recovery.rs` test binary turns this on; the env
    /// var is process-global, and that binary's tests serialize on a
    /// mutex so the flag state is explicit at each test's entry.
    pub boot_recovery: bool,
}

/// Auth-disabled spawn with an explicit boot-recovery toggle, for the
/// dedicated `boot_recovery.rs` binary. Auth mode is always `Disabled`
/// here — recovery tests don't exercise the JWT path.
pub async fn spawn_test_server_with_opts(opts: SpawnOpts) -> TestServer {
    spawn_inner(auris_server::auth::AuthMode::Disabled, opts.boot_recovery).await
}

async fn spawn_inner(auth: auris_server::auth::AuthMode, boot_recovery: bool) -> TestServer {
    // Load `.env` so DATABASE_URL etc. are visible to the test
    // process. The server binary loads this on its own from `main.rs`
    // via dotenvy; integration tests don't run through main, so they
    // have to opt in explicitly. Without it the WS handshake resets
    // on `connect` because db::open_pool() errors and tears down the
    // listener before the client gets to upgrade.
    let _ = dotenvy::dotenv();
    // NOTE: we deliberately do NOT set AURIS_AUTH_DISABLED here. That
    // env var is read only by main.rs, which integration tests never
    // execute — the AuthMode is passed straight into
    // run_server_with_listener below, so the enum value is the single
    // source of truth (Disabled for the legacy helpers, Live for
    // spawn_test_server_live).
    // Disable LLM extraction in tests by default. The actual extract path
    // never fires from spawn_extraction; the LlmClient is constructed only
    // because the run_server signature requires one.
    std::env::set_var("AURIS_LLM_DISABLED", "1");
    // Pool-specific provider vars are now required by from_env (no default
    // fallback). Set synthetic values so the client builds without real creds.
    if std::env::var("AURIS_LLM_BACKGROUND_PROVIDER").is_err() {
        std::env::set_var("AURIS_LLM_BACKGROUND_PROVIDER", "bedrock");
    }
    if std::env::var("AURIS_LLM_BACKGROUND_MODEL_ID").is_err() {
        std::env::set_var(
            "AURIS_LLM_BACKGROUND_MODEL_ID",
            "us.anthropic.claude-sonnet-4-7-20251015-v1:0",
        );
    }
    // Prevent the AWS credential chain from blocking on IMDS / SSO if the
    // dev machine has no real credentials configured.
    if std::env::var("AWS_ACCESS_KEY_ID").is_err() {
        std::env::set_var("AWS_ACCESS_KEY_ID", "test");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
        std::env::set_var("AWS_REGION", "us-west-2");
    }
    if boot_recovery {
        // Enable Phase C boot recovery for this boot. See SpawnOpts.
        // Removal (not "=0") is required — config::flag() is
        // set-AND-non-empty, so "0" would still skip.
        std::env::remove_var("AURIS_SKIP_BOOT_RECOVERY");
    } else {
        // Skip Phase C boot recovery in integration tests. Without this,
        // any previous test that left a meeting active (ended_at IS NULL)
        // would be resurrected on the next test's boot, polluting state.
        // Production servers always set this off (default).
        std::env::set_var("AURIS_SKIP_BOOT_RECOVERY", "1");
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (tx, rx) = oneshot::channel();

    // Pool-specific provider vars for the chat pool (mirrors background already set above).
    if std::env::var("AURIS_LLM_CHAT_PROVIDER").is_err() {
        std::env::set_var("AURIS_LLM_CHAT_PROVIDER", "bedrock");
    }
    if std::env::var("AURIS_LLM_CHAT_MODEL_ID").is_err() {
        std::env::set_var(
            "AURIS_LLM_CHAT_MODEL_ID",
            "us.anthropic.claude-sonnet-4-7-20251015-v1:0",
        );
    }

    let background_llm = std::sync::Arc::new(
        auris_server::llm::LlmClient::from_env(auris_server::llm::LlmPool::Background, None)
            .await
            .expect("background LLM client init in tests"),
    );
    let chat_llm = std::sync::Arc::new(
        auris_server::llm::LlmClient::from_env(auris_server::llm::LlmPool::Chat, None)
            .await
            .expect("chat LLM client init in tests"),
    );

    let breaker_metrics = std::sync::Arc::new(auris_server::observability::BreakerMetrics::new());
    let (handle_tx, handle_rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        let _ = auris_server::boot::run_server_with_listener(
            listener,
            auth,
            chat_llm,
            background_llm,
            breaker_metrics,
            rx,
            Some(handle_tx),
        )
        .await;
    });
    let handle = handle_rx
        .await
        .expect("server boot delivers a ServerHandle (did open_pool fail?)");
    TestServer {
        addr,
        handle,
        join: Some(join),
        shutdown: Some(tx),
    }
}

pub async fn connect(addr: SocketAddr, token: &str) -> Ws {
    let req = ws_url(addr, token);
    let (ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .expect("connect");
    ws
}

pub async fn next_event(ws: &mut Ws, timeout: Duration) -> Value {
    let msg = tokio::time::timeout(timeout, ws.next())
        .await
        .expect("timeout waiting for event")
        .expect("stream ended")
        .expect("ws error");
    let text = msg.to_text().expect("text frame").to_string();
    serde_json::from_str(&text).expect("json")
}

/// Like `next_event`, but returns `None` on timeout instead of
/// panicking. For "assert NOTHING arrives" shapes (e.g. the liveness
/// tests asserting no `meeting_state_changed: idle` inside a window).
pub async fn next_event_opt(ws: &mut Ws, timeout: Duration) -> Option<Value> {
    match tokio::time::timeout(timeout, ws.next()).await {
        Err(_) => None, // timed out — nothing arrived, which is fine
        Ok(msg) => {
            let msg = msg.expect("stream ended").expect("ws error");
            let text = msg.to_text().expect("text frame").to_string();
            Some(serde_json::from_str(&text).expect("json"))
        }
    }
}

pub async fn send_intent(ws: &mut Ws, intent: Value) {
    ws.send(Message::Text(intent.to_string()))
        .await
        .expect("send");
}

pub fn ws_url(addr: SocketAddr, token: &str) -> Request {
    let url = format!("ws://{}/?token={}", addr, token);
    url.into_client_request().expect("client request")
}

pub async fn spawn_test_server_fast_heartbeat() -> TestServer {
    std::env::set_var("AURIS_HEARTBEAT_MS", "300");
    let s = spawn_test_server().await;
    s
}

/// HS256 secret for Live-mode integration tests. >= 32 bytes as
/// required by AurisJwtIssuer::new. Test-only, grants nothing.
pub const TEST_HS256_SECRET: &[u8] = b"handshake-test-secret-0123456789abcdef";

pub struct LiveTestServer {
    pub server: TestServer,
    /// The same issuer instance the server's AuthMode holds — tests
    /// use it (via pairing::redeem_code) to mint device tokens the
    /// server will accept.
    pub issuer: auris_server::auth::pairing::AurisJwtIssuer,
}

/// Spawn the server in AuthMode::Live. The Auth0 validator points at
/// a dead `.invalid` domain: that is fine for these tests because a
/// garbage token fails `decode_header` before any JWKS fetch, and the
/// paired-device path never touches Auth0 at all. (Auth0-acceptance
/// is covered by the stub-backed unit tests in src/auth/validator.rs.)
pub async fn spawn_test_server_live() -> LiveTestServer {
    let issuer = auris_server::auth::pairing::AurisJwtIssuer::new(TEST_HS256_SECRET)
        .expect("test HS256 issuer");
    let auth0 = auris_server::auth::AuthValidator::new("test-tenant.invalid", "test-aud")
        .expect("test auth0 validator");
    let auth = auris_server::auth::AuthMode::Live {
        auth0,
        auris: issuer.clone(),
    };
    let server = spawn_test_server_with_auth(auth).await;
    LiveTestServer { server, issuer }
}
