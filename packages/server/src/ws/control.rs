//! Control-plane WebSocket handler and per-socket loop.
//!
//! `ws_control_handler` — axum upgrade entry for `/`
//! `ws_audio_handler`   — axum upgrade entry for `/audio`
//! `run_control_socket` — per-socket select loop for the control plane
//! `run_audio_socket`   — per-socket loop forwarding PCM into the meeting pipeline
//! `dispatch_intent`    — decode + apply a client intent JSON frame
//! `recover_active_meetings` — boot-time recovery of in-flight meetings

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::protocol::{Event, Intent};

use super::{CLOSE_GOING_AWAY, CLOSE_INTERNAL};
use crate::context::ServerHandle;

/// Auth params shared by all WS handlers — the token is in the
/// query string by convention (URLSessionWebSocketTask doesn't
/// expose custom headers ergonomically, and the PWA mirrors that).
#[derive(Debug, serde::Deserialize)]
pub struct WsAuthParams {
    pub token: Option<String>,
}

/// 401 response for failed WS auth. We can't send a Close frame
/// before the upgrade completes, so a plain HTTP 401 is the right
/// way to reject — clients see the failed handshake and won't try
/// to read frames.
pub fn auth_failed_response(reason: &'static str) -> Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    (StatusCode::UNAUTHORIZED, reason).into_response()
}

/// Local alias for the in-file callers that still use the short name.
fn auth_failed(reason: &'static str) -> Response {
    auth_failed_response(reason)
}

pub async fn ws_control_handler(
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
    // Cache the validated JWT. With the consolidated kleos audience,
    // the same token works for mnemo, so the mnemo client can use it
    // without the UI needing to send a separate SetAuthToken first.
    // SetAuthToken is still useful mid-session for silent-refresh.
    if let Some(token) = auth.token.as_deref() {
        cache_user_token_and_drain(&handle, &user_id, token.to_string());
    }
    ws.on_upgrade(move |socket| run_control_socket(socket, peer, handle, user_id))
}

/// Store a user's auth token in the mnemo token store and kick off an
/// order-preserving drain of any queued events (auth gaps AND
/// transient mnemo failures). No-op when the mnemo client is disabled
/// (no token store exists). Called from both the WS handshake and the
/// SetAuthToken intent.
pub fn cache_user_token_and_drain(handle: &ServerHandle, user_id: &str, token: String) {
    // Only Auth0-issued tokens work against mnemo. Paired-device JWTs
    // (iss=auris-server, aud=auris-api) fail mnemo's JWKS validation;
    // caching one would overwrite a good Auth0 token and silently
    // break every subsequent push until the next Auth0 reconnect.
    if crate::auth::pairing::peek_issuer(&token).as_deref()
        == Some(crate::auth::pairing::JWT_ISSUER)
    {
        return;
    }
    let Some(tokens) = handle.mnemo.tokens() else {
        return;
    };
    tokens.store(user_id, token);
    // Replay any backlog through the order-preserving drain. Unlike
    // the old inline loop (which discarded an event on any failed
    // replay — plus everything already taken out of the queue),
    // drain_pending requeues the failed event and the remainder at
    // the front on transient failure, so a mid-drain blip loses
    // nothing.
    let client = handle.mnemo.clone();
    let uid = user_id.to_string();
    tokio::spawn(async move {
        client.drain_pending(&uid).await;
    });
}

