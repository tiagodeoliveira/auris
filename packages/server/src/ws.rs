//! HTTP server: WebSocket (control + /audio) and REST (/meetings…)
//! on a single port via axum. The WS handlers do their own
//! query-string token check; REST handlers use bearer auth via
//! the `crate::api` module.

use anyhow::Result;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, oneshot, Mutex};
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tracing::{info, warn};

use crate::auth::AuthValidator;
use crate::contract::{Event, Intent, UserEvent};
use crate::llm::LlmClient;
use crate::state::ServerState;

/// Auth mode is decided at boot: either we validate JWTs against
/// Auth0, or we run with a synthetic dev user (env-flag bypass for
/// `websocat`/`curl` smoke testing without a browser flow).
pub enum AuthMode {
    /// `MEETING_COMPANION_AUTH_DISABLED=1` set. Every request is
    /// attributed to a fixed dev user (`auth0_sub = "dev|local"`).
    Disabled,
    /// Real Auth0 validation. Tokens must verify against the tenant's
    /// JWKS and target our API's audience.
    Live(AuthValidator),
}

/// Synthetic Auth0 sub used when the bypass flag is on. Has the same
/// shape as a real Auth0 sub (`<connection>|<id>`) so DB rows look
/// uniform.
pub const DEV_AUTH0_SUB: &str = "dev|local";

// WS close codes per RFC 6455. Tungstenite gave us named variants
// (`CloseCode::Policy` etc.); axum's `CloseFrame` takes raw `u16`.
// Centralised here so the literals don't sprinkle through the file.
// (Auth failures land as plain HTTP 401 before the upgrade — no
// 1008 close-frame path is needed.)
const CLOSE_GOING_AWAY: u16 = 1001;
const CLOSE_INTERNAL: u16 = 1011;

/// Per-connection event mailbox. Looked up by `connection_id` to
/// deliver targeted events without leaking them to every client.
type DirectMailbox = tokio::sync::mpsc::Sender<Event>;

/// Registry of live control connections keyed by `connection_id`.
/// Populated on accept, removed on disconnect.
pub type DirectRegistry = Arc<StdMutex<HashMap<String, DirectMailbox>>>;

#[derive(Clone)]
pub struct ServerHandle {
    pub state: Arc<Mutex<ServerState>>,
    pub events_tx: broadcast::Sender<UserEvent>,
    /// Per-connection senders for targeted events
    /// (currently `Event::CaptureMomentScreenshot`). The `events_tx`
    /// broadcast still handles everything-to-everyone traffic.
    pub direct_tx: DirectRegistry,
    /// Auth mode chosen at boot. `Arc` so it's cheap to clone into
    /// `ApiState` without paying for the `Disabled` enum variant.
    pub auth: Arc<AuthMode>,
    /// Per-user pipeline cancellation tokens. Each user's
    /// start_meeting installs a fresh token here; their stop_meeting
    /// cancels and removes it. Two users can have concurrent meetings
    /// without cross-cancelling each other.
    pub meeting_cancel: Arc<StdMutex<HashMap<String, CancellationToken>>>,
    /// Cancels in-flight metadata extraction, keyed per-user.
    /// Independent of `meeting_cancel` so idle-time extractions
    /// (Intent::ExtractMetadata) survive start_meeting; the entry for
    /// a given user is replaced when they kick a new extraction.
    pub extraction_cancel: Arc<StdMutex<HashMap<String, CancellationToken>>>,
    pub shutdown: CancellationToken,
    pub llm: Arc<LlmClient>,
    /// Per-user `RemoteAudioSource` instances. Each user's
    /// `start_meeting` lazily inserts (or reuses) their entry so
    /// concurrent meetings from different users don't cross-feed.
    /// `/audio` WS handlers route by their connection's `user_id`.
    pub audio_sources: Arc<StdMutex<HashMap<String, Arc<crate::audio::RemoteAudioSource>>>>,
    /// SQLite pool for meeting / moment persistence. See `db` module.
    /// Single connection pool is fine — the access pattern is
    /// "occasional small writes from intent handlers"; we're
    /// nowhere near needing read replicas or sharding.
    pub db: sqlx::PgPool,
    /// Internal broadcast: each moment created via the REST POST is
    /// published here. The async summary worker (spawned at boot)
    /// subscribes; nothing else does today. Held so api.rs can
    /// receive a clone via `ApiState`.
    pub moment_created_tx: broadcast::Sender<crate::api::MomentCreated>,
    /// Mirror channel for artifact uploads (`POST /artifacts`).
    /// Subscribed by the artifact-summary worker.
    pub artifact_created_tx: broadcast::Sender<crate::api::ArtifactCreated>,
    /// Kick the agent loop into firing immediately for a specific
    /// user. Sent by API handlers when something happens that the
    /// agent should react to without waiting for the next
    /// transcript-driven trigger (today: artifact attach to a
    /// running meeting). Subscribed by the agent task.
    pub agent_kick_tx: broadcast::Sender<crate::summarizer::agent::AgentKick>,
}

impl ServerHandle {
    /// Replace the cancellation token for `user_id`'s active meeting
    /// pipeline, cancelling any previous one. Returns the new token.
    pub fn meeting_cancel_for(&self, user_id: &str) -> CancellationToken {
        let mut map = self.meeting_cancel.lock().unwrap();
        if let Some(prev) = map.remove(user_id) {
            prev.cancel();
        }
        let t = CancellationToken::new();
        map.insert(user_id.to_string(), t.clone());
        t
    }

