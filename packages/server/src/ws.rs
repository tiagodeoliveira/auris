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

use crate::contract::{Event, Intent};
use crate::llm::LlmClient;
use crate::state::ServerState;

// WS close codes per RFC 6455. Tungstenite gave us named variants
// (`CloseCode::Policy` etc.); axum's `CloseFrame` takes raw `u16`.
// Centralised here so the literals don't sprinkle through the file.
// (Auth failures land as plain HTTP 401 before the upgrade — no
// 1008 close-frame path is needed.)
const CLOSE_GOING_AWAY: u16 = 1001;
const CLOSE_INTERNAL: u16 = 1011;

#[derive(Clone)]
pub struct ServerHandle {
    pub state: Arc<Mutex<ServerState>>,
    pub events_tx: broadcast::Sender<Event>,
    pub token: Arc<String>,
    pub meeting_cancel: Arc<StdMutex<Option<CancellationToken>>>,
    /// Cancels in-flight metadata extraction. Independent of meeting_cancel
    /// so idle-time extractions (Intent::ExtractMetadata) survive
    /// start_meeting and so we can cancel a stale extraction when the
    /// meeting is stopped.
    pub extraction_cancel: Arc<StdMutex<Option<CancellationToken>>>,
    pub shutdown: CancellationToken,
    pub llm: Arc<LlmClient>,
    /// Single `RemoteAudioSource` instance for the server lifetime.
    /// Meetings call `start()` against it to allocate a new
    /// downstream channel; `/audio` WS connections forward incoming
    /// PCM into the same channel via `current_sender()`.
    pub audio_source: Arc<crate::audio::RemoteAudioSource>,
    /// SQLite pool for meeting / moment persistence. See `db` module.
    /// Single connection pool is fine — the access pattern is
    /// "occasional small writes from intent handlers"; we're
    /// nowhere near needing read replicas or sharding.
    pub db: sqlx::SqlitePool,
    /// Internal broadcast: each moment created via the REST POST is
    /// published here. The async summary worker (spawned at boot)
    /// subscribes; nothing else does today. Held so api.rs can
    /// receive a clone via `ApiState`.
    pub moment_created_tx: broadcast::Sender<crate::api::MomentCreated>,
}

pub async fn run_server(
    addr: SocketAddr,
    token: String,
    llm: Arc<LlmClient>,
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(addr = ?listener.local_addr()?, "listening");
    run_server_with_listener(listener, token, llm, shutdown_rx).await
}

pub async fn run_server_with_listener(
    listener: TcpListener,
    token: String,
    llm: Arc<LlmClient>,
    mut shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let (events_tx, _) = broadcast::channel::<Event>(64);
    let shutdown = CancellationToken::new();
    let state = Arc::new(Mutex::new(ServerState::new()));

    // DB first — boot recovery wants to query meetings before any
    // subscribers spin up.
    let db = crate::db::open_pool().await?;

    // Boot recovery. If the previous run died with a meeting still
    // active (ended_at IS NULL in the meetings table), pick it up,
    // replay the persisted transcript items, and resume as if the
    // meeting had never been interrupted.
    let recovered = recover_active_meeting(&db, &state).await;

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
    let handle = ServerHandle {
        state,
        events_tx,
        token: Arc::new(token),
        meeting_cancel: Arc::new(StdMutex::new(None)),
        extraction_cancel: Arc::new(StdMutex::new(None)),
        shutdown: shutdown.clone(),
        llm,
        audio_source: Arc::new(crate::audio::RemoteAudioSource::new()),
        db,
        moment_created_tx,
    };

    // Async LLM worker that fills in moment summaries. Subscribes
    // to `moment_created_tx`; reads transcript JSONL ±N seconds
    // around each moment, prompts the LLM, writes summary back to
    // SQLite. See `summarizer/moment.rs`.
    crate::summarizer::moment::spawn_worker(handle.clone());

    // If we recovered an active meeting, fire a synthetic
    // `MeetingStateChanged Active` so the subscribers we just spawned
    // (mnemo, etc.) treat it as a started session. Then spin up the
    // live pipeline so STT + summarizers + audio are all running again.
    // The Mac's auto-reconnect will reconnect /audio into the new
    // RemoteAudioSource sender via late binding.
    if let Some(recovered_id) = recovered {
        let _ = handle.events_tx.send(Event::MeetingStateChanged {
            meeting_state: crate::contract::MeetingState::Active,
            meeting_id: Some(recovered_id.clone()),
        });

        let token = {
            let mut slot = handle.meeting_cancel.lock().unwrap();
            let t = CancellationToken::new();
            *slot = Some(t.clone());
            t
        };
        spawn_live_pipeline(handle.clone(), token.child_token()).await;
        info!(meeting_id = %recovered_id, "live pipeline restarted for recovered meeting");
    }

    // Periodic Status broadcast — keeps the wsStatus dot in PWA /
    // Mac UIs accurate even when there's no other event traffic.
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
            let status = {
                let s = hb_handle.state.lock().await;
                crate::contract::Status {
                    listening: matches!(
                        s.snapshot_meeting_state(),
                        crate::contract::MeetingState::Active
                    ),
                    paused: matches!(
                        s.snapshot_meeting_state(),
                        crate::contract::MeetingState::Paused
                    ),
                    error: None,
                }
            };
            let _ = hb_handle.events_tx.send(Event::Status { status });
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
        token: handle.token.clone(),
        moment_created_tx: handle.moment_created_tx.clone(),
    };
    let api_router = crate::api::make_router(api_state);
    let ws_router = Router::new()
        .route("/", get(ws_control_handler))
        .route("/audio", get(ws_audio_handler))
        .with_state(handle);
    api_router.merge(ws_router).layer(CorsLayer::permissive())
}

