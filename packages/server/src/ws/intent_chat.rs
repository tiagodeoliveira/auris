//! Agent-kick intent handlers — `Intent::Chat` and `Intent::ExpandItem`.
//! Both fire `AgentKick` via `handle.agent_kick_tx` rather than mutate
//! `SessionRegistry` state directly, so they share this module out of
//! `ws/control.rs`'s `dispatch_intent`.
//!
//! Flow:
//!   1. Reject empty payload (no text, no attachments) silently.
//!   2. Gate on `MeetingState::Active` (chat is per-meeting in v1)
//!      AND capture the current `meeting_id` in the same lock scope.
//!   3. For each attachment id: row lookup, ownership check (same
//!      meeting + same user), disk read. Any failure emits a typed
//!      `Event::Error` and aborts (no partial attachments leak).
//!   4. Mint deterministic `chat-q-*` / `chat-a-*` ids and push an
//!      optimistic user + `assistant-pending` placeholder pair into
//!      chat-mode state. Broadcast immediately so every connected
//!      surface (sender, other Macs, PWA) sees the bubbles before
//!      the LLM call starts. The agent reuses the same ids in the
//!      final `Event::ItemsUpdate` — clients' `applyItemsUpdate`
//!      merges by id, so the placeholder transitions to the real
//!      answer in place.
//!   5. Send `AgentKick::ChatMessage` carrying the text + resolved
//!      attachments + ids.

use anyhow::Result;
use axum::extract::ws::Message;
use futures_util::SinkExt;

use crate::protocol::{Event, Item};

use super::control::WsSender;
use crate::context::ServerHandle;

pub(super) async fn handle_chat(
    handle: &ServerHandle,
    user_id: &str,
    sink: &mut WsSender,
    text: String,
    attachment_ids: Vec<String>,
) -> Result<()> {
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() && attachment_ids.is_empty() {
        // No content at all — silent no-op.
        return Ok(());
    }
    // Gate on Active AND capture the current meeting_id in
    // the same critical section, so we can ownership-check
    // attachments against the same id the kick will use.
    let (active, current_meeting_id) = {
        let s = handle.sessions.lock().await;
        s.user(user_id)
            .map(|u| {
                let ok = matches!(
                    u.snapshot_meeting_state(),
                    crate::protocol::MeetingState::Active
                );
                let mid = u.meeting.as_ref().map(|m| m.meeting_id.clone());
                (ok, mid)
            })
            .unwrap_or((false, None))
    };
    if !active {
        send_no_active_meeting(sink).await?;
        return Ok(());
    }
    // Active implies current_meeting_id is Some. Guard
    // defensively in case an invariant is violated upstream.
    let Some(current_meeting_id) = current_meeting_id else {
        send_no_active_meeting(sink).await?;
        return Ok(());
    };

    // Hoisted: data_dir() is sync and idempotent. Calling it inside
    // the per-attachment loop wasted N-1 syscalls (create_dir_all).
    // One call at the start covers every attachment in this fire.
    let data_dir = match crate::storage::data_dir() {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!(?e, "data_dir() failed");
            send_attachment_error(sink, None, "chat attachment storage unavailable").await?;
            return Ok(());
        }
    };

    let mut attachments: Vec<crate::agent::AttachmentPayload> = Vec::new();
    for att_id in &attachment_ids {
        let row =
            match crate::storage::chat_attachments::get_chat_attachment(&handle.db, att_id).await {
                Ok(Some(row)) => row,
                Ok(None) => {
                    send_attachment_not_found(sink, att_id).await?;
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(?e, attachment_id = %att_id, "chat_attachment lookup failed");
                    send_attachment_unreadable(sink, att_id).await?;
                    return Ok(());
                }
            };
        if row.meeting_id != current_meeting_id || row.user_id != user_id {
            send_attachment_forbidden(sink, att_id).await?;
            return Ok(());
        }
        let abs_path = data_dir.join(&row.bytes_path);
        let bytes = match tokio::fs::read(&abs_path).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(?e, attachment_id = %att_id, path = %abs_path.display(),
                    "chat_attachment disk read failed");
                send_attachment_unreadable(sink, att_id).await?;
                return Ok(());
            }
        };
        attachments.push(crate::agent::AttachmentPayload {
            mime: row.mime,
            bytes,
        });
    }

    // Optimistic-pending emission: push the user's question + an
    // "assistant-pending" placeholder into chat-mode state and
    // broadcast immediately so every connected surface (sender,
    // other Macs, PWA) sees the bubbles before the LLM call
    // starts. The agent reuses the same ids when emitting the
    // final reply — clients' applyItemsUpdate merges by id so
    // the placeholder transitions to the real answer in place.
    let q_id = format!("chat-q-{}", uuid::Uuid::new_v4());
    let a_id = format!("chat-a-{}", uuid::Uuid::new_v4());
    let user_item = Item {
        id: q_id.clone(),
        text: trimmed.clone(),
        detail: None,
        t: 0,
        meta: Some(user_chat_meta(&attachment_ids)),
    };
    let pending_item = Item {
        id: a_id.clone(),
        text: String::new(),
        detail: None,
        t: 0,
        meta: Some(serde_json::json!({"role": "assistant-pending"})),
    };
    let pair = vec![user_item.clone(), pending_item.clone()];
    {
        let mut s = handle.sessions.lock().await;
        let user_state = s.user_mut(user_id);
        user_state.push_item_for_mode("chat", user_item);
        user_state.push_item_for_mode("chat", pending_item);
    }
    // Durable on purpose: this ItemsUpdate carries the user's question
    // row (the assistant-pending placeholder is filtered out by the
    // writer's is_pending_chat_item guard, exactly as before).
    handle
        .bus
        .emit(
            user_id.to_string(),
            Event::ItemsUpdate {
                mode: "chat".into(),
                items: pair,
            },
        )
        .await;

    let _ = handle.agent_kick_tx.send(crate::agent::AgentKick {
        user_id: user_id.to_string(),
        reason: crate::agent::AgentKickReason::ChatMessage {
            text: trimmed,
            attachments,
            q_id,
            a_id,
        },
    });
    Ok(())
}