    /// Cancel + remove the active-meeting token for `user_id`.
    pub fn cancel_meeting_for(&self, user_id: &str) {
        if let Some(t) = self.meeting_cancel.lock().unwrap().remove(user_id) {
            t.cancel();
        }
    }

    /// Cancel + remove the extraction token for `user_id`.
    pub fn cancel_extraction_for(&self, user_id: &str) {
        if let Some(t) = self.extraction_cancel.lock().unwrap().remove(user_id) {
            t.cancel();
        }
    }

    /// Replace this user's extraction token with a fresh one,
    /// cancelling the previous (if any) so an in-flight extraction
    /// is dropped before the new one fires.
    pub fn extraction_cancel_for(&self, user_id: &str) -> CancellationToken {
        let mut map = self.extraction_cancel.lock().unwrap();
        if let Some(prev) = map.remove(user_id) {
            prev.cancel();
        }
        let t = CancellationToken::new();
        map.insert(user_id.to_string(), t.clone());
        t
    }

    /// Get or create the audio source for a user. Each user has
    /// their own `RemoteAudioSource` so concurrent meetings from
    /// different users have isolated PCM pipelines.
    pub fn audio_source_for(&self, user_id: &str) -> Arc<crate::audio::RemoteAudioSource> {
        let mut map = self.audio_sources.lock().unwrap();
        map.entry(user_id.to_string())
            .or_insert_with(|| Arc::new(crate::audio::RemoteAudioSource::new()))
            .clone()
    }
}

pub async fn run_server(
    addr: SocketAddr,
    auth: AuthMode,
    llm: Arc<LlmClient>,
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(addr = ?listener.local_addr()?, "listening");
    run_server_with_listener(listener, auth, llm, shutdown_rx).await
}

pub async fn run_server_with_listener(
    listener: TcpListener,
    auth: AuthMode,
    llm: Arc<LlmClient>,
    mut shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let (events_tx, _) = broadcast::channel::<UserEvent>(64);
    let shutdown = CancellationToken::new();
    let state = Arc::new(Mutex::new(ServerState::new()));

    // DB first — boot recovery wants to query meetings before any
    // subscribers spin up.
    let db = crate::db::open_pool().await?;

    // Boot recovery. If the previous run died with a meeting still
    // active (ended_at IS NULL in the meetings table), pick it up,
    // replay the persisted transcript items, and resume as if the
    // meeting had never been interrupted.
    let recovered = recover_active_meetings(&db, &state).await;

    // Memory layer: spin up the ingestion pusher and the start-of-meeting
    // recaller. Each gets its own broadcast subscription. No-op if mnemo
    // env vars are not set.
    crate::mnemo::spawn_tasks(
        crate::mnemo::MnemoClient::from_env(),
        state.clone(),
        &events_tx,
    );
    // Transcript persistence: writes one JSONL line per committed
    // transcript item to <DATA_DIR>/blobs/meetings/<id>/transcription.jsonl.
    // Other modes' items are not persisted (they're derived from
    // transcripts and can be re-run if ever needed).
    crate::persistence::spawn_task(state.clone(), &events_tx);
    let (moment_created_tx, _) = broadcast::channel::<crate::api::MomentCreated>(64);
    let (artifact_created_tx, _) = broadcast::channel::<crate::api::ArtifactCreated>(64);
    let (agent_kick_tx, _) = broadcast::channel::<crate::summarizer::agent::AgentKick>(32);
    let handle = ServerHandle {
        state,
        events_tx,
        direct_tx: Arc::new(StdMutex::new(HashMap::new())),
        auth: Arc::new(auth),
        meeting_cancel: Arc::new(StdMutex::new(HashMap::new())),
        extraction_cancel: Arc::new(StdMutex::new(HashMap::new())),
        shutdown: shutdown.clone(),
        llm,
        audio_sources: Arc::new(StdMutex::new(HashMap::new())),
        db,
        moment_created_tx,
        artifact_created_tx,
        agent_kick_tx,
    };

    // Async LLM worker that fills in moment summaries. Subscribes
    // to `moment_created_tx`; reads transcript JSONL ±N seconds
    // around each moment, prompts the LLM, writes summary back to
    // SQLite. See `summarizer/moment.rs`.
    crate::summarizer::moment::spawn_worker(handle.clone());

    // Sibling worker for artifact uploads. Subscribes to
    // `artifact_created_tx`; reads bytes off disk, asks the LLM for
    // short + long summaries in one call, writes them back into
    // `artifacts.short_summary` / `long_summary` and flips
    // `summary_status` to `done` (or `failed` on terminal error).
    // See `summarizer/artifact.rs`.
    crate::summarizer::artifact::spawn_worker(handle.clone());

    // For each recovered user-meeting pair: emit a synthetic
    // `MeetingStateChanged Active` (tagged to that user so only their
    // connections receive it) and spin up that user's live pipeline.
    for r in recovered {
        let _ = handle.events_tx.send(UserEvent::new(
            r.user_id.clone(),
            Event::MeetingStateChanged {
                meeting_state: crate::contract::MeetingState::Active,
                meeting_id: Some(r.meeting_id.clone()),
            },
        ));
        let token = handle.meeting_cancel_for(&r.user_id);
        spawn_live_pipeline(handle.clone(), r.user_id.clone(), token.child_token()).await;
        info!(
            user_id = %r.user_id,
            meeting_id = %r.meeting_id,
            "live pipeline restarted for recovered meeting"
        );
    }

    // Periodic Status broadcast — one event per connected user so
    // each only sees their own listening/paused state. Users without
    // an active meeting still receive an idle Status so the wsStatus
    // dot updates promptly.
    let hb_handle = handle.clone();
    let hb_shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hb_shutdown_clone = hb_shutdown.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(heartbeat_interval());
        interval.tick().await; // skip first immediate tick
        loop {
            interval.tick().await;
            if hb_shutdown_clone.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let snapshots = {
                let s = hb_handle.state.lock().await;
                s.user_ids()
                    .into_iter()
                    .map(|uid| {
                        let listening = s
                            .user(&uid)
                            .map(|u| {
                                matches!(u.meeting_state, crate::contract::MeetingState::Active)
                            })
                            .unwrap_or(false);
                        let paused = s
                            .user(&uid)
                            .map(|u| {
                                matches!(u.meeting_state, crate::contract::MeetingState::Paused)
                            })
                            .unwrap_or(false);
                        (uid, listening, paused)
                    })
                    .collect::<Vec<_>>()
            };
            for (uid, listening, paused) in snapshots {
                let _ = hb_handle.events_tx.send(UserEvent::new(
                    uid,
                    Event::Status {
                        status: crate::contract::Status {
                            listening,
                            paused,
                            error: None,
                        },
                    },
                ));
            }
        }
    });

    // Single axum app: WebSocket (/, /audio) + REST (/meetings…)
    // share one listener. Connection-info (peer address) is wired
    // via `into_make_service_with_connect_info`.
    let app = make_app_router(handle.clone());
    let serve_shutdown = shutdown.clone();
    let serve_handle = tokio::spawn(async move {
        let result = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move { serve_shutdown.cancelled().await })
        .await;
        if let Err(e) = result {
            tracing::warn!(error = ?e, "axum::serve stopped with error");
        }
    });

    // Park until the shutdown signal arrives, then unwind.
    let _ = (&mut shutdown_rx).await;
    info!("shutdown received");
    shutdown.cancel(); // signal all per-connection tasks to close
    hb_shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
    tokio::time::sleep(Duration::from_secs(2)).await; // 2 s drain
    serve_handle.abort();
    Ok(())
}

