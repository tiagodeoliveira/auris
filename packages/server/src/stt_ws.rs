//! `/stt` — one-shot dictation WebSocket.
//!
//! Each connection opens a fresh STT session, isolated from any
//! meeting pipeline that may be running. Wire shape:
//!
//!  Client → Server
//!    - Binary frames: 16 kHz mono S16LE PCM (~640 bytes per 20 ms).
//!    - Text frames (optional): JSON `{"type":"stop"}` to close
//!      gracefully. A plain WS Close frame works too.
//!
//!  Server → Client (text frames, JSON, snake_case)
//!    - `{"type":"ready"}` once the upstream provider opens.
//!    - `{"type":"interim","text":"..."}` running preview.
//!    - `{"type":"final","text":"...","t_start_ms":N,"t_end_ms":M}` flushed utterance.
//!    - `{"type":"error","code":"...","message":"..."}` non-fatal.
//!
//! Auth is the same JWT-in-querystring used by `/` and `/audio`.
//! User identity is resolved from the token (so we can attribute
//! usage in logs / future quotas), but unlike the meeting pipeline
//! the dictation session does not touch shared per-user state.

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use std::net::SocketAddr;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::contract::{Event, UserEvent};
use crate::stt::TranscriptChunk;
use crate::ws::{auth_failed_response, ServerHandle, WsAuthParams};

/// Server → client wire frame. Tagged JSON so the client can
/// `switch (msg.type)` directly. Snake_case so it matches the rest
/// of the protocol.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SttServerMessage<'a> {
    Ready,
    Interim {
        text: &'a str,
    },
    Final {
        text: &'a str,
        t_start_ms: u64,
        t_end_ms: u64,
    },
    Error {
        code: &'a str,
        message: &'a str,
    },
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(auth): Query<WsAuthParams>,
    State(handle): State<ServerHandle>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
) -> Response {
    let user_id =
        match crate::auth::resolve_user_id(&handle.auth, &handle.db, auth.token.as_deref()).await {
            Ok(uid) => uid,
            Err(e) => {
                warn!(?peer, error = %e, "auth failure (/stt)");
                return auth_failed_response("invalid token");
            }
        };
    ws.on_upgrade(move |socket| run_stt_socket(socket, peer, handle, user_id))
}

