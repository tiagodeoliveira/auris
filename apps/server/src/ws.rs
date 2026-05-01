//! WebSocket server. See `docs/specs/server.md` §2.1, §6.3, §7.

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, oneshot, Mutex};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

use crate::contract::{Event, Intent};
use crate::state::ServerState;

#[derive(Clone)]
pub struct ServerHandle {
    pub state: Arc<Mutex<ServerState>>,
    pub events_tx: broadcast::Sender<Event>,
    pub token: Arc<String>,
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
                        dispatch_intent(&t, &handle, &mut sink, &peer).await?;
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
    peer: &SocketAddr,
) -> Result<()> {
    let intent: Intent = match serde_json::from_str(text) {
        Ok(i) => i,
        Err(_) => {
            // Protocol-error handling lands in Task 12.
            warn!(?peer, "bad inbound JSON; will be handled in Task 12");
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
    // Background task signals (started_meeting/stopped_meeting/etc.) handled in later tasks.
    Ok(())
}