/// Build the unified Router: WS handlers at `/` and `/audio`, REST
/// handlers under `/meetings`. CORS-permissive so a future PWA hosted
/// on a different origin can fetch without server-side allowlisting.
fn make_app_router(handle: ServerHandle) -> Router {
    let api_state = crate::api::ApiState {
        db: handle.db.clone(),
        auth: handle.auth.clone(),
        moment_created_tx: handle.moment_created_tx.clone(),
        artifact_created_tx: handle.artifact_created_tx.clone(),
        agent_kick_tx: handle.agent_kick_tx.clone(),
    };
    let api_router = crate::api::make_router(api_state);
    let ws_router = Router::new()
        .route("/", get(ws_control_handler))
        .route("/audio", get(ws_audio_handler))
        .route("/stt", get(crate::stt_ws::ws_handler))
        .with_state(handle);
    api_router.merge(ws_router).layer(CorsLayer::permissive())
}

/// Auth params shared by all WS handlers — the token is in the
/// query string by convention (URLSessionWebSocketTask doesn't
/// expose custom headers ergonomically, and the PWA mirrors that).
#[derive(Debug, Deserialize)]
pub struct WsAuthParams {
    pub token: Option<String>,
}

/// 401 response for failed WS auth. We can't send a Close frame
/// before the upgrade completes, so a plain HTTP 401 is the right
/// way to reject — clients see the failed handshake and won't try
/// to read frames.
pub fn auth_failed_response(reason: &'static str) -> Response {
    (StatusCode::UNAUTHORIZED, reason).into_response()
}

/// Local alias for the in-file callers that still use the short name.
fn auth_failed(reason: &'static str) -> Response {
    auth_failed_response(reason)
}

async fn ws_control_handler(
    ws: WebSocketUpgrade,
    Query(auth): Query<WsAuthParams>,
    State(handle): State<ServerHandle>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    let user_id =
        match crate::auth::resolve_user_id(&handle.auth, &handle.db, auth.token.as_deref()).await {
            Ok(uid) => uid,
            Err(e) => {
                warn!(?peer, error = %e, "auth failure (control)");
                return auth_failed("invalid token");
            }
        };
    ws.on_upgrade(move |socket| run_control_socket(socket, peer, handle, user_id))
}

async fn ws_audio_handler(
    ws: WebSocketUpgrade,
    Query(auth): Query<WsAuthParams>,
    State(handle): State<ServerHandle>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    let user_id =
        match crate::auth::resolve_user_id(&handle.auth, &handle.db, auth.token.as_deref()).await {
            Ok(uid) => uid,
            Err(e) => {
                warn!(?peer, error = %e, "auth failure (/audio)");
                return auth_failed("invalid token");
            }
        };
    ws.on_upgrade(move |socket| run_audio_socket(socket, peer, handle, user_id))
}

