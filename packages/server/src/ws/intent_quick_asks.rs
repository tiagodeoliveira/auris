//! Quick-ask intents and broadcast — `Intent::UpsertQuickAsk` and
//! `Intent::DeleteQuickAsk`, plus the canonical-list rebroadcast
//! used after every mutation (and once at connect-time as a
//! synthetic post-snapshot event).
//!
//! Quick-asks are a per-user library of saved prompts (label + full
//! text) clients can fire as "quick chat questions". The library is
//! capped at 50 items per user; labels at 40 chars, texts at 4000.

use anyhow::Result;
use axum::extract::ws::Message;
use futures_util::SinkExt;

use crate::protocol::{Event, Item};

use super::control::WsSender;
use crate::context::ServerHandle;

/// Cap on the user-curated quick-ask library size. Reads in the
/// glasses snippet view get crowded past this; the UI editors warn
/// at this number too.
const QUICK_ASK_LIBRARY_MAX: usize = 50;
/// Max characters in the short mnemonic label. Sized to fit on a
/// single line of the glasses snippet list at the standard font.
const QUICK_ASK_LABEL_MAX: usize = 40;
/// Max characters in the full prompt. ~4kb covers a few paragraphs
/// of multiline / markdown text without going wild.
const QUICK_ASK_TEXT_MAX: usize = 4000;

/// Convert a DB row to the Item shape used by the wire `items_update`
/// event. Label goes in `text` (the displayed value); the full prompt
/// goes in `detail` (sent to chat when the user picks); position is
/// repurposed as the `t` field for ordering.
fn quick_ask_row_to_item(row: crate::storage::QuickAskRow) -> Item {
    Item {
        id: row.id,
        text: row.label,
        detail: Some(row.text),
        t: row.position.max(0) as u64,
        meta: None,
    }
}

/// Reload the user's quick-ask library from the DB into the in-memory
/// `items_per_mode["quick_asks"]` and broadcast the canonical list to
/// every connection for this user. Called after every Upsert / Delete
/// and once at connect-time as a synthetic post-snapshot event.
pub(super) async fn broadcast_quick_asks(handle: &ServerHandle, user_id: &str) {
    let rows = match crate::storage::quick_ask::list_quick_asks_for_user(&handle.db, user_id).await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(?e, "list_quick_asks_for_user failed during broadcast");
            return;
        }
    };
    let items: Vec<Item> = rows.into_iter().map(quick_ask_row_to_item).collect();
    {
        let mut s = handle.sessions.lock().await;
        s.user_mut(user_id)
            .set_items_for_mode("quick_asks", items.clone());
    }
    // Fan-out only: quick_asks is the user's persistent library, whose
    // source of truth is the `quick_ask` table (reloaded from the DB
    // just above). This ItemsUpdate is a pure UI sync — it must NOT
    // ride the durable lane, where `persist_items_update` would either
    // skip it (no active meeting) or wrongly write the meeting-
    // independent library into a meeting's items rows.
    handle.bus.emit_fanout_only(
        user_id.to_string(),
        Event::ItemsUpdate {
            mode: "quick_asks".into(),
            items,
        },
    );
}

pub(super) async fn handle_upsert(
    handle: &ServerHandle,
    user_id: &str,
    sink: &mut WsSender,
    id: String,
    label: String,
    text: String,
    position: i32,
) -> Result<()> {
    let label = label.trim().to_string();
    let text = text.trim().to_string();
    if label.is_empty()
        || label.len() > QUICK_ASK_LABEL_MAX
        || text.is_empty()
        || text.len() > QUICK_ASK_TEXT_MAX
    {
        send_error(
            sink,
            "invalid_quick_ask",
            &format!(
                "label must be 1..={QUICK_ASK_LABEL_MAX} chars, text must be 1..={QUICK_ASK_TEXT_MAX} chars"
            ),
            Some(&id),
        )
        .await?;
        return Ok(());
    }
    // Library cap. Counted server-side from the canonical DB list.
    let existing =
        match crate::storage::quick_ask::list_quick_asks_for_user(&handle.db, user_id).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(?e, "list_quick_asks_for_user failed");
                send_error(
                    sink,
                    "quick_ask_lookup_failed",
                    "Could not load quick-ask library",
                    Some(&id),
                )
                .await?;
                return Ok(());
            }
        };
    let editing = existing.iter().any(|r| r.id == id);
    if !editing && existing.len() >= QUICK_ASK_LIBRARY_MAX {
        send_error(
            sink,
            "quick_ask_library_full",
            &format!("Maximum {QUICK_ASK_LIBRARY_MAX} quick asks per user"),
            Some(&id),
        )
        .await?;
        return Ok(());
    }
    if let Err(e) = crate::storage::quick_ask::upsert_quick_ask(
        &handle.db, &id, user_id, &label, &text, position,
    )
    .await
    {
        tracing::warn!(?e, "upsert_quick_ask failed");
        send_error(sink, "quick_ask_save_failed", &e.to_string(), Some(&id)).await?;
        return Ok(());
    }
    broadcast_quick_asks(handle, user_id).await;
    Ok(())
}

pub(super) async fn handle_delete(
    handle: &ServerHandle,
    user_id: &str,
    sink: &mut WsSender,
    id: String,
) -> Result<()> {
    if let Err(e) = crate::storage::quick_ask::delete_quick_ask(&handle.db, &id, user_id).await {
        tracing::warn!(?e, "delete_quick_ask failed");
        send_error(sink, "quick_ask_save_failed", &e.to_string(), Some(&id)).await?;
        return Ok(());
    }
    broadcast_quick_asks(handle, user_id).await;
    Ok(())
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