/// Auth params shared by both WS handlers — the token is in the
/// query string by convention (URLSessionWebSocketTask doesn't
/// expose custom headers ergonomically, and the PWA mirrors that).
#[derive(Debug, Deserialize)]
struct WsAuthParams {
    token: Option<String>,
}

/// 401 response for failed WS auth. We can't send a Close frame
/// before the upgrade completes, so a plain HTTP 401 is the right
/// way to reject — clients see the failed handshake and won't try
/// to read frames.
fn auth_failed(reason: &'static str) -> Response {
    (StatusCode::UNAUTHORIZED, reason).into_response()
}

async fn ws_control_handler(
    ws: WebSocketUpgrade,
    Query(auth): Query<WsAuthParams>,
    State(handle): State<ServerHandle>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    if !valid_token(auth.token.as_deref(), &handle.token) {
        warn!(
            ?peer,
            reason = if auth.token.is_some() {
                "mismatch"
            } else {
                "missing"
            },
            "auth failure (control)"
        );
        return auth_failed("invalid token");
    }
    ws.on_upgrade(move |socket| run_control_socket(socket, peer, handle))
}

async fn ws_audio_handler(
    ws: WebSocketUpgrade,
    Query(auth): Query<WsAuthParams>,
    State(handle): State<ServerHandle>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    if !valid_token(auth.token.as_deref(), &handle.token) {
        warn!(?peer, "auth failure (/audio)");
        return auth_failed("invalid token");
    }
    ws.on_upgrade(move |socket| run_audio_socket(socket, peer, handle))
}

fn valid_token(provided: Option<&str>, expected: &str) -> bool {
    match provided {
        Some(t) => constant_time_eq(t.as_bytes(), expected.as_bytes()),
        None => false,
    }
}