/// Run the control-plane WS loop on an upgraded `axum` socket.
/// Sends an initial snapshot, then forwards broadcast events to the
/// client and dispatches incoming intents until close or shutdown.
/// `user_id` is the local `users.id` resolved from the request's
/// JWT (or the synthetic dev user when auth is disabled). It scopes
/// every DB write originating from this connection.
async fn run_control_socket(
    socket: WebSocket,
    peer: SocketAddr,
    handle: ServerHandle,
    user_id: String,
) {
    info!(?peer, user_id = %user_id, "control connection accepted");

    // Per-connection ID. Used as the key for any device this
    // connection registers; on disconnect we remove the entry.
    let connection_id = uuid::Uuid::new_v4().to_string();
    let mut events_rx = handle.events_tx.subscribe();

    // Per-connection mailbox for targeted events. Bounded — if the
    // client is so backed up we hit the cap, dropping the targeted
    // event is preferable to blocking the sender (a moment without a
    // screenshot is a softer failure than a stuck server).
    let (direct_mailbox_tx, mut direct_rx) = tokio::sync::mpsc::channel::<Event>(16);
    handle
        .direct_tx
        .lock()
        .unwrap()
        .insert(connection_id.clone(), direct_mailbox_tx);

    let snapshot = {
        let mut s = handle.state.lock().await;
        s.snapshot(&user_id)
    };

    let (mut sink, mut stream) = socket.split();
    let snapshot_json = match serde_json::to_string(&snapshot) {
        Ok(s) => s,
        Err(e) => {
            warn!(?peer, error = %e, "snapshot serialize failed");
            return;
        }
    };
    if sink.send(Message::Text(snapshot_json)).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            _ = handle.shutdown.cancelled() => {
                let _ = sink.send(Message::Close(Some(CloseFrame {
                    code: CLOSE_GOING_AWAY,
                    reason: "going away".into(),
                }))).await;
                break;
            }
            evt = events_rx.recv() => {
                match evt {
                    Ok(envelope) => {
                        // Per-user fan-out: drop events not addressed to
                        // this connection's user. The wire shape stays
                        // the same (just `Event`); only the broadcast
                        // bus carries the routing tag.
                        if envelope.user_id != user_id {
                            continue;
                        }
                        let json = match serde_json::to_string(&envelope.event) {
                            Ok(j) => j,
                            Err(e) => {
                                warn!(?peer, error = %e, "event serialize failed");
                                continue;
                            }
                        };
                        if sink.send(Message::Text(json)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(?peer, lagged = n, "client lagging — disconnecting");
                        let _ = sink.send(Message::Close(Some(CloseFrame {
                            code: CLOSE_INTERNAL,
                            reason: "client lagging".into(),
                        }))).await;
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            // Per-connection targeted events (e.g. CaptureMomentScreenshot).
            // Channel-closed = the registry dropped our sender, which only
            // happens during shutdown; treat it as a non-event.
            direct_evt = direct_rx.recv() => {
                if let Some(event) = direct_evt {
                    let json = match serde_json::to_string(&event) {
                        Ok(j) => j,
                        Err(e) => {
                            warn!(?peer, error = %e, "direct event serialize failed");
                            continue;
                        }
                    };
                    if sink.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        if let Err(e) = dispatch_intent(&t, &handle, &connection_id, &user_id, &mut sink).await {
                            warn!(?peer, error = %e, "dispatch_intent failed");
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}    // ignore binary, ping, pong
                    Some(Err(e)) => {
                        warn!(?peer, error = %e, "ws read error");
                        break;
                    }
                }
            }
        }
    }

    // Drop the targeted-event mailbox so future sends to this
    // connection just fall on the floor instead of routing nowhere.
    handle.direct_tx.lock().unwrap().remove(&connection_id);

    // Drop any device registered against this connection; broadcast
    // the resulting devices list to the *owning user's* connections
    // only. `unregister_connection` returns the user_id so we don't
    // leak this connection's removal to other users.
    let removed = {
        let mut s = handle.state.lock().await;
        s.unregister_connection(&connection_id)
    };
    if let Some((owner_uid, d)) = removed {
        info!(?peer, device_id = %d.id, hostname = %d.hostname, user_id = %owner_uid, "device unregistered on disconnect");
        let devices = handle.state.lock().await.devices_clone_for(&owner_uid);
        let _ = handle
            .events_tx
            .send(UserEvent::new(owner_uid, Event::DevicesChanged { devices }));
    }

    info!(?peer, "control connection closed");
}

/// Handles the `/audio` WebSocket. The client streams binary frames
/// of 16 kHz mono S16LE PCM (~640 bytes each); the handler forwards
/// each frame into the active meeting's audio sender (held by
/// `RemoteAudioSource`).
///
/// Late-binding semantics: the meeting can start before *or* after
/// the `/audio` connection, and a mid-meeting `/audio` reconnect
/// reuses the same downstream rx (the STT pipeline never sees the
/// connection churn). The handler caches the current sender locally
/// to avoid locking on every frame, and refreshes on `Closed` (rx
/// dropped — meeting ended) or `is_none` (no active meeting yet).
async fn run_audio_socket(
    socket: WebSocket,
    peer: SocketAddr,
    handle: ServerHandle,
    user_id: String,
) {
    // Per-user audio source — frames from this connection are
    // routed only into this user's STT pipeline. Cross-user PCM
    // bleed is structurally prevented.
    let remote = handle.audio_source_for(&user_id);
    info!(?peer, user_id = %user_id, "/audio connection accepted");

    let (mut sink, mut stream) = socket.split();
    let mut frames_received: u64 = 0;
    let mut bytes_received: u64 = 0;
    let mut frames_dropped_no_meeting: u64 = 0;
    // Cached sender for the active meeting. Refreshed lazily — on
    // first frame, on `Closed` errors (meeting just ended), and
    // each frame while there's no meeting yet (so we pick it up
    // promptly when one starts).
    let mut tx_cache: Option<tokio::sync::mpsc::Sender<Vec<u8>>> = remote.current_sender().await;

    loop {
        tokio::select! {
            _ = handle.shutdown.cancelled() => {
                let _ = sink.send(Message::Close(Some(CloseFrame {
                    code: CLOSE_GOING_AWAY,
                    reason: "going away".into(),
                }))).await;
                break;
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(bytes))) => {
                        frames_received += 1;
                        bytes_received += bytes.len() as u64;

                        // Refresh the cache when we don't have a sender;
                        // covers "/audio connected before start_meeting".
                        if tx_cache.is_none() {
                            tx_cache = remote.current_sender().await;
                        }

                        if let Some(tx) = &tx_cache {
                            // try_send drops on backpressure; the
                            // alternative (await send) would block PCM
                            // intake on STT lag.
                            match tx.try_send(bytes) {
                                Ok(()) => {}
                                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                    tracing::warn!(
                                        ?peer,
                                        frame = frames_received,
                                        "/audio: downstream backlogged — frame dropped"
                                    );
                                }
                                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                    // Meeting just ended (rx dropped).
                                    // Clear the cache; the next frame
                                    // will retry current_sender, which
                                    // self-cleans the slot to None.
                                    tx_cache = None;
                                }
                            }
                        } else {
                            frames_dropped_no_meeting += 1;
                            // First-time-only nudge so the operator
                            // sees /audio frames are arriving but
                            // there's nowhere to send them.
                            if frames_dropped_no_meeting == 1 {
                                tracing::warn!(
                                    ?peer,
                                    "/audio: frames arriving but no meeting active — dropping"
                                );
                            }
                        }

                        // Periodic ingest log so the operator can see
                        // frames are arriving even before/without a
                        // meeting consuming them.
                        if frames_received % 250 == 0 {
                            tracing::info!(
                                ?peer,
                                frames = frames_received,
                                bytes = bytes_received,
                                dropped_no_meeting = frames_dropped_no_meeting,
                                "/audio: ingest progress (~5 s of audio)"
                            );
                        }
                    }
                    Some(Ok(Message::Ping(p))) => {
                        let _ = sink.send(Message::Pong(p)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {
                        // Ignore text/other; only binary is meaningful here.
                    }
                    Some(Err(e)) => {
                        warn!(?peer, error = %e, "/audio: ws read error");
                        break;
                    }
                }
            }
        }
    }

    info!(
        ?peer,
        frames = frames_received,
        bytes = bytes_received,
        "/audio connection closed"
    );
}

