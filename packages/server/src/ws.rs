//! WebSocket server.

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, oneshot, Mutex};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::contract::{Event, Intent};
use crate::llm::LlmClient;
use crate::state::ServerState;

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
}

pub async fn run_server(
    ws_addr: SocketAddr,
    api_addr: SocketAddr,
    token: String,
    llm: Arc<LlmClient>,
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let ws_listener = TcpListener::bind(ws_addr).await?;
    let api_listener = TcpListener::bind(api_addr).await?;
    info!(ws = ?ws_listener.local_addr()?, api = ?api_listener.local_addr()?, "listening");
    run_server_with_listener(ws_listener, Some(api_listener), token, llm, shutdown_rx).await
}

pub async fn run_server_with_listener(
    listener: TcpListener,
    api_listener: Option<TcpListener>,
    token: String,
    llm: Arc<LlmClient>,
    mut shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let (events_tx, _) = broadcast::channel::<Event>(64);
    let shutdown = CancellationToken::new();
    let state = Arc::new(Mutex::new(ServerState::new()));
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
    let db = crate::db::open_pool().await?;
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
    };

    // REST API for browsing past meetings. Lives on a separate
    // listener (typically `ws_port + 1`); shares this handle's db
    // pool and token. Production / docker exposes both ports.
    if let Some(api_listener) = api_listener {
        let api_state = crate::api::ApiState {
            db: handle.db.clone(),
            token: handle.token.clone(),
        };
        let router = crate::api::make_router(api_state);
        let api_shutdown = shutdown.clone();
        let local = api_listener.local_addr().ok();
        tokio::spawn(async move {
            if let Some(addr) = local {
                info!(?addr, "api listening");
            }
            let server = axum::serve(api_listener, router)
                .with_graceful_shutdown(async move { api_shutdown.cancelled().await });
            if let Err(e) = server.await {
                tracing::warn!(error = ?e, "api server stopped with error");
            }
        });
    }

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

    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok((stream, peer)) => {
                        let h = handle.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, peer, h).await {
                                warn!(?peer, error = %e, "connection ended with error");
                            }
                        });
                    }
                    Err(e) => warn!(error = %e, "accept error"),
                }
            }
            _ = &mut shutdown_rx => {
                info!("shutdown received");
                break;
            }
        }
    }
    shutdown.cancel(); // signal all per-connection tasks to close
    hb_shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
    tokio::time::sleep(Duration::from_secs(2)).await; // give connections 2s to drain
    Ok(())
}

async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    handle: ServerHandle,
) -> Result<()> {
    let token_cell = Arc::new(std::sync::Mutex::new(None::<String>));
    let path_cell = Arc::new(std::sync::Mutex::new(String::new()));
    let token_clone = Arc::clone(&token_cell);
    let path_clone = Arc::clone(&path_cell);

    #[allow(clippy::result_large_err)]
    let ws = tokio_tungstenite::accept_hdr_async(stream, |req: &Request, response: Response| {
        let raw_path = req.uri().to_string();
        *token_clone.lock().unwrap() = parse_token_from_uri(&raw_path);
        *path_clone.lock().unwrap() = req.uri().path().to_string();
        Ok(response)
    })
    .await?;

    let provided = token_cell.lock().unwrap().clone();
    let valid = match provided.as_deref() {
        Some(t) => constant_time_eq(t.as_bytes(), handle.token.as_bytes()),
        None => false,
    };

    if !valid {
        warn!(
            ?peer,
            reason = if provided.is_some() {
                "mismatch"
            } else {
                "missing"
            },
            "auth failure"
        );
        let mut ws = ws;
        let _ = ws
            .send(Message::Close(Some(CloseFrame {
                code: CloseCode::Policy,
                reason: "invalid token".into(),
            })))
            .await;
        return Ok(());
    }

    let path = path_cell.lock().unwrap().clone();
    info!(?peer, path = %path, "connection accepted");

    // Dispatch by path. /audio is the binary-PCM intake from the
    // RemoteAudioSource (Mac app or wscat). Everything else uses the
    // PWA's intent/event protocol.
    if path == "/audio" {
        return handle_audio_connection(ws, peer, handle).await;
    }

    // Per-connection ID. Used as the key for any device this
    // connection registers; on disconnect we remove the entry.
    let connection_id = uuid::Uuid::new_v4().to_string();

    let mut events_rx = handle.events_tx.subscribe();
    let snapshot = {
        let s = handle.state.lock().await;
        s.snapshot()
    };

    let (mut sink, mut stream) = ws.split();
    sink.send(Message::Text(serde_json::to_string(&snapshot)?))
        .await?;

    loop {
        tokio::select! {
            _ = handle.shutdown.cancelled() => {
                let _ = sink.send(Message::Close(Some(CloseFrame {
                    code: CloseCode::Away,
                    reason: "going away".into(),
                }))).await;
                break;
            }
            evt = events_rx.recv() => {
                match evt {
                    Ok(event) => {
                        let json = serde_json::to_string(&event)?;
                        if sink.send(Message::Text(json)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(?peer, lagged = n, "client lagging — disconnecting");
                        let _ = sink.send(Message::Close(Some(CloseFrame {
                            code: CloseCode::Error,
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
                        dispatch_intent(&t, &handle, &connection_id, &mut sink).await?;
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

    info!(?peer, "connection closed");
    Ok(())
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
async fn handle_audio_connection(
    ws: tokio_tungstenite::WebSocketStream<TcpStream>,
    peer: SocketAddr,
    handle: ServerHandle,
) -> Result<()> {
    let remote = &handle.audio_source;

    info!(?peer, "/audio connection accepted");

    let (mut sink, mut stream) = ws.split();
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
                    code: CloseCode::Away,
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
    Ok(())
}

fn parse_token_from_uri(raw: &str) -> Option<String> {
    let uri: Uri = raw.parse().ok()?;
    let q = uri.query()?;
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == "token" {
            return Some(v.to_string());
        }
    }
    None
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}

async fn dispatch_intent(
    text: &str,
    handle: &ServerHandle,
    connection_id: &str,
    sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<TcpStream>,
        Message,
    >,
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
        match crate::db::insert_moment(
            &handle.db,
            &req.meeting_id,
            req.t as i64,
            req.note.as_deref(),
        )
        .await
        {
            Ok(moment_id) => tracing::info!(
                meeting_id = %req.meeting_id, moment_id = %moment_id, t = req.t,
                "moment persisted"
            ),
            Err(e) => tracing::warn!(error = ?e, "insert_moment failed"),
        }
    }
    Ok(())
}

async fn send_protocol_error(
    sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<TcpStream>,
        Message,
    >,
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