/// Run the control-plane WS loop on an upgraded `axum` socket.
/// Sends an initial snapshot, then forwards broadcast events to the
/// client and dispatches incoming intents until close or shutdown.
async fn run_control_socket(socket: WebSocket, peer: SocketAddr, handle: ServerHandle) {
    info!(?peer, "control connection accepted");

    // Per-connection ID. Used as the key for any device this
    // connection registers; on disconnect we remove the entry.
    let connection_id = uuid::Uuid::new_v4().to_string();
    let mut events_rx = handle.events_tx.subscribe();

    let snapshot = {
        let s = handle.state.lock().await;
        s.snapshot()
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
                    Ok(event) => {
                        let json = match serde_json::to_string(&event) {
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
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        if let Err(e) = dispatch_intent(&t, &handle, &connection_id, &mut sink).await {
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

    // Drop any device registered against this connection; broadcast.
    let removed = {
        let mut s = handle.state.lock().await;
        s.unregister_device(&connection_id)
    };
    if let Some(d) = removed {
        info!(?peer, device_id = %d.id, hostname = %d.hostname, "device unregistered on disconnect");
        let devices = {
            let s = handle.state.lock().await;
            s.devices_clone()
        };
        let _ = handle.events_tx.send(Event::DevicesChanged { devices });
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
async fn run_audio_socket(socket: WebSocket, peer: SocketAddr, handle: ServerHandle) {
    let remote = &handle.audio_source;
    info!(?peer, "/audio connection accepted");

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
async fn recover_active_meeting(
    db: &sqlx::SqlitePool,
    state: &Arc<Mutex<ServerState>>,
) -> Option<String> {
    // Test escape hatch: integration tests share a process and would
    // resurrect each other's leftover meetings without this gate.
    // Production never sets it.
    if std::env::var("MEETING_COMPANION_SKIP_BOOT_RECOVERY").is_ok() {
        return None;
    }
    let row = match crate::db::find_active_meeting(db).await {
        Ok(Some(r)) => r,
        Ok(None) => return None,
        Err(e) => {
            warn!(error = ?e, "find_active_meeting failed; skipping boot recovery");
            return None;
        }
    };
    let (id, description, metadata_json, started_at) = row;

    let metadata: HashMap<String, String> =
        serde_json::from_str(&metadata_json).unwrap_or_default();
    let transcript_items = crate::persistence::read_transcription(&id)
        .await
        .unwrap_or_default();

    info!(
        meeting_id = %id,
        items = transcript_items.len(),
        ?started_at,
        "recovering active meeting"
    );

    let recovered = crate::state::RecoveredMeeting {
        id: id.clone(),
        description,
        metadata,
        started_at,
        transcript_items,
    };
    state
        .lock()
        .await
        .rehydrate_from_recovered_meeting(&recovered);
    Some(id)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}

/// Sink alias used by the control-plane intent path. axum's
/// `WebSocket` is split into a `SplitSink<WebSocket, Message>`;
/// callers pass a `&mut` to that.
type WsSender = futures_util::stream::SplitSink<WebSocket, Message>;

async fn dispatch_intent(
    text: &str,
    handle: &ServerHandle,
    connection_id: &str,
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
            let device = s.register_device(connection_id.to_string(), hostname, capabilities);
            let all = s.devices_clone();
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
        // Broadcast to everyone else.
        let _ = handle.events_tx.send(Event::DevicesChanged {
            devices: all_devices,
        });
        return Ok(());
    }

    let outcome = {
        let mut s = handle.state.lock().await;
        s.apply_intent(intent)
    };

    if let Some(err_event) = outcome.error {
        let json = serde_json::to_string(&err_event)?;
        sink.send(Message::Text(json)).await.ok();
    }
    for event in outcome.events {
        let _ = handle.events_tx.send(event);
    }
    if outcome.started_meeting || outcome.resumed_meeting {
        let mut slot = handle.meeting_cancel.lock().unwrap();
        if let Some(prev) = slot.take() {
            prev.cancel();
        }
        *slot = Some(CancellationToken::new());
    }
    if outcome.started_meeting {
        let token = handle
            .meeting_cancel
            .lock()
            .unwrap()
            .as_ref()
            .map(|t| t.child_token());
        if let Some(t) = token {
            spawn_live_pipeline(handle.clone(), t).await;
        }
    }
    if let Some(description) = outcome.start_extraction_for {
        // Cancel any previous extraction; install a fresh token so this
        // request can be canceled by stop_meeting or by the next extract.
        let token = {
            let mut slot = handle.extraction_cancel.lock().unwrap();
            if let Some(prev) = slot.take() {
                prev.cancel();
            }
            let t = CancellationToken::new();
            *slot = Some(t.clone());
            t
        };
        spawn_extraction(handle.clone(), description, token);
    }
    if outcome.stopped_meeting || outcome.paused_meeting {
        let prev = handle.meeting_cancel.lock().unwrap().take();
        if let Some(t) = prev {
            t.cancel();
        }
    }
    if outcome.stopped_meeting {
        // Drop any in-flight extraction; otherwise its result would land
        // in the now-empty idle metadata as if from a fresh request.
        if let Some(t) = handle.extraction_cancel.lock().unwrap().take() {
            t.cancel();
        }
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
            rec.started_at,
            rec.description.as_deref(),
            &metadata_json,
        )
        .await
        {
            tracing::warn!(error = ?e, meeting_id = %rec.id, "insert_meeting failed");
        } else {
            tracing::info!(meeting_id = %rec.id, "meeting persisted");
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
                });
                // Delegate screenshot capture to the audio-source
                // device if it has `screen_capture`. We don't try
                // arbitrary other devices: the audio source is the
                // user's "active" Mac, so it's also the right
                // screenshot authority. If the source is e.g. a
                // PWA-only meeting, we skip — moment lands without an
                // image, which is the documented degraded path.
                let target = {
                    let s = handle.state.lock().await;
                    s.audio_source_device_id.as_ref().and_then(|id| {
                        s.devices_by_connection
                            .values()
                            .find(|d| {
                                d.id == *id
                                    && d.online
                                    && d.capabilities
                                        .contains(&crate::contract::Capability::ScreenCapture)
                            })
                            .map(|d| d.id.clone())
                    })
                };
                if let Some(target_device_id) = target {
                    let _ = handle.events_tx.send(Event::CaptureMomentScreenshot {
                        target_device_id,
                        meeting_id: req.meeting_id.clone(),
                        moment_id: moment_id.clone(),
                        t_ms: req.t as i64,
                    });
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
fn spawn_extraction(handle: ServerHandle, description: String, cancel: CancellationToken) {
    tokio::spawn(async move {
        // Dev escape hatch.
        if std::env::var("MEETING_COMPANION_LLM_DISABLED").is_ok() {
            tracing::info!("LLM extraction disabled by env var; skipping");
            return;
        }

        tracing::info!(
            provider = ?handle.llm.provider(),
            description_len = description.len(),
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
                    let status = crate::contract::Status {
                        listening: matches!(s.snapshot_meeting_state(), crate::contract::MeetingState::Active),
                        paused: matches!(s.snapshot_meeting_state(), crate::contract::MeetingState::Paused),
                        error: Some(short_error(&e)),
                    };
                    drop(s);
                    let _ = handle.events_tx.send(Event::Status { status });
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
            let manual = s.metadata_clone();
            let merged = crate::extraction::merge_manual_wins(extracted, &manual);
            s.set_metadata_full(merged.clone());
            Event::MetadataChanged { metadata: merged }
        };
        let _ = handle.events_tx.send(event);
    });
}

fn short_error(e: &crate::llm::ExtractionError) -> String {
    use crate::llm::ExtractionError::*;
    match e {
        Timeout(_) => "Metadata extraction timed out".to_string(),
        Extract(_) => "Metadata extraction failed".to_string(),
    }
}

async fn spawn_live_pipeline(handle: ServerHandle, cancel: CancellationToken) {
    let (chunk_tx, _) = tokio::sync::broadcast::channel::<crate::stt::TranscriptChunk>(64);

    // -------------------------------------------------------------------
    // Audio source. Allocates a downstream channel; the rx feeds STT,
    // the tx is parked on the `RemoteAudioSource` for `/audio` WS
    // handlers to forward into. `MEETING_COMPANION_AUDIO_DISABLED` is
    // a test/CI knob that runs the pipeline silent (no STT input).
    // -------------------------------------------------------------------
    let audio_disabled = std::env::var("MEETING_COMPANION_AUDIO_DISABLED").is_ok();
    let audio_rx = if audio_disabled {
        tracing::info!("audio capture disabled by env var");
        None
    } else {
        let audio_cancel = cancel.child_token();
        let rx = handle.audio_source.start(audio_cancel).await;
        tracing::info!("audio source started");
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
            tracing::info!(provider = provider.name(), "live pipeline STT spawning");
            tokio::spawn(provider.run(audio_rx, stt_tx, stt_events_tx, stt_cancel));
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
        tokio::spawn(async move {
            crate::summarizer::transcript::run_transcript_summarizer(
                task_state,
                task_rx,
                task_events,
                task_cancel,
            )
            .await;
        });
    }

    // Highlights summarizer
    {
        let task_state = Arc::clone(&handle.state);
        let task_llm = Arc::clone(&handle.llm);
        let task_events = handle.events_tx.clone();
        let task_cancel = cancel.child_token();
        let interval_ms: u64 = std::env::var("MEETING_COMPANION_HIGHLIGHTS_INTERVAL_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(crate::summarizer::highlights::HEARTBEAT_DEFAULT_MS);
        tokio::spawn(async move {
            crate::summarizer::highlights::run_highlights_summarizer(
                task_state,
                task_llm,
                task_events,
                task_cancel,
                Duration::from_millis(interval_ms),
            )
            .await;
        });
    }

    // Actions summarizer
    {
        let task_state = Arc::clone(&handle.state);
        let task_llm = Arc::clone(&handle.llm);
        let task_events = handle.events_tx.clone();
        let task_cancel = cancel.child_token();
        let interval_ms: u64 = std::env::var("MEETING_COMPANION_ACTIONS_INTERVAL_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(crate::summarizer::actions::HEARTBEAT_DEFAULT_MS);
        tokio::spawn(async move {
            crate::summarizer::actions::run_actions_summarizer(
                task_state,
                task_llm,
                task_events,
                task_cancel,
                Duration::from_millis(interval_ms),
            )
            .await;
        });
    }

    // Open-questions summarizer
    {
        let task_state = Arc::clone(&handle.state);
        let task_llm = Arc::clone(&handle.llm);
        let task_events = handle.events_tx.clone();
        let task_cancel = cancel.child_token();
        let interval_ms: u64 = std::env::var("MEETING_COMPANION_OPEN_QUESTIONS_INTERVAL_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(crate::summarizer::open_questions::HEARTBEAT_DEFAULT_MS);
        tokio::spawn(async move {
            crate::summarizer::open_questions::run_open_questions_summarizer(
                task_state,
                task_llm,
                task_events,
                task_cancel,
                Duration::from_millis(interval_ms),
            )
            .await;
        });
    }
}