/// On boot, look for a meeting whose `ended_at` is still NULL in the
/// `meetings` table — that's the previous run dying mid-meeting.
/// Replay its transcript from the per-meeting JSONL blob, mutate
/// `ServerState` so the next snapshot reflects an active meeting,
/// and return the recovered id (used by the caller to spawn the
/// live pipeline + emit a synthetic state-change event).
///
/// Returns `None` if there's nothing to recover or if loading fails.
/// Failures are logged but never propagate — boot recovery is
/// best-effort, the server should still come up even if the disk
/// is corrupted.
/// Each entry is `(user_id, meeting_id)` for one user that had an
/// unfinished meeting at server stop. Boot recovery hands these off
/// to the per-user pipeline-spawn path.
struct RecoveredUserMeeting {
    user_id: String,
    meeting_id: String,
}

async fn recover_active_meetings(
    db: &sqlx::PgPool,
    state: &Arc<Mutex<ServerState>>,
) -> Vec<RecoveredUserMeeting> {
    // Test escape hatch: integration tests share a process and would
    // resurrect each other's leftover meetings without this gate.
    if std::env::var("MEETING_COMPANION_SKIP_BOOT_RECOVERY").is_ok() {
        return Vec::new();
    }
    let rows = match crate::db::find_active_meetings_per_user(db).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = ?e, "find_active_meetings_per_user failed; skipping boot recovery");
            return Vec::new();
        }
    };

    let mut recovered = Vec::new();
    let mut seen_users = std::collections::HashSet::new();
    for (user_id, meeting_id, description, metadata_json, started_at) in rows {
        // One active meeting per user is the design invariant. If
        // the DB has stragglers (e.g., crash mid-stop), pick the
        // newest per user (rows are ordered DESC) and ignore older.
        if !seen_users.insert(user_id.clone()) {
            continue;
        }
        let metadata: HashMap<String, String> =
            serde_json::from_str(&metadata_json).unwrap_or_default();
        let transcript_items = crate::persistence::read_transcription(&meeting_id)
            .await
            .unwrap_or_default();
        info!(
            user_id = %user_id,
            meeting_id = %meeting_id,
            items = transcript_items.len(),
            ?started_at,
            "recovering active meeting"
        );
        let r = crate::state::RecoveredMeeting {
            id: meeting_id.clone(),
            description,
            metadata,
            started_at,
            transcript_items,
        };
        state
            .lock()
            .await
            .rehydrate_user_from_recovered(&user_id, &r);
        recovered.push(RecoveredUserMeeting {
            user_id,
            meeting_id,
        });
    }
    recovered
}