async fn run_stt_socket(
    socket: WebSocket,
    peer: SocketAddr,
    handle: ServerHandle,
    user_id: String,
) {
    info!(?peer, user_id = %user_id, "/stt connection accepted");

    // Pick the configured provider. If init fails (e.g., missing
    // SONIOX_API_KEY), tell the client and bail — there's nothing
    // we can do server-side to recover.
    let provider_name = std::env::var("MEETING_COMPANION_STT_PROVIDER")
        .or_else(|_| {
            if crate::env::flag("MEETING_COMPANION_STT_MOCK") {
                Ok("mock".to_string())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        })
        .unwrap_or_else(|_| "soniox".to_string());

    let (mut sink, mut stream) = socket.split();

    let provider = match crate::stt::make_provider(&provider_name) {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("STT provider init failed: {e}");
            warn!(?peer, error = %msg, "/stt provider init");
            let _ = send_json(
                &mut sink,
                &SttServerMessage::Error {
                    code: "stt_unavailable",
                    message: &msg,
                },
            )
            .await;
            let _ = sink.send(Message::Close(None)).await;
            return;
        }
    };

    // Per-session channels. Provider drains audio_rx, emits
    // TranscriptChunks on chunk_tx, status/interim on events_tx.
    // None of these are shared with the meeting bus.
    let (audio_tx, audio_rx) = mpsc::channel::<Vec<u8>>(64);
    let (chunk_tx, mut chunk_rx) = broadcast::channel::<TranscriptChunk>(64);
    let (events_tx, mut events_rx) = broadcast::channel::<UserEvent>(64);

    let cancel = CancellationToken::new();
    let provider_cancel = cancel.child_token();
    let provider_uid = user_id.clone();
    let provider_task = tokio::spawn(provider.run(
        Some(audio_rx),
        chunk_tx,
        events_tx,
        provider_uid,
        provider_cancel,
    ));

    // Tell the client we're ready. Doesn't wait for the upstream
    // session to actually open — providers handle reconnects
    // internally and surface errors via Status events.
    if (send_json(&mut sink, &SttServerMessage::Ready).await).is_err() {
        cancel.cancel();
        let _ = provider_task.await;
        return;
    }

    let mut frames_received: u64 = 0;
    let mut closed_by_client = false;

    loop {
        tokio::select! {
            _ = handle.shutdown.cancelled() => {
                break;
            }
            // Final flushed utterance from the provider.
            chunk = chunk_rx.recv() => {
                match chunk {
                    Ok(c) => {
                        if send_json(
                            &mut sink,
                            &SttServerMessage::Final {
                                text: &c.text,
                                t_start_ms: c.t_start_ms,
                                t_end_ms: c.t_end_ms,
                            },
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        // Provider exited; tell the client and close.
                        let _ = send_json(
                            &mut sink,
                            &SttServerMessage::Error {
                                code: "stt_closed",
                                message: "provider closed",
                            },
                        )
                        .await;
                        break;
                    }
                }
            }
            // Interim previews + status errors from the provider.
            evt = events_rx.recv() => {
                match evt {
                    Ok(envelope) => {
                        // Provider emits with the user_id we passed in;
                        // belt-and-suspenders filter just in case.
                        if envelope.user_id != user_id {
                            continue;
                        }
                        match envelope.event {
                            Event::TranscriptInterim { text } => {
                                if send_json(&mut sink, &SttServerMessage::Interim { text: &text })
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Event::Status { status } => {
                                if let Some(code) = status.error.as_deref() {
                                    if send_json(
                                        &mut sink,
                                        &SttServerMessage::Error {
                                            code,
                                            message: code,
                                        },
                                    )
                                    .await
                                    .is_err()
                                    {
                                        break;
                                    }
                                }
                            }
                            _ => {} // ignore meeting-only events
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(bytes))) => {
                        frames_received += 1;
                        if let Err(e) = audio_tx.send(bytes).await {
                            warn!(?peer, error = %e, "/stt: audio_tx send failed");
                            break;
                        }
                    }
                    Some(Ok(Message::Text(t))) => {
                        // Only "stop" is recognised today; ignore others
                        // so the wire format can grow without breaking
                        // older clients.
                        if t.contains("\"stop\"") {
                            closed_by_client = true;
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        closed_by_client = true;
                        break;
                    }
                    Some(Ok(_)) => {} // ignore Ping/Pong/Frame
                    Some(Err(e)) => {
                        warn!(?peer, error = %e, "/stt ws read error");
                        break;
                    }
                }
            }
        }
    }

    // Cancel the provider; drop audio_tx so its rx returns None and
    // the provider drains cleanly. Wait briefly so any final flushed
    // chunk lands before the WS shuts.
    cancel.cancel();
    drop(audio_tx);
    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), provider_task).await;

    // Drain any final chunks that arrived during shutdown.
    while let Ok(chunk) = chunk_rx.try_recv() {
        let _ = send_json(
            &mut sink,
            &SttServerMessage::Final {
                text: &chunk.text,
                t_start_ms: chunk.t_start_ms,
                t_end_ms: chunk.t_end_ms,
            },
        )
        .await;
    }

    let _ = sink.send(Message::Close(None)).await;

    info!(
        ?peer,
        frames = frames_received,
        closed_by_client,
        "/stt connection closed"
    );
}

async fn send_json<T: Serialize>(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    msg: &T,
) -> Result<(), axum::Error> {
    let s = match serde_json::to_string(msg) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "/stt: serialize failed");
            return Ok(());
        }
    };
    sink.send(Message::Text(s)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_serializes_to_tagged_json() {
        let s = serde_json::to_string(&SttServerMessage::Ready).unwrap();
        assert_eq!(s, r#"{"type":"ready"}"#);
    }

    #[test]
    fn interim_serializes_with_text() {
        let s = serde_json::to_string(&SttServerMessage::Interim {
            text: "hello world",
        })
        .unwrap();
        assert_eq!(s, r#"{"type":"interim","text":"hello world"}"#);
    }

    #[test]
    fn final_carries_timestamps() {
        let s = serde_json::to_string(&SttServerMessage::Final {
            text: "Hi.",
            t_start_ms: 100,
            t_end_ms: 800,
        })
        .unwrap();
        assert_eq!(
            s,
            r#"{"type":"final","text":"Hi.","t_start_ms":100,"t_end_ms":800}"#
        );
    }

    #[test]
    fn error_carries_code_and_message() {
        let s = serde_json::to_string(&SttServerMessage::Error {
            code: "stt_unavailable",
            message: "no api key",
        })
        .unwrap();
        assert_eq!(
            s,
            r#"{"type":"error","code":"stt_unavailable","message":"no api key"}"#
        );
    }
}
