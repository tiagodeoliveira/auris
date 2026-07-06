//! Moment persistence: insert, update, delete, and list moments.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// Insert a moment for the given meeting. `t` is the millisecond
/// offset from meeting start — clients that can't compute it send
/// 0 and `UserSession::handle_mark_moment` resolves the real
/// offset before this insert runs, so a literal 0 here means the
/// moment really was marked at meeting start.
/// `kind` is the discriminator for the
/// moment-creation mode ("manual" today; future modes might use
/// "interview" etc.) — the async summary worker dispatches on it.
/// `id` is supplied by the caller so client-side workflows can
/// pre-mint and use the id for the screenshot path before the
/// row exists; pass a freshly-minted UUID if there's no preference.
pub async fn insert_moment(
    pool: &PgPool,
    id: &str,
    meeting_id: &str,
    kind: &str,
    t: i64,
    note: Option<&str>,
    asset_path: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO moments (id, meeting_id, kind, t, note, asset_path)
           VALUES ($1, $2, $3, $4, $5, $6)"#,
    )
    .bind(id)
    .bind(meeting_id)
    .bind(kind)
    .bind(t)
    .bind(note)
    .bind(asset_path)
    .execute(pool)
    .await
    .with_context(|| format!("insert_moment(id={id}, meeting={meeting_id}, t={t})"))?;
    Ok(())
}

/// Replace `summary` + flip `summary_status` to `done` (or `failed`).
/// Called by the moment-summary worker after the LLM round trip.
pub async fn update_moment_summary(
    pool: &PgPool,
    moment_id: &str,
    summary: Option<&str>,
    status: &str,
) -> Result<()> {
    sqlx::query(
        r#"UPDATE moments
              SET summary = $1, summary_status = $2
            WHERE id = $3"#,
    )
    .bind(summary)
    .bind(status)
    .bind(moment_id)
    .execute(pool)
    .await
    .with_context(|| format!("update_moment_summary(id={moment_id})"))?;
    Ok(())
}

/// Delete a single moment row only if its owning meeting belongs
/// to `user_id`. Returns `Ok(Some(asset_path))` on success (with
/// the screenshot path the caller should remove from disk),
/// `Ok(None)` if the row didn't exist *or* the meeting belongs to
/// another user (API surfaces both as 404).
pub async fn delete_moment_for_user(
    pool: &PgPool,
    moment_id: &str,
    user_id: &str,
) -> Result<Option<Option<String>>> {
    // Read asset_path + ownership in one shot so we can fail-fast
    // without locking, and so the caller knows what file to remove.
    let asset_path: Option<Option<String>> = sqlx::query_scalar(
        r#"SELECT m.asset_path
             FROM moments m
             JOIN meetings me ON me.id = m.meeting_id
            WHERE m.id = $1 AND me.user_id = $2"#,
    )
    .bind(moment_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("delete_moment_for_user.read({moment_id})"))?;
    if asset_path.is_none() {
        return Ok(None);
    }
    let res = sqlx::query(
        r#"DELETE FROM moments
            WHERE id = $1
              AND meeting_id IN (SELECT id FROM meetings WHERE user_id = $2)"#,
    )
    .bind(moment_id)
    .bind(user_id)
    .execute(pool)
    .await
    .with_context(|| format!("delete_moment_for_user({moment_id})"))?;
    if res.rows_affected() == 0 {
        return Ok(None);
    }
    Ok(asset_path)
}

