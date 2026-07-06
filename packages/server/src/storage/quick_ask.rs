//! Quick-ask persistence: per-user curated chat prompt library.

use anyhow::{Context, Result};
use sqlx::PgPool;

/// Row shape for the per-user quick-ask library. See migration 0008.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct QuickAskRow {
    pub id: String,
    pub user_id: String,
    pub label: String,
    pub text: String,
    pub position: i32,
}

/// List one user's quick-ask library in display order. Loaded once
/// on connect into `items_per_mode["quick_asks"]`; live edits keep
/// the in-memory copy in sync without re-reading the DB.
pub async fn list_quick_asks_for_user(pool: &PgPool, user_id: &str) -> Result<Vec<QuickAskRow>> {
    let rows = sqlx::query_as::<_, QuickAskRow>(
        r#"SELECT id, user_id, label, text, position
             FROM quick_asks
            WHERE user_id = $1
            ORDER BY position ASC"#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("list_quick_asks_for_user({user_id})"))?;
    Ok(rows)
}

/// Upsert by `(id, user_id)` — `id` is client-minted so an edit
/// re-uses the same id; first-write creates the row, subsequent
/// writes update label/text/position in place.
pub async fn upsert_quick_ask(
    pool: &PgPool,
    id: &str,
    user_id: &str,
    label: &str,
    text: &str,
    position: i32,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO quick_asks (id, user_id, label, text, position)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (id) DO UPDATE
              SET label = EXCLUDED.label,
                  text = EXCLUDED.text,
                  position = EXCLUDED.position,
                  updated_at = NOW()
            WHERE quick_asks.user_id = EXCLUDED.user_id"#,
    )
    .bind(id)
    .bind(user_id)
    .bind(label)
    .bind(text)
    .bind(position)
    .execute(pool)
    .await
    .with_context(|| format!("upsert_quick_ask({id})"))?;
    Ok(())
}

/// Delete by `(id, user_id)`. Idempotent — unknown ids return Ok(0).
pub async fn delete_quick_ask(pool: &PgPool, id: &str, user_id: &str) -> Result<()> {
    sqlx::query(
        r#"DELETE FROM quick_asks
            WHERE id = $1 AND user_id = $2"#,
    )
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await
    .with_context(|| format!("delete_quick_ask({id})"))?;
    Ok(())
}