/// Sink alias used by the control-plane intent path. axum's
/// `WebSocket` is split into a `SplitSink<WebSocket, Message>`;
/// callers pass a `&mut` to that.
type WsSender = futures_util::stream::SplitSink<WebSocket, Message>;

async fn dispatch_intent(
    text: &str,
    handle: &ServerHandle,
    connection_id: &str,
    user_id: &str,
    sink: &mut WsSender,
) -> Result<()> {
    // 1. Parse as raw JSON object first to distinguish bad_json vs unknown_intent vs bad_payload.
    let raw: serde_json::Value = match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) if v.is_object() => v,
        _ => {
            send_protocol_error(sink, "bad_json", "frame is not a valid JSON object", None).await?;
            return Ok(());
        }
    };

    let ty: Option<String> = raw
        .get("type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned());
    let known_intents = [
        "start_meeting",
        "stop_meeting",
        "pause",
        "resume",
        "set_mode",
        "set_metadata",
        "extract_metadata",
        "register_device",
        "mark_moment",
        "expand_item",
    ];
    let Some(ty) = ty else {
        send_protocol_error(sink, "unknown_intent", "missing 'type' field", None).await?;
        return Ok(());
    };
    if !known_intents.contains(&ty.as_str()) {
        send_protocol_error(
            sink,
            "unknown_intent",
            &format!("unknown intent type '{}'", ty),
            Some(&ty),
        )
        .await?;
        return Ok(());
    }

    // 2. Parse as Intent strictly. Failure here = bad_payload.
    let intent: Intent = match serde_json::from_value(raw) {
        Ok(i) => i,
        Err(e) => {
            send_protocol_error(sink, "bad_payload", &format!("{}", e), Some(&ty)).await?;
            return Ok(());
        }
    };

    tracing::info!(intent = ?intent, "intent received");

    // RegisterDevice needs the connection_id (only ws.rs knows it),
    // so it's handled here rather than in `apply_intent`.
    if let Intent::RegisterDevice {
        hostname,
        capabilities,
    } = intent
    {
        let (device, all_devices) = {
            let mut s = handle.state.lock().await;
            let device =
                s.register_device(user_id, connection_id.to_string(), hostname, capabilities);
            let all = s.devices_clone_for(user_id);
            (device, all)
        };
        // Direct response to the registering client (so it learns its
        // own assigned device_id). Sent on the auth'd sink before any
        // broadcast lands, so the client never sees its own device in
        // a `DevicesChanged` before the `DeviceRegistered`.
        let registered = Event::DeviceRegistered { device };
        sink.send(Message::Text(serde_json::to_string(&registered)?))
            .await
            .ok();
        // Fan out the new devices list to *this user's* connections only.
        let _ = handle.events_tx.send(UserEvent::new(
            user_id.to_string(),
            Event::DevicesChanged {
                devices: all_devices,
            },
        ));
        return Ok(());
    }

    let outcome = {
        let mut s = handle.state.lock().await;
        s.apply_intent(user_id, intent)
    };

    if let Some(err_event) = outcome.error {
        let json = serde_json::to_string(&err_event)?;
        sink.send(Message::Text(json)).await.ok();
    }
    for event in outcome.events {
        let _ = handle
            .events_tx
            .send(UserEvent::new(user_id.to_string(), event));
    }
    if outcome.started_meeting {
        // Install a fresh per-user pipeline cancel token + spawn the
        // pipeline. `meeting_cancel_for` cancels any previous token
        // for this user (e.g., a stale resume that didn't tear down).
        let token = handle.meeting_cancel_for(user_id);
        spawn_live_pipeline(handle.clone(), user_id.to_string(), token.child_token()).await;
    } else if outcome.resumed_meeting {
        // Resume reuses the existing token; the meeting state machine
        // doesn't tear down the pipeline on pause/resume cycles.
    }
    if let Some(description) = outcome.start_extraction_for {
        // Cancel any previous extraction for *this* user; install
        // a fresh token. Cross-user extractions don't interfere.
        let token = handle.extraction_cancel_for(user_id);
        spawn_extraction(handle.clone(), user_id.to_string(), description, token);
    }
    if outcome.stopped_meeting || outcome.paused_meeting {
        handle.cancel_meeting_for(user_id);
    }
    if outcome.stopped_meeting {
        // Drop any in-flight extraction; otherwise its result would land
        // in the now-empty idle metadata as if from a fresh request.
        handle.cancel_extraction_for(user_id);
        // Per-meeting LLM usage summary. Drains the per-user counter so
        // the next meeting starts fresh. Char counts (not tokens) — see
        // `LlmUsage` for the rationale. Tracked across summarizers, the
        // moment worker, and (eventually) the agent loop.
        let usage = handle.llm.take_usage(user_id);
        tracing::info!(
            user_id,
            calls = usage.calls,
            prompt_chars = usage.prompt_chars,
            response_chars = usage.response_chars,
            "llm_usage_at_stop"
        );
    }

    // Persistence side-effects. None of these block the broadcast
    // path above — events have already gone out by the time we get
    // here. A DB hiccup logs a warning but doesn't fail the intent;
    // the meeting still proceeds in memory.
    if let Some(rec) = outcome.created_meeting {
        let metadata_json =
            serde_json::to_string(&rec.metadata).unwrap_or_else(|_| "{}".to_string());
        if let Err(e) = crate::db::insert_meeting(
            &handle.db,
            &rec.id,
            user_id,
            rec.started_at,
            rec.description.as_deref(),
            &metadata_json,
        )
        .await
        {
            tracing::warn!(error = ?e, meeting_id = %rec.id, "insert_meeting failed");
        } else {
            tracing::info!(meeting_id = %rec.id, user_id = %user_id, "meeting persisted");
        }
    }
    if let Some(rec) = outcome.closed_meeting {
        if let Err(e) = crate::db::end_meeting(&handle.db, &rec.id, rec.ended_at).await {
            tracing::warn!(error = ?e, meeting_id = %rec.id, "end_meeting failed");
        } else {
            tracing::info!(meeting_id = %rec.id, "meeting closed in db");
        }
    }
    if let Some(req) = outcome.mark_moment {
        let moment_id = uuid::Uuid::new_v4().to_string();
        let kind = "manual";
        match crate::db::insert_moment(
            &handle.db,
            &moment_id,
            &req.meeting_id,
            kind,
            req.t as i64,
            req.note.as_deref(),
            None,
        )
        .await
        {
            Ok(()) => {
                tracing::info!(
                    meeting_id = %req.meeting_id, moment_id = %moment_id, t = req.t,
                    "moment persisted"
                );
                // Wake the summary worker. Mirrors the REST path
                // (`api::create_moment`) — without this, WS-initiated
                // moments stay stuck on `summary_status='pending'`.
                let _ = handle.moment_created_tx.send(crate::api::MomentCreated {
                    meeting_id: req.meeting_id.clone(),
                    moment_id: moment_id.clone(),
                    kind: kind.to_string(),
                    t_ms: req.t as i64,
                    user_id: user_id.to_string(),
                });
                // Delegate screenshot capture to the audio-source
                // device if it has `screen_capture`. We don't try
                // arbitrary other devices: the audio source is the
                // user's "active" Mac, so it's also the right
                // screenshot authority. If the source is e.g. a
                // PWA-only meeting, we skip — moment lands without an
                // image, which is the documented degraded path.
                // Look up the connection_id of the target device so we
                // can deliver point-to-point via direct_tx instead of
                // broadcasting and asking every client to filter.
                let target_connection: Option<String> = {
                    let s = handle.state.lock().await;
                    s.user(user_id)
                        .and_then(|u| u.audio_source_device_id.as_ref().cloned())
                        .and_then(|device_id| {
                            s.user(user_id).and_then(|u| {
                                u.devices_by_connection
                                    .iter()
                                    .find(|(_, d)| {
                                        d.id == device_id
                                            && d.online
                                            && d.capabilities.contains(
                                                &crate::contract::Capability::ScreenCapture,
                                            )
                                    })
                                    .map(|(conn, _)| conn.clone())
                            })
                        })
                };
                if let Some(conn_id) = target_connection {
                    let event = Event::CaptureMomentScreenshot {
                        meeting_id: req.meeting_id.clone(),
                        moment_id: moment_id.clone(),
                        t_ms: req.t as i64,
                    };
                    let mailbox = handle.direct_tx.lock().unwrap().get(&conn_id).cloned();
                    if let Some(tx) = mailbox {
                        if let Err(e) = tx.try_send(event) {
                            tracing::warn!(
                                error = ?e, conn_id = %conn_id,
                                "capture_moment_screenshot mailbox full or closed"
                            );
                        }
                    }
                } else {
                    tracing::debug!(
                        moment_id = %moment_id,
                        "no screen_capture-capable audio source online; moment without screenshot"
                    );
                }
            }
            Err(e) => tracing::warn!(error = ?e, "insert_moment failed"),
        }
    }
    Ok(())
}