pub async fn ws_audio_handler(
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

/// Server-internal events never serialize onto a client WS. They exist
/// only on the in-process broadcast bus: `MeetingFinalized` retimes the
/// mnemo session reset; `TranscriptTail` carries finalize's drained
/// post-stop transcript to the persistence loop + mnemo pusher.
fn is_server_internal(event: &Event) -> bool {
    matches!(
        event,
        Event::MeetingFinalized { .. } | Event::TranscriptTail { .. }
    )
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
    let mut events_rx = handle.bus.fanout.subscribe();

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
        let mut s = handle.sessions.lock().await;
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

    // Synthetic follow-up: the snapshot ships an empty
    // `attached_meeting_ids` (state machine doesn't cache them in
    // memory). If the user actually has an active meeting with
    // attachments, replay the current set right after the snapshot
    // so the picker UI gets canonical state on reconnect. Best
    // effort — DB miss is logged and skipped (next attach/detach
    // will re-broadcast anyway).
    let active_meeting_id: Option<String> = {
        let s = handle.sessions.lock().await;
        s.user(&user_id)
            .and_then(|u| u.meeting.as_ref())
            .map(|m| m.meeting_id.clone())
    };
    // Synthetic follow-up: ship the user's quick-ask library as an
    // ItemsUpdate on `quick_asks` mode. The snapshot above ships only
    // items for the *current* mode, so a fresh client wouldn't see
    // the library until it switched modes. Replays as a broadcast so
    // every connection for this user converges on the same state.
    super::intent_quick_asks::broadcast_quick_asks(&handle, &user_id).await;

    if let Some(mid) = active_meeting_id {
        match crate::storage::meetings::list_attached_meeting_ids(&handle.db, &mid).await {
            Ok(ids) if !ids.is_empty() => {
                let evt = crate::protocol::Event::AttachedMeetingsChanged { meeting_ids: ids };
                if let Ok(json) = serde_json::to_string(&evt) {
                    if sink.send(Message::Text(json)).await.is_err() {
                        return;
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                warn!(?peer, error = ?e, "post-snapshot attached_meetings list failed");
            }
        }
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
                        // Server-internal signals; never go on the wire
                        // (clients don't know these variants).
                        if is_server_internal(&envelope.event) {
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
        let mut s = handle.sessions.lock().await;
        s.unregister_connection(&connection_id)
    };
    if let Some((owner_uid, d)) = removed {
        info!(?peer, device_id = %d.id, hostname = %d.hostname, user_id = %owner_uid, "device unregistered on disconnect");
        let devices = handle.sessions.lock().await.devices_clone_for(&owner_uid);
        handle
            .bus
            .emit(owner_uid, Event::DevicesChanged { devices })
            .await;
    }

    info!(?peer, "control connection closed");
}

/// Resolve the CURRENT active meeting's audio sender by going through
/// the session registry — never through a cached per-meeting
/// `RemoteAudioSource` Arc. Each meeting gets a *fresh* source
/// (`MeetingRuntime::new`), so an Arc captured at `/audio` accept time
/// goes permanently dead the moment that meeting stops; a socket that
/// outlives a stop→start must re-resolve to find the new meeting's
/// source. Returns `None` when no meeting is active, or when one is
/// active but `spawn_live_pipeline` hasn't called `start()` on its
/// source yet — callers drop the frame and retry on the next one.
///
/// Locking: the registry lock is taken briefly and released before the
/// await on `current_sender()` (a separate, per-source mutex), so we
/// never hold the global sessions lock across an await point.
async fn resolve_audio_sender(
    sessions: &Arc<Mutex<crate::session::SessionRegistry>>,
    user_id: &str,
) -> Option<tokio::sync::mpsc::Sender<Vec<u8>>> {
    let src = {
        let s = sessions.lock().await; // released before the await below
        s.audio_source_for_active_meeting(user_id)
    };
    match src {
        Some(src) => src.current_sender().await,
        None => None,
    }
}

/// Handles the `/audio` WebSocket. The client streams binary frames
/// of 16 kHz mono S16LE PCM (~640 bytes each); the handler forwards
/// each frame into the active meeting's audio sender (held by
/// `RemoteAudioSource`).
///
/// Per-meeting semantics (spec §4.2): the audio pipe is owned by
/// `MeetingRuntime` and created fresh for each meeting. Opening
/// `/audio` before `start_meeting` is now an explicit error —
/// the socket closes with 1011 instead of lazy-creating a pipe
/// that might never see PCM. Clients should open `/audio` after
/// `start_meeting` (or reconnect when the meeting-state event fires).
///
/// A socket that *survives* a stop→start (e.g. a client whose control
/// plane is mid-reconnect and never saw `meeting_state == "idle"`)
/// re-binds to the new meeting automatically: every sender-cache
/// refresh re-resolves the audio source through the session registry
/// (`resolve_audio_sender`) rather than the per-meeting source
/// captured at accept time, which is dead once its meeting stops.
async fn run_audio_socket(
    socket: WebSocket,
    peer: SocketAddr,
    handle: ServerHandle,
    user_id: String,
) {
    // Look up the audio source owned by the user's active meeting.
    // Lock released before any await so we don't hold it across I/O.
    let remote = {
        let sessions = handle.sessions.lock().await;
        match sessions.audio_source_for_active_meeting(&user_id) {
            Some(src) => src,
            None => {
                // No active meeting → no audio pipe to bind to. Spec §4.2:
                // refuse the upgrade cleanly instead of lazy-creating.
                tracing::warn!(
                    user_id = %user_id,
                    ?peer,
                    "audio socket opened with no active meeting; closing with 1011"
                );
                let (mut sink, _) = socket.split();
                let _ = sink
                    .send(Message::Close(Some(CloseFrame {
                        code: CLOSE_INTERNAL,
                        reason: "no active meeting".into(),
                    })))
                    .await;
                return;
            }
        }
    };
    info!(?peer, user_id = %user_id, "/audio connection accepted");
    // Audio is flowing again — cancel any liveness-reaper timer armed
    // by a previous disconnect (covers crash/force-quit → relaunch,
    // and transient network drops). Capture this socket's generation:
    // the close below passes it back so that if a REPLACEMENT socket
    // connects before this one's close lands (Wi-Fi→LTE handoff, fast
    // relaunch), our late close can't re-arm the timer under the
    // replacement's feet.
    let my_audio_gen = handle.sessions.lock().await.mark_audio_connected(&user_id);

    let (mut sink, mut stream) = socket.split();
    let mut frames_received: u64 = 0;
    let mut bytes_received: u64 = 0;
    let mut frames_dropped_no_meeting: u64 = 0;
    // Cached sender for the active meeting. Initialized from the
    // source resolved at accept time (`remote`); refreshed lazily
    // whenever empty — on first frame, after a `Closed` error (the
    // meeting just ended), and every frame while no meeting is
    // consuming. Refreshes go through the session registry
    // (`resolve_audio_sender`), never through `remote`: each meeting
    // owns a fresh `RemoteAudioSource`, so a socket that survives a
    // stop→start must re-resolve to bind to the new meeting's pipe.
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

                        // Refresh by re-resolving through the session
                        // registry — NOT the Arc captured at accept
                        // time. Each meeting gets a fresh
                        // `RemoteAudioSource`, so the accept-time Arc
                        // is permanently dead once that meeting stops.
                        // This covers both the StartMeeting →
                        // spawn_live_pipeline race (source exists but
                        // start() hasn't run yet) and a stop→start on
                        // a surviving socket (frames re-bind to the
                        // NEW meeting's pipe within one frame).
                        if tx_cache.is_none() {
                            tx_cache = resolve_audio_sender(&handle.sessions, &user_id).await;
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
                                    // re-resolves through the registry,
                                    // picking up a restarted meeting
                                    // within one frame (~20 ms).
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
                        if frames_received.is_multiple_of(250) {
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
    // Arm the liveness-reaper timer: if the meeting is still active and
    // no `/audio` reconnects within the grace window, the reaper ends
    // it (see `spawn_liveness_reaper`). No-op if the meeting already
    // ended (a normal stop closes /audio too) or if this socket was
    // already replaced by a newer one (stale generation).
    handle
        .sessions
        .lock()
        .await
        .mark_audio_disconnected(&user_id, my_audio_gen);
}

/// Each entry is `(user_id, meeting_id)` for one user that had an
/// unfinished meeting at server stop. Boot recovery hands these off
/// to the per-user pipeline-spawn path.
pub(crate) struct RecoveredUserMeeting {
    pub user_id: String,
    pub meeting_id: String,
}

/// On boot, look for a meeting whose `ended_at` is still NULL in the
/// `meetings` table — that's the previous run dying mid-meeting.
/// Replay its transcript from the per-meeting JSONL blob, mutate
/// `SessionRegistry` so the next snapshot reflects an active meeting,
/// and return the recovered id (used by the caller to spawn the
/// live pipeline + emit a synthetic state-change event).
///
/// Returns `None` if there's nothing to recover or if loading fails.
/// Failures are logged but never propagate — boot recovery is
/// best-effort, the server should still come up even if the disk
/// is corrupted.
pub(crate) async fn recover_active_meetings(
    db: &sqlx::PgPool,
    state: &Arc<Mutex<crate::session::SessionRegistry>>,
) -> Vec<RecoveredUserMeeting> {
    // Test escape hatch: integration tests share a process and would
    // resurrect each other's leftover meetings without this gate.
    if crate::config::flag("AURIS_SKIP_BOOT_RECOVERY") {
        return Vec::new();
    }
    let rows = match crate::storage::meetings::find_active_meetings_per_user(db).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = ?e, "find_active_meetings_per_user failed; skipping boot recovery");
            return Vec::new();
        }
    };

    let mut recovered = Vec::new();
    let mut seen_users = std::collections::HashSet::new();
    for (user_id, meeting_id, description, metadata_json, started_at, sensitivity_str) in rows {
        // One active meeting per user is the design invariant. If
        // the DB has stragglers (e.g., crash mid-stop), pick the
        // newest per user (rows are ordered DESC) and ignore older.
        if !seen_users.insert(user_id.clone()) {
            continue;
        }
        let metadata: HashMap<String, String> = match serde_json::from_str(&metadata_json) {
            Ok(m) => m,
            Err(err) => {
                tracing::warn!(meeting_id, error = %err, "meeting metadata malformed at boot recovery; using empty map");
                HashMap::new()
            }
        };
        let transcript_items = crate::storage::persistence_loop::read_transcription(&meeting_id)
            .await
            .unwrap_or_default();
        // NULL / unknown values fall back to the default (Moderate).
        // Logged at debug so a misspelled value surfaces in dev.
        let assist_sensitivity = sensitivity_str
            .as_deref()
            .and_then(crate::protocol::AssistSensitivity::parse_wire)
            .unwrap_or_default();
        info!(
            user_id = %user_id,
            meeting_id = %meeting_id,
            items = transcript_items.len(),
            ?started_at,
            assist_sensitivity = assist_sensitivity.as_str(),
            "recovering active meeting"
        );
        let r = crate::session::RecoveredMeeting {
            id: meeting_id.clone(),
            description,
            metadata,
            started_at,
            transcript_items,
            assist_sensitivity,
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

/// Sibling boot sweep, run right after `recover_active_meetings`:
/// a meeting that ENDED but still reads `wrap_up_status='running'`
/// was orphaned by a restart mid-finalize — finalize and the wrap-up
/// retry are detached `tokio::spawn` tasks and die with the process,
/// and the retry endpoint rejects 'running' rows with 400
/// `already_running`, so without this sweep the row is wedged forever
/// (manual SQL was the only escape). Flipping to 'failed' surfaces
/// the existing red banner + retry/regenerate affordances on all
/// three clients; retry is idempotent (replace-by-mode), so even a
/// mislabeled row costs at most one spurious banner.
///
/// Rows with `ended_at IS NULL` are deliberately spared — those
/// belong to `recover_active_meetings` above, whose rehydrated
/// pipeline runs a fresh finalize that overwrites the status itself.
///
/// Best-effort, mirroring boot recovery's posture: failures are
/// logged, never propagated. Gated behind the same
/// `AURIS_SKIP_BOOT_RECOVERY` flag so process-sharing integration
/// tests can't clobber each other's deliberately-'running' fixtures.
/// Single-instance deployment makes the unconditional sweep sound:
/// at boot, no other process can own a genuinely-running finalize.
pub(crate) async fn sweep_orphaned_wrap_ups(db: &sqlx::PgPool) {
    if crate::config::flag("AURIS_SKIP_BOOT_RECOVERY") {
        return;
    }
    match crate::storage::meetings::fail_orphaned_wrap_ups(db).await {
        Ok(0) => {}
        Ok(n) => info!(count = n, "boot: orphaned wrap-ups marked failed"),
        Err(e) => warn!(error = ?e, "boot: orphaned wrap-up sweep failed"),
    }
}

/// Sink alias used by the control-plane intent path. axum's
/// `WebSocket` is split into a `SplitSink<WebSocket, Message>`;
/// callers pass a `&mut` to that.
pub(super) type WsSender = futures_util::stream::SplitSink<WebSocket, Message>;

/// Wire-format strings the control plane recognises for `Intent::*`.
/// Used as an early-gate allow-list in `dispatch_intent` so we can
/// emit a friendly `unknown_intent` error before serde's strict
/// deserializer produces a generic "data did not match any variant"
/// blob.
///
/// **This array MUST stay in sync with the `Intent` enum.** The
/// `intent_wire_name` match below is compiler-checked exhaustive, and
/// `known_intents_covers_all_intent_variants` (test) confirms every
/// variant's wire name is in this array. Adding a variant without
/// updating both will fail to compile or fail tests — never silently
/// reach a client as the kind of "unknown_intent" regression that
/// shipped before this guard was added.
const KNOWN_INTENTS: &[&str] = &[
    "start_meeting",
    "stop_meeting",
    "set_assist_sensitivity",
    "pause",
    "resume",
    "set_mode",
    "set_metadata",
    "register_device",
    "mark_moment",
    "expand_item",
    "chat",
    "set_auth_token",
    "mint_pair_code",
    "upsert_quick_ask",
    "delete_quick_ask",
];

/// Map an `Intent` to its serde-encoded wire `type` string. The
/// match is exhaustive — adding a new `Intent` variant fails to
/// compile here, which is the compile-time half of the drift guard
/// for `KNOWN_INTENTS`. The runtime half is the unit test below.
///
/// Only used by tests today; if a runtime caller ever needs it,
/// remove the `#[cfg(test)]` gate.
#[cfg(test)]
fn intent_wire_name(intent: &Intent) -> &'static str {
    match intent {
        Intent::StartMeeting { .. } => "start_meeting",
        Intent::StopMeeting => "stop_meeting",
        Intent::SetAssistSensitivity { .. } => "set_assist_sensitivity",
        Intent::Pause => "pause",
        Intent::Resume => "resume",
        Intent::SetMode { .. } => "set_mode",
        Intent::SetMetadata { .. } => "set_metadata",
        Intent::RegisterDevice { .. } => "register_device",
        Intent::MarkMoment { .. } => "mark_moment",
        Intent::ExpandItem { .. } => "expand_item",
        Intent::Chat { .. } => "chat",
        Intent::UpsertQuickAsk { .. } => "upsert_quick_ask",
        Intent::DeleteQuickAsk { .. } => "delete_quick_ask",
        Intent::SetAuthToken { .. } => "set_auth_token",
        Intent::MintPairCode { .. } => "mint_pair_code",
    }
}

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
    let Some(ty) = ty else {
        send_protocol_error(sink, "unknown_intent", "missing 'type' field", None).await?;
        return Ok(());
    };
    if !KNOWN_INTENTS.contains(&ty.as_str()) {
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

    // Chat dispatches to the agent task via the kick channel; it
    // doesn't mutate state directly. Validation: meeting must be
    // active (chat is per-meeting only in v1). Attachments
    // (added 2026-05-12) are resolved here: row by id, ownership-
    // checked (same meeting + same user), bytes read from disk, then
    // packed into AttachmentPayload for the agent kick. Any error
    // surfaces as an Event::Error and aborts the send (no partial
    // attachments leak through to the agent).
    if let Intent::Chat {
        text,
        attachment_ids,
    } = intent
    {
        return super::intent_chat::handle_chat(handle, user_id, sink, text, attachment_ids).await;
    }

    // ExpandItem also dispatches to the agent — same pattern as
    // Chat. Look up the item by id (it may live in any mode), pack
    // mode + text into the kick payload so the agent's prompt
    // tells it which mode it's expanding on. The agent's text
    // reply lands as the item's `detail` via `Event::ItemUpdated`.
    if let Intent::ExpandItem { item_id } = intent {
        return super::intent_chat::handle_expand(handle, user_id, sink, item_id).await;
    }

    // SetAuthToken refreshes the cached JWT for the user without
    // dropping the WS. First-connect tokens are already cached at the
    // handshake (see ws_control_handler); this intent covers Auth0
    // silent-refresh while the WS stays open. Both code paths funnel
    // through cache_user_token_and_drain to keep behavior identical.
    if let Intent::SetAuthToken { access_token } = intent {
        cache_user_token_and_drain(handle, user_id, access_token);
        return Ok(());
    }

    // MintPairCode needs the DB pool (not on SessionRegistry), and the
    // response is direct-only — the code is sensitive enough that
    // we don't want it broadcast across the user's other surfaces.
    // The HTTP /pair/code endpoint stays alive for backwards compat.
    if let Intent::MintPairCode { device_label: _ } = intent {
        return super::intent_pairing::handle_mint_pair_code(handle, user_id, sink).await;
    }

    // UpsertQuickAsk needs the DB pool. Validates length limits + the
    // 50-asks-per-user cap, writes to the DB, refreshes the in-memory
    // items_per_mode["quick_asks"] from disk so the broadcast carries
    // a canonical ordered set.
    if let Intent::UpsertQuickAsk {
        id,
        label,
        text,
        position,
    } = intent
    {
        return super::intent_quick_asks::handle_upsert(
            handle, user_id, sink, id, label, text, position,
        )
        .await;
    }

    if let Intent::DeleteQuickAsk { id } = intent {
        return super::intent_quick_asks::handle_delete(handle, user_id, sink, id).await;
    }

    if let Intent::RegisterDevice {
        hostname,
        capabilities,
        device_id,
    } = intent
    {
        return super::intent_pairing::handle_register_device(
            handle,
            user_id,
            connection_id,
            sink,
            hostname,
            capabilities,
            device_id,
        )
        .await;
    }

    let mut outcome = {
        let mut s = handle.sessions.lock().await;
        s.apply_intent(user_id, intent)
    };

    if let Some(direct_event) = outcome.originator_only {
        let json = serde_json::to_string(&direct_event)?;
        sink.send(Message::Text(json)).await.ok();
    }
    for event in outcome.events {
        handle.bus.emit(user_id.to_string(), event).await;
    }
    if outcome.started_meeting {
        // The cancel token was already installed by `MeetingRuntime::new`
        // inside `handle_start_meeting`. Fetch a clone for the pipeline
        // spawn — lock released before the await.
        let token = {
            let sessions = handle.sessions.lock().await;
            sessions
                .meeting_cancel_token(user_id)
                .expect("MeetingRuntime::new installs cancel token")
        };
        spawn_live_pipeline(handle.clone(), user_id.to_string(), token.child_token()).await;
    }
    if let Some(description) = outcome.start_extraction_for {
        // Cancel any previous extraction for *this* user; install
        // a fresh token. Cross-user extractions don't interfere.
        let token = handle.sessions.lock().await.extraction_cancel_for(user_id);
        spawn_extraction(handle.clone(), user_id.to_string(), description, token);
    }
    if let Some(runtime) = outcome.stopped_runtime {
        // Cancel any in-flight metadata extraction first.
        handle.sessions.lock().await.cancel_extraction_for(user_id);
        // Detached graceful finalize: drains the STT pipeline (so the
        // last sentences aren't lost), runs wrap-up on the COMPLETE
        // transcript, then tears down the remaining tasks. The meeting
        // is already Idle in session state — clients have closed.
        let pre_stop_transcript = outcome
            .start_wrap_up
            .take()
            .map(|r| r.transcript_text)
            .unwrap_or_default();
        let db = handle.db.clone();
        let chat_llm = handle.chat_llm.clone();
        let background_llm = handle.background_llm.clone();
        let bus = handle.bus.clone();
        let uid = user_id.to_string();
        handle.tasks.spawn(async move {
            crate::workers::finalize::run(
                runtime,
                db,
                chat_llm,
                background_llm,
                bus,
                uid,
                pre_stop_transcript,
            )
            .await;
        });
    }

    // Persistence side-effects. None of these block the broadcast
    // path above — events have already gone out by the time we get
    // here. A DB hiccup logs a warning but doesn't fail the intent;
    // the meeting still proceeds in memory.
    if let Some(rec) = outcome.created_meeting {
        let metadata_json =
            serde_json::to_string(&rec.metadata).unwrap_or_else(|_| "{}".to_string());
        if let Err(e) = crate::storage::meetings::insert_meeting(
            &handle.db,
            &rec.id,
            user_id,
            rec.started_at,
            rec.description.as_deref(),
            &metadata_json,
            Some(rec.assist_sensitivity.as_str()),
        )
        .await
        {
            tracing::warn!(error = ?e, meeting_id = %rec.id, "insert_meeting failed");
        } else {
            tracing::info!(meeting_id = %rec.id, user_id = %user_id, "meeting persisted");
        }
    }
    // Mid-meeting sensitivity change — write the new value to the
    // meeting row so a reconnect sees the same choice. Idempotent
    // at the SQL level; the intent handler already gates on
    // "value actually changed" before populating this field.
    if let Some(persist) = outcome.assist_sensitivity_persist {
        if let Err(e) = crate::storage::meetings::set_assist_sensitivity(
            &handle.db,
            &persist.meeting_id,
            persist.value.as_str(),
        )
        .await
        {
            tracing::warn!(
                error = ?e,
                meeting_id = %persist.meeting_id,
                value = persist.value.as_str(),
                "set_assist_sensitivity DB write failed"
            );
        }
    }
    if let Some(rec) = outcome.closed_meeting {
        if let Err(e) =
            crate::storage::meetings::end_meeting(&handle.db, &rec.id, rec.ended_at).await
        {
            tracing::warn!(error = ?e, meeting_id = %rec.id, "end_meeting failed");
        } else {
            tracing::info!(meeting_id = %rec.id, "meeting closed in db");
        }
    }
    if let Some(req) = outcome.mark_moment {
        let moment_id = uuid::Uuid::new_v4().to_string();
        let kind = "manual";
        match crate::storage::moments::insert_moment(
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
                // Tell the agent immediately. The summary worker
                // takes 15-22 s end-to-end (grace + LLM + DB write),
                // so without this kick the agent wouldn't know the
                // moment exists if the user chats about it right
                // after snapping. The summary lands as a follow-up
                // event when ready.
                let _ = handle.agent_kick_tx.send(crate::agent::AgentKick {
                    user_id: user_id.to_string(),
                    reason: crate::agent::AgentKickReason::MomentMarked {
                        t_ms: req.t as i64,
                        note: req.note.clone(),
                    },
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
                    let s = handle.sessions.lock().await;
                    s.user(user_id)
                        .and_then(|u| {
                            u.meeting
                                .as_ref()
                                .and_then(|m| m.audio_source_device_id.clone())
                        })
                        .and_then(|device_id| {
                            s.user(user_id).and_then(|u| {
                                u.devices_by_connection
                                    .iter()
                                    .find(|(_, d)| {
                                        d.id == device_id
                                            && d.online
                                            && d.capabilities.contains(
                                                &crate::protocol::Capability::ScreenCapture,
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

pub fn heartbeat_interval() -> Duration {
    if let Ok(s) = std::env::var("AURIS_HEARTBEAT_MS") {
        if let Ok(ms) = s.parse::<u64>() {
            return Duration::from_millis(ms);
        }
    }
    Duration::from_secs(10)
}

/// Grace window before the liveness reaper ends a meeting whose audio
/// source has disconnected. Generous by default (15 min) so a Wi-Fi
/// blip, display sleep, or a quick app relaunch — which resumes capture
/// via the Mac snapshot path — never ends a live meeting. Override via
/// `AURIS_AUDIO_LIVENESS_TIMEOUT_MS`.
pub fn audio_liveness_timeout() -> Duration {
    if let Ok(s) = std::env::var("AURIS_AUDIO_LIVENESS_TIMEOUT_MS") {
        if let Ok(ms) = s.parse::<u64>() {
            return Duration::from_millis(ms);
        }
    }
    Duration::from_secs(15 * 60)
}

/// Sweep period for the liveness-reaper loop. Coarse by default
/// (60 s) — the grace window (`audio_liveness_timeout`, minutes) sets
/// the real latency, so sweep precision doesn't matter in production.
/// Override via `AURIS_LIVENESS_SWEEP_MS`; integration tests shrink
/// it to ~100 ms so a reap is observable within a test deadline
/// (see `tests/liveness_reaper.rs`).
pub fn liveness_sweep_interval() -> Duration {
    if let Ok(s) = std::env::var("AURIS_LIVENESS_SWEEP_MS") {
        if let Ok(ms) = s.parse::<u64>() {
            return Duration::from_millis(ms);
        }
    }
    Duration::from_secs(60)
}

/// Background task: periodically end meetings whose audio source has
/// been gone past `audio_liveness_timeout()`. The safety net for a
/// client that dies mid-meeting and never returns — without it the
/// meeting sits Active (`ended_at` NULL) forever. A client that DOES
/// return reconnects `/audio`, clearing the timer (`mark_audio_connected`),
/// so this never races a resume. Sweeps coarsely (the grace is minutes).
pub fn spawn_liveness_reaper(handle: ServerHandle) {
    tokio::spawn(async move {
        let timeout = audio_liveness_timeout();
        info!(timeout_s = timeout.as_secs(), "liveness reaper started");
        let mut interval = tokio::time::interval(liveness_sweep_interval());
        interval.tick().await; // skip the immediate first tick
        loop {
            tokio::select! {
                _ = handle.shutdown.cancelled() => break,
                _ = interval.tick() => {}
            }
            let stale = {
                let s = handle.sessions.lock().await;
                s.stale_audio_meetings(timeout)
            };
            for user_id in stale {
                warn!(
                    user_id = %user_id,
                    timeout_s = timeout.as_secs(),
                    "liveness reaper: audio source gone past grace window — ending abandoned meeting"
                );
                reap_stale_meeting(&handle, &user_id, timeout).await;
            }
        }
    });
}

/// End one abandoned meeting through the SAME stop→finalize path a
/// client `StopMeeting` uses, so wrap-up/summary run and `ended_at` is
/// set. Re-checks staleness under the lock so we never end a meeting
/// that reconnected `/audio` between the sweep and here.
async fn reap_stale_meeting(handle: &ServerHandle, user_id: &str, timeout: Duration) {
    let outcome = {
        let mut s = handle.sessions.lock().await;
        if !s.is_audio_stale(user_id, timeout) {
            // Reconnected (timer cleared) or already ended in the gap —
            // don't reap. Drop any leftover timer for tidiness.
            s.clear_audio_loss(user_id);
            return;
        }
        let outcome = s.apply_intent(user_id, Intent::StopMeeting);
        // Mirror the client stop path: drop any in-flight metadata
        // extraction, and clear the loss timer now that we've acted.
        s.cancel_extraction_for(user_id);
        s.clear_audio_loss(user_id);
        outcome
    };
    apply_reaped_stop_outcome(handle, user_id, outcome).await;
}

/// Process the subset of `IntentOutcome` a `StopMeeting` produces, off
/// the session lock: broadcast the state change, spawn the detached
/// finalize (STT drain + wrap-up + summary on the complete transcript),
/// and persist `ended_at`. Mirrors the stop branch of `dispatch_intent`
/// — keep the two in sync.
async fn apply_reaped_stop_outcome(
    handle: &ServerHandle,
    user_id: &str,
    mut outcome: crate::session::IntentOutcome,
) {
    for event in outcome.events {
        handle.bus.emit(user_id.to_string(), event).await;
    }
    if let Some(runtime) = outcome.stopped_runtime {
        let pre_stop_transcript = outcome
            .start_wrap_up
            .take()
            .map(|r| r.transcript_text)
            .unwrap_or_default();
        let db = handle.db.clone();
        let chat_llm = handle.chat_llm.clone();
        let background_llm = handle.background_llm.clone();
        let bus = handle.bus.clone();
        let uid = user_id.to_string();
        handle.tasks.spawn(async move {
            crate::workers::finalize::run(
                runtime,
                db,
                chat_llm,
                background_llm,
                bus,
                uid,
                pre_stop_transcript,
            )
            .await;
        });
    }
    if let Some(rec) = outcome.closed_meeting {
        if let Err(e) =
            crate::storage::meetings::end_meeting(&handle.db, &rec.id, rec.ended_at).await
        {
            warn!(error = ?e, meeting_id = %rec.id, "liveness reaper: end_meeting failed");
        } else {
            info!(meeting_id = %rec.id, "liveness reaper: meeting closed in db");
        }
    }
}

fn spawn_extraction(
    handle: ServerHandle,
    user_id: String,
    description: String,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        // Dev escape hatch.
        if crate::config::flag("AURIS_LLM_DISABLED") {
            tracing::info!("LLM extraction disabled by env var; skipping");
            return;
        }

        tracing::info!(
            provider = ?handle.background_llm.provider(),
            description_len = description.len(),
            user_id = %user_id,
            "metadata extraction starting"
        );
        let extracted = tokio::select! {
            result = handle.background_llm.extract(&description) => match result {
                Ok(map) => {
                    tracing::info!(field_count = map.len(), fields = ?map.keys().collect::<Vec<_>>(), "metadata extraction succeeded");
                    map
                }
                Err(e) => {
                    tracing::warn!(error = %e, "metadata extraction failed");
                    let s = handle.sessions.lock().await;
                    let user = s.user(&user_id);
                    let listening = user
                        .map(|u| matches!(u.meeting_state, crate::protocol::MeetingState::Active))
                        .unwrap_or(false);
                    let status = crate::protocol::Status {
                        listening,
                        error: Some(short_error(&e)),
                    };
                    drop(s);
                    handle
                        .bus
                        .emit(user_id.clone(), Event::Status { status })
                        .await;
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
            let mut s = handle.sessions.lock().await;
            let user = s.user_mut(&user_id);
            let manual = user.metadata_clone();
            let merged = crate::workers::metadata::merge_manual_wins(extracted, &manual);
            user.set_metadata_full(merged.clone());
            Event::MetadataChanged { metadata: merged }
        };
        // Durable: the mnemo pusher must never miss MetadataChanged.
        handle.bus.emit(user_id, event).await;
    });
}

fn short_error(e: &crate::llm::ExtractionError) -> String {
    use crate::llm::ExtractionError::*;
    match e {
        Timeout(_) => "Metadata extraction timed out".to_string(),
        QuotaExhausted(_) => "LLM account out of credits / over quota".to_string(),
        Provider(_) => "LLM provider rejected the request".to_string(),
        Schema(_) | NoData => "Metadata extraction returned no usable data".to_string(),
        Extract(_) => "Metadata extraction failed".to_string(),
        CircuitOpen(_) => "LLM circuit breaker open — temporarily unavailable".to_string(),
    }
}

pub async fn spawn_live_pipeline(handle: ServerHandle, user_id: String, cancel: CancellationToken) {
    // -------------------------------------------------------------------
    // Audio source — per-meeting. The `RemoteAudioSource` now lives on
    // `MeetingRuntime` (created by `MeetingRuntime::new`). We fetch it
    // here by looking up the session registry. The lock is released
    // before any await to avoid holding it across I/O.
    // -------------------------------------------------------------------
    let audio_disabled = crate::config::flag("AURIS_AUDIO_DISABLED");
    let audio_rx = if audio_disabled {
        tracing::info!("audio capture disabled by env var");
        None
    } else {
        let user_audio = {
            let sessions = handle.sessions.lock().await;
            sessions.audio_source_for_active_meeting(&user_id)
        };
        match user_audio {
            Some(src) => {
                let rx = src.start().await;
                tracing::info!(user_id = %user_id, "audio source started");
                Some(rx)
            }
            None => {
                tracing::warn!(
                    user_id = %user_id,
                    "spawn_live_pipeline: no active meeting audio source — pipeline runs silent"
                );
                None
            }
        }
    };

    // The transcript-chunk channel + drain signal live on the
    // MeetingRuntime so the finalize task (which owns the runtime after
    // stop) can subscribe and drain. Fetch clones here; lock released
    // immediately.
    let (chunk_tx, drain_token, reactive_token, meeting_id) = {
        let sessions = handle.sessions.lock().await;
        match (
            sessions.meeting_chunk_sender(&user_id),
            sessions.meeting_drain_token(&user_id),
            sessions.meeting_reactive_token(&user_id),
            sessions.active_meeting_id_for(&user_id),
        ) {
            (Some(tx), Some(dr), Some(rt), Some(mid)) => (tx, dr, rt, mid),
            _ => {
                tracing::error!(
                    user_id = %user_id,
                    "spawn_live_pipeline: no active meeting runtime; aborting pipeline spawn"
                );
                return;
            }
        }
    };

    // -------------------------------------------------------------------
    // STT task — dispatch via trait so future providers slot in cleanly.
    // -------------------------------------------------------------------
    let provider_name = crate::config::var_opt("AURIS_STT_PROVIDER").unwrap_or_else(|| {
        if crate::config::flag("AURIS_STT_MOCK") {
            "mock".to_string()
        } else {
            "soniox".to_string()
        }
    });

    // Collect all meeting-scoped task handles so MeetingRuntime::shutdown
    // can await them. We register each one immediately after spawning.
    let mut meeting_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let mut stt_handle: Option<tokio::task::JoinHandle<()>> = None;

    match crate::stt::make_provider(&provider_name) {
        Ok(provider) => {
            let stt_cancel = cancel.child_token();
            let stt_tx = chunk_tx.clone();
            let stt_events_tx = handle.bus.fanout.clone();
            let stt_uid = user_id.clone();
            tracing::info!(provider = provider.name(), user_id = %user_id, "live pipeline STT spawning");
            let h = tokio::spawn(provider.run(
                audio_rx,
                stt_tx,
                stt_events_tx,
                stt_uid,
                stt_cancel,
                drain_token.clone(),
            ));
            stt_handle = Some(h);
        }
        Err(e) => {
            tracing::error!(error = %e, provider = %provider_name, "STT provider init failed; meeting will run without transcription");
        }
    }

    // Transcript summarizer (no LLM)
    {
        let task_state = Arc::clone(&handle.sessions);
        let task_bus = handle.bus.clone();
        let task_rx = chunk_tx.subscribe();
        let task_cancel = cancel.child_token();
        let task_uid = user_id.clone();
        let task_meeting_id = meeting_id.clone();
        let h = tokio::spawn(async move {
            crate::workers::transcript::run_transcript_summarizer(
                task_state,
                task_rx,
                task_bus,
                task_uid,
                task_meeting_id,
                task_cancel,
            )
            .await;
        });
        meeting_tasks.push(h);
    }

    // Reactive chat agent — fires on user chat / expand-item kicks
    // only. Data-event kicks (moments, artifacts, attachments) get
    // folded into the next chat fire's prompt. See agent::chat.
    {
        let h = crate::agent::spawn_meeting_agent(
            Arc::clone(&handle.sessions),
            handle.db.clone(),
            handle.agent_kick_tx.subscribe(),
            handle.agent_kick_tx.clone(),
            handle.bus.clone(),
            user_id.clone(),
            meeting_id.clone(),
            Arc::clone(&handle.chat_llm),
            handle.mnemo.clone(),
            reactive_token.child_token(),
        );
        meeting_tasks.push(h);
    }

    // Active extraction agent — fires on transcript thresholds + every
    // data-event kick. Calls replace_summary / replace_highlights /
    // push_assist_suggestion as the LLM sees fit per fire. Uses
    // background_llm (cheap, fast). See agent::active.
    {
        let h = crate::agent::spawn_active_extractor(
            Arc::clone(&handle.sessions),
            handle.db.clone(),
            chunk_tx.subscribe(),
            handle.agent_kick_tx.subscribe(),
            handle.bus.clone(),
            user_id.clone(),
            meeting_id.clone(),
            Arc::clone(&handle.background_llm),
            handle.mnemo.clone(),
            reactive_token.child_token(),
        );
        meeting_tasks.push(h);
    }

    // Register all meeting-scoped tasks with the runtime so shutdown can
    // await them. Lock released immediately after registration.
    {
        let mut sessions = handle.sessions.lock().await;
        if let Some(h) = stt_handle {
            sessions.register_meeting_stt_task(&user_id, h);
        }
        for h in meeting_tasks {
            sessions.register_meeting_task(&user_id, h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AssistSensitivity;

    /// The WS forward loop must never serialize server-internal events
    /// onto a client socket — clients don't know these variants.
    /// Regression guard for both `MeetingFinalized` (pre-existing) and
    /// `TranscriptTail` (drained-tail delivery, improvement #19).
    #[test]
    fn server_internal_events_are_not_forwarded_to_clients() {
        assert!(is_server_internal(&Event::MeetingFinalized {
            meeting_id: "m-1".into(),
        }));
        assert!(is_server_internal(&Event::TranscriptTail {
            meeting_id: "m-1".into(),
            items: vec![],
        }));
        assert!(!is_server_internal(&Event::ItemsUpdate {
            mode: "transcript".into(),
            items: vec![],
        }));
        assert!(!is_server_internal(&Event::MeetingStateChanged {
            meeting_state: crate::protocol::MeetingState::Idle,
            meeting_id: None,
        }));
    }

    /// Every `Intent` variant's wire name must be in `KNOWN_INTENTS`.
    /// The pattern match in `intent_wire_name` is compiler-exhaustive,
    /// so adding a variant without naming it fails to compile. This
    /// test catches the other half: a variant gets a wire name but
    /// `KNOWN_INTENTS` isn't updated (the regression that shipped
    /// `set_assist_sensitivity` without an allow-list entry, surfacing
    /// to clients as `unknown_intent`).
    ///
    /// One sample per variant; the only thing the test actually
    /// reads is `intent_wire_name(&v)`, so payload fields can be
    /// minimal / default. Keep this list exhaustive — the `match`
    /// in `intent_wire_name` will not let you forget a new variant,
    /// and this `Vec::new()` won't either (a missing sample means
    /// the test still passes but the new wire name isn't asserted —
    /// reviewers should treat "added Intent variant" as a checklist
    /// item for this list).
    fn one_of_each_intent() -> Vec<Intent> {
        vec![
            Intent::StartMeeting {
                description: None,
                metadata: None,
                audio_source_device_id: None,
                assist_sensitivity: None,
            },
            Intent::StopMeeting,
            Intent::SetAssistSensitivity {
                value: AssistSensitivity::Moderate,
            },
            Intent::Pause,
            Intent::Resume,
            Intent::SetMode {
                mode: "transcript".into(),
            },
            Intent::SetMetadata {
                key: "k".into(),
                value: None,
            },
            Intent::RegisterDevice {
                hostname: "host".into(),
                capabilities: vec![],
                device_id: None,
            },
            Intent::MarkMoment { t: 0, note: None },
            Intent::ExpandItem {
                item_id: "id".into(),
            },
            Intent::Chat {
                text: "".into(),
                attachment_ids: vec![],
            },
            Intent::UpsertQuickAsk {
                id: "id".into(),
                label: "l".into(),
                text: "t".into(),
                position: 0,
            },
            Intent::DeleteQuickAsk { id: "id".into() },
            Intent::SetAuthToken {
                access_token: "tok".into(),
            },
            Intent::MintPairCode { device_label: None },
        ]
    }

    #[test]
    fn known_intents_covers_all_intent_variants() {
        for intent in one_of_each_intent() {
            let name = intent_wire_name(&intent);
            assert!(
                KNOWN_INTENTS.contains(&name),
                "Intent variant emits wire name {name:?} but it's missing from KNOWN_INTENTS"
            );
            // Cross-check: the name we returned must round-trip through
            // serde — i.e. it's the actual `type` tag serde emits, not
            // a typo in `intent_wire_name`.
            let json = serde_json::to_value(&intent).expect("serializes");
            assert_eq!(
                json["type"].as_str(),
                Some(name),
                "intent_wire_name disagrees with serde for {intent:?}"
            );
        }
    }

    #[test]
    fn known_intents_has_no_phantom_entries() {
        // The inverse direction: every string in KNOWN_INTENTS must
        // appear as the `type` of some real Intent variant. If a
        // variant gets removed but KNOWN_INTENTS isn't trimmed, the
        // allow-list lets through a payload that serde will then
        // reject as bad_payload — confusing rather than helpful.
        let observed: std::collections::HashSet<&'static str> =
            one_of_each_intent().iter().map(intent_wire_name).collect();
        for s in KNOWN_INTENTS {
            assert!(
                observed.contains(s),
                "KNOWN_INTENTS entry {s:?} has no matching Intent variant"
            );
        }
    }

    /// Process-wide env-mutation lock. `liveness_sweep_interval_reads_
    /// env_with_60s_default` mutates `AURIS_LIVENESS_SWEEP_MS`; other
    /// env-touching tests in the crate that could race a shared var
    /// take this same lock. `tokio::sync::Mutex` (not std::sync) so the
    /// guard can be held across an `.await` if a future test needs it.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// `spawn_liveness_reaper`'s sweep period must default to 60 s
    /// (production behavior unchanged) and honor
    /// `AURIS_LIVENESS_SWEEP_MS` so integration tests can observe a
    /// reap within a test-sized deadline. Mirrors `heartbeat_interval`.
    #[tokio::test]
    async fn liveness_sweep_interval_reads_env_with_60s_default() {
        let _env_guard = ENV_LOCK.lock().await;
        std::env::remove_var("AURIS_LIVENESS_SWEEP_MS");
        assert_eq!(liveness_sweep_interval(), Duration::from_secs(60));

        std::env::set_var("AURIS_LIVENESS_SWEEP_MS", "100");
        assert_eq!(liveness_sweep_interval(), Duration::from_millis(100));

        // Garbage value falls back to the default, like heartbeat_interval.
        std::env::set_var("AURIS_LIVENESS_SWEEP_MS", "not-a-number");
        assert_eq!(liveness_sweep_interval(), Duration::from_secs(60));

        std::env::remove_var("AURIS_LIVENESS_SWEEP_MS");
    }

    /// Shared fixture for the wrap-up sweep tests: a user plus an
    /// ENDED meeting whose wrap_up_status reads 'running' — the
    /// restart-orphaned shape from improvement #24.
    async fn insert_orphaned_wrap_up(pool: &sqlx::PgPool) -> String {
        let sub = format!("test|{}", uuid::Uuid::new_v4());
        let uid = crate::storage::users::upsert_user_by_auth0_sub(pool, &sub, None, None)
            .await
            .unwrap()
            .id;
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        crate::storage::meetings::insert_meeting(pool, &id, &uid, now, None, "{}", None)
            .await
            .unwrap();
        crate::storage::meetings::end_meeting(pool, &id, now)
            .await
            .unwrap();
        crate::storage::meetings::set_wrap_up_status(pool, &id, "running")
            .await
            .unwrap();
        id
    }

    async fn read_wrap_up_status(pool: &sqlx::PgPool, id: &str) -> Option<String> {
        let row: (Option<String>,) =
            sqlx::query_as("SELECT wrap_up_status FROM meetings WHERE id = $1")
                .bind(id)
                .fetch_one(pool)
                .await
                .unwrap();
        row.0
    }

    /// Improvement #24: with the flag unset (production posture), the
    /// boot sweep flips restart-orphaned ended+'running' rows to
    /// 'failed' so the retry endpoint and client banners apply.
    #[sqlx::test]
    async fn sweep_orphaned_wrap_ups_marks_ended_running_failed(pool: sqlx::PgPool) {
        // Env mutation is safe: --test-threads=1 is mandated for this
        // crate. Save + restore so later tests see the original state.
        let saved = std::env::var("AURIS_SKIP_BOOT_RECOVERY").ok();
        std::env::remove_var("AURIS_SKIP_BOOT_RECOVERY");

        let id = insert_orphaned_wrap_up(&pool).await;
        sweep_orphaned_wrap_ups(&pool).await;

        match saved {
            Some(v) => std::env::set_var("AURIS_SKIP_BOOT_RECOVERY", v),
            None => std::env::remove_var("AURIS_SKIP_BOOT_RECOVERY"),
        }

        assert_eq!(
            read_wrap_up_status(&pool, &id).await.as_deref(),
            Some("failed"),
            "restart-orphaned wrap-up must be marked failed at boot"
        );
    }

    /// The sweep shares boot recovery's test escape hatch: with
    /// AURIS_SKIP_BOOT_RECOVERY set (the integration-test posture,
    /// see tests/common/mod.rs), it must leave 'running' rows alone
    /// so process-sharing tests can't clobber each other's fixtures.
    #[sqlx::test]
    async fn sweep_orphaned_wrap_ups_gated_by_skip_boot_recovery(pool: sqlx::PgPool) {
        let saved = std::env::var("AURIS_SKIP_BOOT_RECOVERY").ok();
        std::env::set_var("AURIS_SKIP_BOOT_RECOVERY", "1");

        let id = insert_orphaned_wrap_up(&pool).await;
        sweep_orphaned_wrap_ups(&pool).await;

        match saved {
            Some(v) => std::env::set_var("AURIS_SKIP_BOOT_RECOVERY", v),
            None => std::env::remove_var("AURIS_SKIP_BOOT_RECOVERY"),
        }

        assert_eq!(
            read_wrap_up_status(&pool, &id).await.as_deref(),
            Some("running"),
            "sweep must be a no-op when AURIS_SKIP_BOOT_RECOVERY is set"
        );
    }

    // ── /audio sender resolution across meeting boundaries ──────────
    //
    // Each meeting gets a FRESH `RemoteAudioSource` (MeetingRuntime::new),
    // so an Arc captured when the `/audio` socket was accepted goes
    // permanently dead the moment that meeting stops. These tests pin
    // the handler's refresh contract: always re-resolve through the
    // SessionRegistry, never through a cached per-meeting Arc.

    use crate::session::SessionRegistry;

    fn start_meeting_intent() -> Intent {
        Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        }
    }

    #[tokio::test]
    async fn resolve_audio_sender_picks_up_new_meeting_after_stop_start() {
        let sessions = Arc::new(Mutex::new(SessionRegistry::new()));
        let uid = "u1";

        // Meeting A starts; spawn_live_pipeline would call start() on
        // its per-meeting source, installing the sender in the slot.
        sessions
            .lock()
            .await
            .apply_intent(uid, start_meeting_intent());
        let src_a = sessions
            .lock()
            .await
            .audio_source_for_active_meeting(uid)
            .expect("meeting A has an audio source");
        let rx_a = src_a.start().await;

        // What the buggy handler does: capture the per-meeting Arc at
        // socket-accept time and refresh from it forever.
        let captured_at_accept = src_a.clone();
        assert!(captured_at_accept.current_sender().await.is_some());

        // Stop A. handle_stop_meeting moves the runtime out via the
        // outcome (the ws layer normally hands it to workers::finalize);
        // dropping the outcome + A's STT receiver simulates finalize
        // teardown.
        let outcome = sessions.lock().await.apply_intent(uid, Intent::StopMeeting);
        drop(outcome);
        drop(rx_a);

        // Meeting B starts on the same user — with a BRAND NEW source —
        // and its pipeline calls start().
        sessions
            .lock()
            .await
            .apply_intent(uid, start_meeting_intent());
        let src_b = sessions
            .lock()
            .await
            .audio_source_for_active_meeting(uid)
            .expect("meeting B has an audio source");
        let mut rx_b = src_b.start().await;

        // The bug this test pins: the accept-time Arc is meeting A's
        // source. Its slot self-cleaned to None when rx_a dropped and
        // will NEVER be repopulated — a handler polling it forwards
        // nothing into meeting B, forever (silent empty transcript).
        assert!(
            captured_at_accept.current_sender().await.is_none(),
            "stale per-meeting Arc must never resolve meeting B's sender"
        );

        // The fix: re-resolving through the registry finds B's live
        // sender, and a frame pushed through it reaches B's pipeline rx.
        let tx = resolve_audio_sender(&sessions, uid)
            .await
            .expect("registry re-resolution must find meeting B's sender");
        tx.try_send(b"frame".to_vec())
            .expect("send into meeting B's channel");
        assert_eq!(rx_b.recv().await.unwrap(), b"frame");
    }

    #[tokio::test]
    async fn resolve_audio_sender_none_when_idle() {
        // No active meeting → nothing to bind to. The handler keeps
        // the socket open, counts the drop, and retries next frame.
        let sessions = Arc::new(Mutex::new(SessionRegistry::new()));
        assert!(resolve_audio_sender(&sessions, "nobody").await.is_none());

        // Also after a meeting existed and stopped.
        sessions
            .lock()
            .await
            .apply_intent("u1", start_meeting_intent());
        sessions
            .lock()
            .await
            .apply_intent("u1", Intent::StopMeeting);
        assert!(resolve_audio_sender(&sessions, "u1").await.is_none());
    }

    #[tokio::test]
    async fn resolve_audio_sender_none_before_pipeline_start() {
        // The StartMeeting → spawn_live_pipeline race: the meeting (and
        // its source) exists, but start() hasn't installed a sender yet.
        // Resolution must return None (drop the frame, retry next frame)
        // rather than panic or fabricate a channel.
        let sessions = Arc::new(Mutex::new(SessionRegistry::new()));
        let uid = "u1";
        sessions
            .lock()
            .await
            .apply_intent(uid, start_meeting_intent());
        assert!(resolve_audio_sender(&sessions, uid).await.is_none());

        // Once the pipeline starts the source, resolution succeeds.
        let src = sessions
            .lock()
            .await
            .audio_source_for_active_meeting(uid)
            .expect("active meeting has a source");
        let _rx = src.start().await;
        assert!(resolve_audio_sender(&sessions, uid).await.is_some());
    }
}
