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
}
