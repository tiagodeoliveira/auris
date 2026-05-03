//! WebSocket server. See `docs/specs/server.md` §2.1, §6.3, §7.

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
    pub shutdown: CancellationToken,
    pub llm: Arc<LlmClient>,
}

pub async fn run_server(
    addr: SocketAddr,
    token: String,
    llm: Arc<LlmClient>,
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    let actual = listener.local_addr()?;
    info!(addr = ?actual, "listening");
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
    let handle = ServerHandle {
        state: Arc::new(Mutex::new(ServerState::new())),
        events_tx,
        token: Arc::new(token),
        meeting_cancel: Arc::new(StdMutex::new(None)),
        shutdown: shutdown.clone(),
        llm,
    };

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
    let cell_clone = Arc::clone(&token_cell);

    #[allow(clippy::result_large_err)]
    let ws = tokio_tungstenite::accept_hdr_async(stream, |req: &Request, response: Response| {
        let raw_path = req.uri().to_string();
        let token = parse_token_from_uri(&raw_path);
        *cell_clone.lock().unwrap() = token;
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

    info!(?peer, "connection accepted");

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
                        dispatch_intent(&t, &handle, &mut sink).await?;
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

    info!(?peer, "connection closed");
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
    if let Some(description) = outcome.start_extraction_for {
        let token = handle
            .meeting_cancel
            .lock()
            .unwrap()
            .as_ref()
            .map(|t| t.child_token());
        if let Some(t) = token {
            spawn_extraction(handle.clone(), description, t);
        }
    }
    if outcome.stopped_meeting || outcome.paused_meeting {
        let prev = handle.meeting_cancel.lock().unwrap().take();
        if let Some(t) = prev {
            t.cancel();
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

        // Re-acquire lock; abandon if meeting was stopped between extraction and lock.
        let event = {
            let mut s = handle.state.lock().await;
            if !matches!(
                s.snapshot_meeting_state(),
                crate::contract::MeetingState::Active | crate::contract::MeetingState::Paused
            ) {
                return;
            }
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
