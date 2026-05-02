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
use crate::extraction;
use crate::mock::make_item;
use crate::state::ServerState;

#[derive(Clone)]
pub struct ServerHandle {
    pub state: Arc<Mutex<ServerState>>,
    pub events_tx: broadcast::Sender<Event>,
    pub token: Arc<String>,
    pub meeting_cancel: Arc<StdMutex<Option<CancellationToken>>>,
}

pub async fn run_server(addr: SocketAddr, token: String, shutdown_rx: oneshot::Receiver<()>) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    let actual = listener.local_addr()?;
    info!(addr = ?actual, "listening");
    run_server_with_listener(listener, token, shutdown_rx).await
}

pub async fn run_server_with_listener(
    listener: TcpListener,
    token: String,
    mut shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let (events_tx, _) = broadcast::channel::<Event>(64);
    let handle = ServerHandle {
        state: Arc::new(Mutex::new(ServerState::new())),
        events_tx,
        token: Arc::new(token),
        meeting_cancel: Arc::new(StdMutex::new(None)),
    };

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
        warn!(?peer, reason = if provided.is_some() { "mismatch" } else { "missing" }, "auth failure");
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
    sink.send(Message::Text(serde_json::to_string(&snapshot)?)).await?;

    loop {
        tokio::select! {
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

    let ty: Option<String> = raw.get("type").and_then(|v| v.as_str()).map(|s| s.to_owned());
    let known_intents = [
        "start_meeting", "stop_meeting", "pause", "resume",
        "set_mode", "set_metadata", "mark_moment", "expand_item",
    ];
    let Some(ty) = ty else {
        send_protocol_error(sink, "unknown_intent", "missing 'type' field", None).await?;
        return Ok(());
    };
    if !known_intents.contains(&ty.as_str()) {
        send_protocol_error(sink, "unknown_intent", &format!("unknown intent type '{}'", ty), Some(&ty)).await?;
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
        let token = CancellationToken::new();
        {
            let mut slot = handle.meeting_cancel.lock().unwrap();
            if let Some(prev) = slot.take() {
                prev.cancel();
            }
            *slot = Some(token.clone());
        }
        spawn_mock_generator(handle.clone(), token);
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

const MOCK_INTERVAL: Duration = Duration::from_secs(3);
const EXTRACTION_DELAY: Duration = Duration::from_millis(1500);

fn spawn_extraction(handle: ServerHandle, description: String, cancel: CancellationToken) {
    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep(EXTRACTION_DELAY) => {
                let extracted = extraction::extract_metadata(&description);
                let event = {
                    let mut s = handle.state.lock().await;
                    if !matches!(s.snapshot_meeting_state(), crate::contract::MeetingState::Active | crate::contract::MeetingState::Paused) {
                        return;
                    }
                    let manual = s.metadata_clone();
                    let merged = extraction::merge_manual_wins(extracted, &manual);
                    s.set_metadata_full(merged.clone());
                    Event::MetadataChanged { metadata: merged }
                };
                let _ = handle.events_tx.send(event);
            }
            _ = cancel.cancelled() => {}
        }
    });
}

pub fn spawn_mock_generator(handle: ServerHandle, cancel: CancellationToken) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(MOCK_INTERVAL);
        interval.tick().await;   // discard the immediate tick
        let mut idx: usize = 0;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let event = {
                        let mut s = handle.state.lock().await;
                        let started_at = match s.meeting_started_at() {
                            Some(t) => t,
                            None => break,
                        };
                        let mode_id = s.current_mode_id().to_string();
                        let item = make_item(&mode_id, idx, started_at);
                        let payload = s.push_mock_item(item);
                        Event::ItemsUpdate { items: payload }
                    };
                    let _ = handle.events_tx.send(event);
                    idx += 1;
                }
                _ = cancel.cancelled() => break,
            }
        }
    });
}
