//! Per-meeting item persistence (highlights, actions, open_questions,
//! summary, chat). Transcript items live in JSONL blobs instead.

use anyhow::{Context, Result};
use sqlx::PgPool;

/// Append one item to its meeting's persisted history. Used by the
/// items-persistence task on every Append-strategy `ItemsUpdate`
/// broadcast (actions / open_questions). `ON CONFLICT DO NOTHING` so
/// the writer is idempotent — a transient retry that re-broadcasts
/// the same id is a no-op.
pub async fn insert_item_row(
    pool: &PgPool,
    meeting_id: &str,
    mode: &str,
    item: &crate::protocol::Item,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO items (id, meeting_id, mode, text, detail, t_ms, meta)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           ON CONFLICT (id) DO NOTHING"#,
    )
    .bind(&item.id)
    .bind(meeting_id)
    .bind(mode)
    .bind(&item.text)
    .bind(&item.detail)
    .bind(item.t as i64)
    .bind(item.meta.as_ref())
    .execute(pool)
    .await
    .with_context(|| format!("insert_item_row({})", item.id))?;
    Ok(())
}

/// Replace-strategy persistence: drop everything for `(meeting_id,
/// mode)` and insert the new set in a single transaction so a crash
/// can never leave a torn snapshot. Used for highlights / summary /
/// chat — modes whose live state is "the current full list, no
/// history."
pub async fn replace_items_for_meeting_mode(
    pool: &PgPool,
    meeting_id: &str,
    mode: &str,
    items: &[crate::protocol::Item],
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(r#"DELETE FROM items WHERE meeting_id = $1 AND mode = $2"#)
        .bind(meeting_id)
        .bind(mode)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("replace_items: delete {meeting_id}/{mode}"))?;
    for item in items {
        // ON CONFLICT (id) DO UPDATE — `items_pkey` is on `id` alone,
        // but the DELETE above only clears the (meeting_id, mode)
        // slice. Two near-concurrent replace flows for the same scope
        // (e.g. a client reconnect replaying a snapshot while the
        // server's own state is also flushing) interleave so that
        // TX2's DELETE doesn't see TX1's not-yet-committed insert,
        // then TX2's INSERT collides on PK once TX1 commits. Last-
        // write-wins matches Replace semantics — the latest payload
        // is by definition the authoritative one.
        sqlx::query(
            r#"INSERT INTO items (id, meeting_id, mode, text, detail, t_ms, meta)
               VALUES ($1, $2, $3, $4, $5, $6, $7)
               ON CONFLICT (id) DO UPDATE SET
                   meeting_id = EXCLUDED.meeting_id,
                   mode       = EXCLUDED.mode,
                   text       = EXCLUDED.text,
                   detail     = EXCLUDED.detail,
                   t_ms       = EXCLUDED.t_ms,
                   meta       = EXCLUDED.meta"#,
        )
        .bind(&item.id)
        .bind(meeting_id)
        .bind(mode)
        .bind(&item.text)
        .bind(&item.detail)
        .bind(item.t as i64)
        .bind(item.meta.as_ref())
        .execute(&mut *tx)
        .await
        .with_context(|| format!("replace_items: upsert {}", item.id))?;
    }
    tx.commit().await?;
    Ok(())
}

/// Update one item's `detail` field in-place. Used by the
/// expand_item flow when the agent's text expansion is ready —
/// the row is keyed by (meeting_id, mode, id). No-op (silent) if
/// the matching row doesn't exist; caller's broadcast already
/// updated in-memory state, so a missing DB row just means the
/// detail won't appear in past-meeting view (rare race).
pub async fn update_item_detail(
    pool: &PgPool,
    meeting_id: &str,
    mode: &str,
    item_id: &str,
    detail: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"UPDATE items
              SET detail = $1
            WHERE id = $2 AND meeting_id = $3 AND mode = $4"#,
    )
    .bind(detail)
    .bind(item_id)
    .bind(meeting_id)
    .bind(mode)
    .execute(pool)
    .await
    .with_context(|| format!("update_item_detail({item_id})"))?;
    Ok(())
}

/// Read every persisted item for a meeting, grouped by mode and
/// ordered by `created_at` within each group. Powers the meeting-
/// detail view's per-mode tabs. Excludes transcript-mode items —
/// those live in the JSONL blob and aren't written to this table
/// at all.
pub async fn list_items_for_meeting_grouped(
    pool: &PgPool,
    meeting_id: &str,
) -> Result<std::collections::HashMap<String, Vec<crate::protocol::Item>>> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        String,
        String,
        String,
        Option<String>,
        i64,
        Option<serde_json::Value>,
    )> = sqlx::query_as(
        r#"SELECT id, mode, text, detail, t_ms, meta
                 FROM items
                WHERE meeting_id = $1
             ORDER BY mode, created_at"#,
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("list_items_for_meeting_grouped({meeting_id})"))?;
    let mut grouped: std::collections::HashMap<String, Vec<crate::protocol::Item>> =
        std::collections::HashMap::new();
    for (id, mode, text, detail, t_ms, meta) in rows {
        grouped
            .entry(mode)
            .or_default()
            .push(crate::protocol::Item {
                id,
                text,
                detail,
                t: t_ms.max(0) as u64,
                meta,
            });
    }
    Ok(grouped)
}