/// Set or replace a moment's `asset_path`. Used by the late-binding
/// screenshot upload endpoint that lands an image after a WS-initiated
/// `mark_moment` already created the row.
pub async fn update_moment_asset_path(
    pool: &PgPool,
    moment_id: &str,
    asset_path: &str,
) -> Result<()> {
    sqlx::query(r#"UPDATE moments SET asset_path = $1 WHERE id = $2"#)
        .bind(asset_path)
        .bind(moment_id)
        .execute(pool)
        .await
        .with_context(|| format!("update_moment_asset_path(id={moment_id})"))?;
    Ok(())
}

/// Row shape for `list_moments_for_meeting`. `asset_path` is the
/// relative path under `<DATA_DIR>/blobs/...` (or NULL); the REST
/// endpoint maps it to a `/screenshot` URL clients can fetch.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MomentRow {
    pub id: String,
    pub meeting_id: String,
    pub kind: String,
    pub t: i64,
    pub note: Option<String>,
    pub asset_path: Option<String>,
    pub summary: Option<String>,
    pub summary_status: String,
    pub created_at: DateTime<Utc>,
}

/// List moments for a meeting, oldest first (`t ASC` so the order
/// matches the meeting's natural timeline).
pub async fn list_moments_for_meeting(pool: &PgPool, meeting_id: &str) -> Result<Vec<MomentRow>> {
    let rows = sqlx::query_as::<_, MomentRow>(
        r#"SELECT id, meeting_id, kind, t, note, asset_path,
                  summary, summary_status, created_at
             FROM moments
            WHERE meeting_id = $1
            ORDER BY t ASC"#,
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("list_moments_for_meeting({meeting_id})"))?;
    Ok(rows)
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
    async fn insert_moment_links_to_meeting(pool: PgPool) {
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, Utc::now(), None, "{}", None)
            .await
            .unwrap();

        let moment_id = uuid::Uuid::new_v4().to_string();
        insert_moment(
            &pool,
            &moment_id,
            &mid,
            "manual",
            1500,
            Some("breakthrough"),
            Some("blobs/meetings/X/screenshots/Y.png"),
        )
        .await
        .unwrap();

        let row: (String, String, i64, Option<String>, Option<String>, String) = sqlx::query_as(
            r#"SELECT meeting_id, kind, t, note, asset_path, summary_status
                 FROM moments WHERE id = $1"#,
        )
        .bind(&moment_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, mid);
        assert_eq!(row.1, "manual");
        assert_eq!(row.2, 1500);
        assert_eq!(row.3.as_deref(), Some("breakthrough"));
        assert_eq!(row.4.as_deref(), Some("blobs/meetings/X/screenshots/Y.png"));
        assert_eq!(row.5, "pending", "summary should start as pending");
    }

    #[sqlx::test]
    async fn moment_fk_blocks_orphan_inserts(pool: PgPool) {
        let id = uuid::Uuid::new_v4().to_string();
        let res = insert_moment(&pool, &id, "no-such-meeting", "manual", 0, None, None).await;
        assert!(res.is_err(), "expected FK violation on orphan moment");
    }

    #[sqlx::test]
    async fn update_moment_summary_round_trips(pool: PgPool) {
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, Utc::now(), None, "{}", None)
            .await
            .unwrap();
        let moment_id = uuid::Uuid::new_v4().to_string();
        insert_moment(&pool, &moment_id, &mid, "manual", 0, None, None)
            .await
            .unwrap();

        update_moment_summary(&pool, &moment_id, Some("Summary text"), "done")
            .await
            .unwrap();

        let row: (Option<String>, String) =
            sqlx::query_as("SELECT summary, summary_status FROM moments WHERE id = $1")
                .bind(&moment_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0.as_deref(), Some("Summary text"));
        assert_eq!(row.1, "done");
    }

    #[sqlx::test]
    async fn list_moments_returns_oldest_first(pool: PgPool) {
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, Utc::now(), None, "{}", None)
            .await
            .unwrap();
        let later = uuid::Uuid::new_v4().to_string();
        let earlier = uuid::Uuid::new_v4().to_string();
        // Insert later first to confirm ordering by `t` (not insert order).
        insert_moment(&pool, &later, &mid, "manual", 5000, None, None)
            .await
            .unwrap();
        insert_moment(&pool, &earlier, &mid, "manual", 1000, None, None)
            .await
            .unwrap();
        let rows = list_moments_for_meeting(&pool, &mid).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, earlier);
        assert_eq!(rows[1].id, later);
    }
}