async fn send_protocol_error(
    sink: &mut WsSender,
    code: &str,
    message: &str,
    intent_ref: Option<&str>,
) -> Result<()> {
    let evt = Event::Error {
        code: code.into(),
        message: message.into(),
        intent_ref: intent_ref.map(|s| s.into()),
    };
    let json = serde_json::to_string(&evt)?;
    sink.send(Message::Text(json)).await.ok();
    Ok(())
}

fn heartbeat_interval() -> Duration {
    if let Ok(s) = std::env::var("MEETING_COMPANION_HEARTBEAT_MS") {
        if let Ok(ms) = s.parse::<u64>() {
            return Duration::from_millis(ms);
        }
    }
    Duration::from_secs(10)
}
fn spawn_extraction(
    handle: ServerHandle,
    user_id: String,
    description: String,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        // Dev escape hatch.
        if std::env::var("MEETING_COMPANION_LLM_DISABLED").is_ok() {
            tracing::info!("LLM extraction disabled by env var; skipping");
            return;
        }

        tracing::info!(
            provider = ?handle.llm.provider(),
            description_len = description.len(),
            user_id = %user_id,
            "metadata extraction starting"
        );
        let extracted = tokio::select! {
            result = handle.llm.extract(&description) => match result {
                Ok(map) => {
                    tracing::info!(field_count = map.len(), fields = ?map.keys().collect::<Vec<_>>(), "metadata extraction succeeded");
                    map
                }
                Err(e) => {
                    tracing::warn!(error = %e, "metadata extraction failed");
                    let s = handle.state.lock().await;
                    let user = s.user(&user_id);
                    let listening = user
                        .map(|u| matches!(u.meeting_state, crate::contract::MeetingState::Active))
                        .unwrap_or(false);
                    let paused = user
                        .map(|u| matches!(u.meeting_state, crate::contract::MeetingState::Paused))
                        .unwrap_or(false);
                    let status = crate::contract::Status {
                        listening,
                        paused,
                        error: Some(short_error(&e)),
                    };
                    drop(s);
                    let _ = handle
                        .events_tx
                        .send(UserEvent::new(user_id.clone(), Event::Status { status }));
                    return;
                }
            },
            _ = cancel.cancelled() => {
                tracing::debug!("extraction cancelled");
                return;
            }
        };

        // Re-acquire lock and merge. Idle is a valid state — extraction
        // may have been requested before the meeting was started.
        let event = {
            let mut s = handle.state.lock().await;
            let user = s.user_mut(&user_id);
            let manual = user.metadata_clone();
            let merged = crate::extraction::merge_manual_wins(extracted, &manual);
            user.set_metadata_full(merged.clone());
            Event::MetadataChanged { metadata: merged }
        };
        let _ = handle.events_tx.send(UserEvent::new(user_id, event));
    });
}