/// Who produced a chat message. `Wearer` (not `User`) matches the
/// vocabulary the extractor prompts use throughout `agent/active.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    Wearer,
    Assistant,
}

/// One chat message, flattened for prompt rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
}

/// Read a meeting's chat in wire order.
///
/// Ordered by `created_at`, NOT `t_ms`: chat rows are written with
/// `t_ms = 0` and carry no usable timestamp. Backed by the existing
/// `idx_items_meeting_mode_created` index.
///
/// Rows whose `meta.role` is missing or unrecognized are skipped — a
/// message we can't attribute is worse than no message, since the
/// prompt grants the wearer's voice authority over the transcript.
pub async fn list_chat_messages_for_meeting(
    pool: &PgPool,
    meeting_id: &str,
) -> Result<Vec<ChatMessage>> {
    let rows: Vec<(String, Option<serde_json::Value>)> = sqlx::query_as(
        r#"SELECT text, meta
                 FROM items
                WHERE meeting_id = $1 AND mode = 'chat'
             ORDER BY created_at"#,
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("list_chat_messages_for_meeting({meeting_id})"))?;

    let mut msgs = Vec::with_capacity(rows.len());
    for (text, meta) in rows {
        let role = meta
            .as_ref()
            .and_then(|m| m.get("role"))
            .and_then(|r| r.as_str());
        let role = match role {
            Some("user") => ChatRole::Wearer,
            Some("assistant") => ChatRole::Assistant,
            other => {
                tracing::debug!(
                    meeting_id,
                    role = ?other,
                    "chat message with unusable role; skipping",
                );
                continue;
            }
        };
        msgs.push(ChatMessage { role, text });
    }
    Ok(msgs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::meetings::insert_meeting;
    use crate::storage::users::upsert_user_by_auth0_sub;

    async fn test_user(pool: &PgPool) -> String {
        let sub = format!("test|{}", uuid::Uuid::new_v4());
        upsert_user_by_auth0_sub(pool, &sub, None, None)
            .await
            .unwrap()
            .id
    }

    #[sqlx::test]
    async fn list_items_grouped_includes_chat_mode(pool: PgPool) {
        // Defense-in-depth for cross-surface-coordination.md Rule 1:
        // past-meeting detail responses must surface chat history under
        // `items_by_mode["chat"]` so mobile + PWA can render the tab.
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, chrono::Utc::now(), None, "{}", None)
            .await
            .unwrap();
        let item = crate::protocol::Item {
            id: uuid::Uuid::new_v4().to_string(),
            text: "what did we agree on?".into(),
            detail: None,
            t: 1000,
            meta: None,
        };
        insert_item_row(&pool, &mid, "chat", &item).await.unwrap();
        let grouped = list_items_for_meeting_grouped(&pool, &mid).await.unwrap();
        let chat = grouped.get("chat").expect("chat key present");
        assert_eq!(chat.len(), 1);
        assert_eq!(chat[0].id, item.id);
    }

    #[sqlx::test]
    async fn replace_items_is_idempotent_under_replay(pool: PgPool) {
        // Reproduces the production duplicate-key bug: a client
        // reconnect replays the same items payload while the server's
        // own state is also flushing. The PK is on `id` alone, but
        // the DELETE in replace_items only clears (meeting_id, mode);
        // a plain INSERT would collide on the second call. With the
        // upsert, the second call is a silent no-op.
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, chrono::Utc::now(), None, "{}", None)
            .await
            .unwrap();
        let items = vec![
            crate::protocol::Item {
                id: "h-1".into(),
                text: "first".into(),
                detail: None,
                t: 0,
                meta: None,
            },
            crate::protocol::Item {
                id: "h-2".into(),
                text: "second".into(),
                detail: None,
                t: 1000,
                meta: None,
            },
        ];
        replace_items_for_meeting_mode(&pool, &mid, "highlights", &items)
            .await
            .unwrap();
        // Second call with the same items must not panic on PK
        // collision — this is the exact scenario that fired in
        // production on every Mac reconnect.
        replace_items_for_meeting_mode(&pool, &mid, "highlights", &items)
            .await
            .expect("replay must succeed without items_pkey violation");
        let grouped = list_items_for_meeting_grouped(&pool, &mid).await.unwrap();
        let hi = grouped.get("highlights").expect("highlights present");
        assert_eq!(hi.len(), 2);
    }

    #[sqlx::test]
    async fn list_chat_messages_returns_roles_in_created_order(pool: PgPool) {
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, chrono::Utc::now(), None, "{}", None)
            .await
            .unwrap();

        for (text, role) in [
            ("his name is Ngoc Tran", "user"),
            ("Got it, using Ngoc Tran.", "assistant"),
            ("also the budget is 40k", "user"),
        ] {
            let item = crate::protocol::Item {
                id: uuid::Uuid::new_v4().to_string(),
                text: text.into(),
                detail: None,
                t: 0,
                meta: Some(serde_json::json!({ "role": role })),
            };
            insert_item_row(&pool, &mid, "chat", &item).await.unwrap();
        }

        let msgs = list_chat_messages_for_meeting(&pool, &mid).await.unwrap();

        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].role, ChatRole::Wearer);
        assert_eq!(msgs[0].text, "his name is Ngoc Tran");
        assert_eq!(msgs[1].role, ChatRole::Assistant);
        assert_eq!(msgs[1].text, "Got it, using Ngoc Tran.");
        assert_eq!(msgs[2].role, ChatRole::Wearer);
        assert_eq!(msgs[2].text, "also the budget is 40k");
    }

    #[sqlx::test]
    async fn list_chat_messages_skips_unusable_roles(pool: PgPool) {
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, chrono::Utc::now(), None, "{}", None)
            .await
            .unwrap();

        // Unknown role, and no meta at all: both are unusable and skipped.
        let bad_role = crate::protocol::Item {
            id: uuid::Uuid::new_v4().to_string(),
            text: "from nowhere".into(),
            detail: None,
            t: 0,
            meta: Some(serde_json::json!({ "role": "system" })),
        };
        insert_item_row(&pool, &mid, "chat", &bad_role)
            .await
            .unwrap();

        let no_meta = crate::protocol::Item {
            id: uuid::Uuid::new_v4().to_string(),
            text: "orphan".into(),
            detail: None,
            t: 0,
            meta: None,
        };
        insert_item_row(&pool, &mid, "chat", &no_meta)
            .await
            .unwrap();

        let keeper = crate::protocol::Item {
            id: uuid::Uuid::new_v4().to_string(),
            text: "real message".into(),
            detail: None,
            t: 0,
            meta: Some(serde_json::json!({ "role": "user" })),
        };
        insert_item_row(&pool, &mid, "chat", &keeper).await.unwrap();

        let msgs = list_chat_messages_for_meeting(&pool, &mid).await.unwrap();

        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "real message");
    }

    #[sqlx::test]
    async fn list_chat_messages_ignores_other_modes_and_meetings(pool: PgPool) {
        let uid = test_user(&pool).await;
        let mine = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mine, &uid, chrono::Utc::now(), None, "{}", None)
            .await
            .unwrap();
        insert_meeting(&pool, &other, &uid, chrono::Utc::now(), None, "{}", None)
            .await
            .unwrap();

        let chat_item = crate::protocol::Item {
            id: uuid::Uuid::new_v4().to_string(),
            text: "mine".into(),
            detail: None,
            t: 0,
            meta: Some(serde_json::json!({ "role": "user" })),
        };
        insert_item_row(&pool, &mine, "chat", &chat_item)
            .await
            .unwrap();

        // A non-chat mode on the same meeting must not leak in.
        let summary_item = crate::protocol::Item {
            id: uuid::Uuid::new_v4().to_string(),
            text: "a summary".into(),
            detail: None,
            t: 0,
            meta: Some(serde_json::json!({ "role": "user" })),
        };
        insert_item_row(&pool, &mine, "summary", &summary_item)
            .await
            .unwrap();

        // Chat on a different meeting must not leak in.
        let other_chat = crate::protocol::Item {
            id: uuid::Uuid::new_v4().to_string(),
            text: "theirs".into(),
            detail: None,
            t: 0,
            meta: Some(serde_json::json!({ "role": "user" })),
        };
        insert_item_row(&pool, &other, "chat", &other_chat)
            .await
            .unwrap();

        let msgs = list_chat_messages_for_meeting(&pool, &mine).await.unwrap();

        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "mine");
    }

    #[sqlx::test]
    async fn list_chat_messages_empty_for_meeting_without_chat(pool: PgPool) {
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, chrono::Utc::now(), None, "{}", None)
            .await
            .unwrap();

        let msgs = list_chat_messages_for_meeting(&pool, &mid).await.unwrap();

        assert!(msgs.is_empty());
    }
}