/// `Intent::ExpandItem` — same agent-kick pattern as chat, but
/// looks up an item by id first (it may live in any mode) and packs
/// `(mode, item_id, item_text)` into the kick payload so the
/// agent's prompt knows which item it's expanding on. The agent's
/// text reply lands as the item's `detail` via `Event::ItemUpdated`.
pub(super) async fn handle_expand(
    handle: &ServerHandle,
    user_id: &str,
    sink: &mut WsSender,
    item_id: String,
) -> Result<()> {
    let lookup = {
        let s = handle.sessions.lock().await;
        s.user(user_id).and_then(|u| u.find_item_by_id(&item_id))
    };
    let Some((mode, item_text)) = lookup else {
        send_error(
            sink,
            "unknown_item",
            &format!("item '{item_id}' not found in any mode"),
            Some(&item_id),
        )
        .await?;
        return Ok(());
    };
    let _ = handle.agent_kick_tx.send(crate::agent::AgentKick {
        user_id: user_id.to_string(),
        reason: crate::agent::AgentKickReason::ExpandItem {
            mode,
            item_id,
            item_text,
        },
    });
    Ok(())
}

/// Build the `meta` for the optimistic user chat bubble. Always carries
/// `role: "user"`; when the message rode screenshots, also carries
/// `attachment_ids` (the resolved, ownership-checked ids — by the time
/// this runs, every id has passed the lookup loop above). Storing the
/// ids (not just a count) future-proofs the wire: every surface can show
/// "N attached" today and fetch/render the actual images later without a
/// second format change. Omitted entirely when there are no attachments
/// so plain messages keep the minimal `{"role":"user"}` shape.
fn user_chat_meta(attachment_ids: &[String]) -> serde_json::Value {
    let mut meta = serde_json::Map::new();
    meta.insert("role".into(), serde_json::Value::String("user".into()));
    if !attachment_ids.is_empty() {
        meta.insert(
            "attachment_ids".into(),
            serde_json::Value::Array(
                attachment_ids
                    .iter()
                    .cloned()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
    }
    serde_json::Value::Object(meta)
}

async fn send_no_active_meeting(sink: &mut WsSender) -> Result<()> {
    send_error(
        sink,
        "no_active_meeting",
        "Chat is only available during an active meeting",
        None,
    )
    .await
}

async fn send_attachment_not_found(sink: &mut WsSender, att_id: &str) -> Result<()> {
    send_error(
        sink,
        "chat_attachment_not_found",
        &format!("chat attachment '{att_id}' not found"),
        Some(att_id),
    )
    .await
}

async fn send_attachment_unreadable(sink: &mut WsSender, att_id: &str) -> Result<()> {
    send_error(
        sink,
        "chat_attachment_unreadable",
        &format!("chat attachment '{att_id}' unreadable"),
        Some(att_id),
    )
    .await
}

async fn send_attachment_forbidden(sink: &mut WsSender, att_id: &str) -> Result<()> {
    send_error(
        sink,
        "chat_attachment_forbidden",
        &format!("chat attachment '{att_id}' does not belong to this meeting"),
        Some(att_id),
    )
    .await
}

async fn send_attachment_error(sink: &mut WsSender, att_id: Option<&str>, msg: &str) -> Result<()> {
    send_error(sink, "chat_attachment_unreadable", msg, att_id).await
}

async fn send_error(
    sink: &mut WsSender,
    code: &str,
    message: &str,
    intent_ref: Option<&str>,
) -> Result<()> {
    let err = Event::Error {
        code: code.into(),
        message: message.into(),
        intent_ref: intent_ref.map(|s| s.to_string()),
    };
    sink.send(Message::Text(serde_json::to_string(&err)?))
        .await
        .ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::user_chat_meta;

    #[test]
    fn meta_omits_attachment_ids_when_none() {
        let m = user_chat_meta(&[]);
        assert_eq!(m["role"], "user");
        assert!(m.get("attachment_ids").is_none());
    }

    #[test]
    fn meta_includes_attachment_ids_when_present() {
        let m = user_chat_meta(&["att-1".to_string(), "att-2".to_string()]);
        assert_eq!(m["role"], "user");
        assert_eq!(m["attachment_ids"], serde_json::json!(["att-1", "att-2"]));
    }
}