fn short_error(e: &crate::llm::ExtractionError) -> String {
    use crate::llm::ExtractionError::*;
    match e {
        Timeout(_) => "Metadata extraction timed out".to_string(),
        Extract(_) => "Metadata extraction failed".to_string(),
    }
}

async fn spawn_live_pipeline(handle: ServerHandle, user_id: String, cancel: CancellationToken) {
    let (chunk_tx, _) = tokio::sync::broadcast::channel::<crate::stt::TranscriptChunk>(64);

    // -------------------------------------------------------------------
    // Audio source — per-user. Allocates a downstream channel scoped
    // to this user; the rx feeds *their* STT, the tx is parked on
    // `RemoteAudioSource` so `/audio` WS handlers carrying the same
    // user_id can forward incoming PCM into it. Nothing here ever
    // crosses user boundaries.
    // -------------------------------------------------------------------
    let audio_disabled = std::env::var("MEETING_COMPANION_AUDIO_DISABLED").is_ok();
    let audio_rx = if audio_disabled {
        tracing::info!("audio capture disabled by env var");
        None
    } else {
        let audio_cancel = cancel.child_token();
        let user_audio = handle.audio_source_for(&user_id);
        let rx = user_audio.start(audio_cancel).await;
        tracing::info!(user_id = %user_id, "audio source started");
        Some(rx)
    };

    // -------------------------------------------------------------------
    // STT task — dispatch via trait so future providers slot in cleanly.
    // -------------------------------------------------------------------
    let provider_name = std::env::var("MEETING_COMPANION_STT_PROVIDER")
        .or_else(|_| {
            if std::env::var("MEETING_COMPANION_STT_MOCK").is_ok() {
                Ok("mock".to_string())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        })
        .unwrap_or_else(|_| "soniox".to_string());

    match crate::stt::make_provider(&provider_name) {
        Ok(provider) => {
            let stt_cancel = cancel.child_token();
            let stt_tx = chunk_tx.clone();
            let stt_events_tx = handle.events_tx.clone();
            let stt_uid = user_id.clone();
            tracing::info!(provider = provider.name(), user_id = %user_id, "live pipeline STT spawning");
            tokio::spawn(provider.run(audio_rx, stt_tx, stt_events_tx, stt_uid, stt_cancel));
        }
        Err(e) => {
            tracing::error!(error = %e, provider = %provider_name, "STT provider init failed; meeting will run without transcription");
        }
    }

    // Transcript summarizer (no LLM)
    {
        let task_state = Arc::clone(&handle.state);
        let task_events = handle.events_tx.clone();
        let task_rx = chunk_tx.subscribe();
        let task_cancel = cancel.child_token();
        let task_uid = user_id.clone();
        tokio::spawn(async move {
            crate::summarizer::transcript::run_transcript_summarizer(
                task_state,
                task_rx,
                task_events,
                task_uid,
                task_cancel,
            )
            .await;
        });
    }

    // Single agent task replaces the three per-mode summarizers.
    // It reasons over each batch of new transcript chunks and
    // pushes items into highlights/actions/open_questions via
    // tool calls.
    crate::summarizer::agent::spawn_meeting_agent(
        Arc::clone(&handle.state),
        handle.db.clone(),
        chunk_tx.subscribe(),
        handle.agent_kick_tx.subscribe(),
        handle.events_tx.clone(),
        user_id.clone(),
        Arc::clone(&handle.llm),
        cancel.child_token(),
    );

    // Conversation summary task — single-item Replace mode driven
    // by token-threshold + hard-ceiling triggers. Re-summarizes the
    // full transcript on each fire (separate concern from the agent;
    // see summarizer/summary.rs).
    {
        let task_state = Arc::clone(&handle.state);
        let task_llm = Arc::clone(&handle.llm);
        let task_events = handle.events_tx.clone();
        let task_uid = user_id.clone();
        let task_cancel = cancel.child_token();
        tokio::spawn(async move {
            crate::summarizer::summary::run_summary_summarizer(
                task_state,
                task_llm,
                task_events,
                task_uid,
                task_cancel,
            )
            .await;
        });
    }
}
