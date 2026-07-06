//! Artifact persistence: user-uploaded reference files and their
//! meeting attachment join records.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// Row shape for the artifacts library. `asset_path` is relative to
/// `<DATA_DIR>/blobs/`. `short_summary` / `long_summary` populate
/// async after upload — until then `summary_status` is `pending` and
/// the artifact can't be attached to meetings.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ArtifactRow {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub mime_type: String,
    pub asset_path: String,
    pub short_summary: Option<String>,
    pub long_summary: Option<String>,
    pub summary_status: String,
    pub size_bytes: i64,
    pub created_at: DateTime<Utc>,
}

/// Insert a fresh artifact row owned by `user_id`. Summaries land
/// later via `update_artifact_summaries` once the async worker
/// completes; the row starts with `summary_status='pending'`.
pub async fn insert_artifact(
    pool: &PgPool,
    id: &str,
    user_id: &str,
    name: &str,
    mime_type: &str,
    asset_path: &str,
    size_bytes: i64,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO artifacts
               (id, user_id, name, mime_type, asset_path, size_bytes)
           VALUES ($1, $2, $3, $4, $5, $6)"#,
    )
    .bind(id)
    .bind(user_id)
    .bind(name)
    .bind(mime_type)
    .bind(asset_path)
    .bind(size_bytes)
    .execute(pool)
    .await
    .with_context(|| format!("insert_artifact({id})"))?;
    Ok(())
}

/// List one user's artifact library, newest first. Used by the Mac
/// Settings → Artifacts tab and the PWA artifacts modal.
pub async fn list_artifacts_for_user(pool: &PgPool, user_id: &str) -> Result<Vec<ArtifactRow>> {
    let rows = sqlx::query_as::<_, ArtifactRow>(
        r#"SELECT id, user_id, name, mime_type, asset_path,
                  short_summary, long_summary, summary_status,
                  size_bytes, created_at
             FROM artifacts
            WHERE user_id = $1
            ORDER BY created_at DESC"#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("list_artifacts_for_user({user_id})"))?;
    Ok(rows)
}

/// Look up one artifact, ownership-checked against `user_id` so a
/// rogue id from another user can't leak. Returns `None` if the row
/// doesn't exist OR isn't owned by the caller.
pub async fn get_artifact_for_user(
    pool: &PgPool,
    id: &str,
    user_id: &str,
) -> Result<Option<ArtifactRow>> {
    let row = sqlx::query_as::<_, ArtifactRow>(
        r#"SELECT id, user_id, name, mime_type, asset_path,
                  short_summary, long_summary, summary_status,
                  size_bytes, created_at
             FROM artifacts
            WHERE id = $1 AND user_id = $2"#,
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("get_artifact_for_user({id})"))?;
    Ok(row)
}

/// Populate the two summary fields and flip `summary_status`. Called
/// by the async summary worker once both summaries are produced.
/// `status` is one of `'done'` or `'failed'`.
pub async fn update_artifact_summaries(
    pool: &PgPool,
    id: &str,
    short_summary: &str,
    long_summary: &str,
    status: &str,
) -> Result<()> {
    sqlx::query(
        r#"UPDATE artifacts
              SET short_summary = $1,
                  long_summary  = $2,
                  summary_status = $3
            WHERE id = $4"#,
    )
    .bind(short_summary)
    .bind(long_summary)
    .bind(status)
    .bind(id)
    .execute(pool)
    .await
    .with_context(|| format!("update_artifact_summaries({id})"))?;
    Ok(())
}

/// Delete one artifact; ownership-checked. Cascade drops any
/// `meeting_artifacts` rows for it. Caller is responsible for the
/// blob on disk — the DB doesn't know the `<DATA_DIR>` root.
pub async fn delete_artifact_for_user(pool: &PgPool, id: &str, user_id: &str) -> Result<()> {
    sqlx::query(r#"DELETE FROM artifacts WHERE id = $1 AND user_id = $2"#)
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await
        .with_context(|| format!("delete_artifact_for_user({id})"))?;
    Ok(())
}

/// Attach an artifact to a meeting. FK-checked: both rows must
/// exist. Idempotent — re-attaching the same artifact silently
/// no-ops via `ON CONFLICT DO NOTHING`. Mid-meeting picker UX
/// relies on this so the user doesn't have to track what's
/// already attached.
pub async fn attach_artifact_to_meeting(
    pool: &PgPool,
    meeting_id: &str,
    artifact_id: &str,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO meeting_artifacts (meeting_id, artifact_id)
           VALUES ($1, $2)
           ON CONFLICT (meeting_id, artifact_id) DO NOTHING"#,
    )
    .bind(meeting_id)
    .bind(artifact_id)
    .execute(pool)
    .await
    .with_context(|| format!("attach_artifact_to_meeting({meeting_id}, {artifact_id})"))?;
    Ok(())
}

/// Detach one artifact from one meeting. No-op if the join row
/// already isn't there.
pub async fn detach_artifact_from_meeting(
    pool: &PgPool,
    meeting_id: &str,
    artifact_id: &str,
) -> Result<()> {
    sqlx::query(
        r#"DELETE FROM meeting_artifacts
            WHERE meeting_id = $1 AND artifact_id = $2"#,
    )
    .bind(meeting_id)
    .bind(artifact_id)
    .execute(pool)
    .await
    .with_context(|| format!("detach_artifact_from_meeting({meeting_id}, {artifact_id})"))?;
    Ok(())
}

/// List artifacts attached to one meeting, attach order. Joined
/// against `artifacts` so the caller gets the full row in one trip.
pub async fn list_artifacts_for_meeting(
    pool: &PgPool,
    meeting_id: &str,
) -> Result<Vec<ArtifactRow>> {
    let rows = sqlx::query_as::<_, ArtifactRow>(
        r#"SELECT a.id, a.user_id, a.name, a.mime_type, a.asset_path,
                  a.short_summary, a.long_summary, a.summary_status,
                  a.size_bytes, a.created_at
             FROM artifacts a
             JOIN meeting_artifacts ma ON ma.artifact_id = a.id
            WHERE ma.meeting_id = $1
            ORDER BY ma.attached_at ASC"#,
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("list_artifacts_for_meeting({meeting_id})"))?;
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
    async fn insert_artifact_round_trips(pool: PgPool) {
        let uid = test_user(&pool).await;
        let aid = uuid::Uuid::new_v4().to_string();
        insert_artifact(
            &pool,
            &aid,
            &uid,
            "agenda.md",
            "text/markdown",
            "artifacts/u/x.md",
            1234,
        )
        .await
        .unwrap();
        let row = get_artifact_for_user(&pool, &aid, &uid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.id, aid);
        assert_eq!(row.name, "agenda.md");
        assert_eq!(row.mime_type, "text/markdown");
        assert_eq!(row.asset_path, "artifacts/u/x.md");
        assert_eq!(row.size_bytes, 1234);
        assert_eq!(row.summary_status, "pending");
        assert!(row.short_summary.is_none());
        assert!(row.long_summary.is_none());
    }

    #[sqlx::test]
    async fn list_artifacts_for_user_orders_newest_first(pool: PgPool) {
        let uid = test_user(&pool).await;
        let a1 = uuid::Uuid::new_v4().to_string();
        insert_artifact(&pool, &a1, &uid, "first.md", "text/markdown", "p1", 10)
            .await
            .unwrap();
        // Force a later created_at.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let a2 = uuid::Uuid::new_v4().to_string();
        insert_artifact(&pool, &a2, &uid, "second.md", "text/markdown", "p2", 20)
            .await
            .unwrap();
        let rows = list_artifacts_for_user(&pool, &uid).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, a2, "newest first");
        assert_eq!(rows[1].id, a1);
    }

    #[sqlx::test]
    async fn update_artifact_summaries_transitions_status(pool: PgPool) {
        let uid = test_user(&pool).await;
        let aid = uuid::Uuid::new_v4().to_string();
        insert_artifact(&pool, &aid, &uid, "x.md", "text/markdown", "p", 1)
            .await
            .unwrap();
        update_artifact_summaries(&pool, &aid, "short text", "long text", "done")
            .await
            .unwrap();
        let row = get_artifact_for_user(&pool, &aid, &uid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.short_summary.as_deref(), Some("short text"));
        assert_eq!(row.long_summary.as_deref(), Some("long text"));
        assert_eq!(row.summary_status, "done");
    }

    #[sqlx::test]
    async fn delete_artifact_removes_row_and_join(pool: PgPool) {
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, Utc::now(), None, "{}", None)
            .await
            .unwrap();
        let aid = uuid::Uuid::new_v4().to_string();
        insert_artifact(&pool, &aid, &uid, "x.md", "text/markdown", "p", 1)
            .await
            .unwrap();
        attach_artifact_to_meeting(&pool, &mid, &aid).await.unwrap();
        delete_artifact_for_user(&pool, &aid, &uid).await.unwrap();
        // Artifact row gone.
        assert!(get_artifact_for_user(&pool, &aid, &uid)
            .await
            .unwrap()
            .is_none());
        // Cascade dropped the join row too.
        let attached = list_artifacts_for_meeting(&pool, &mid).await.unwrap();
        assert!(attached.is_empty());
    }

    #[sqlx::test]
    async fn attach_artifact_round_trips_through_join(pool: PgPool) {
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, Utc::now(), None, "{}", None)
            .await
            .unwrap();
        let a1 = uuid::Uuid::new_v4().to_string();
        let a2 = uuid::Uuid::new_v4().to_string();
        insert_artifact(&pool, &a1, &uid, "a1.md", "text/markdown", "p1", 1)
            .await
            .unwrap();
        insert_artifact(&pool, &a2, &uid, "a2.md", "text/markdown", "p2", 1)
            .await
            .unwrap();
        attach_artifact_to_meeting(&pool, &mid, &a1).await.unwrap();
        attach_artifact_to_meeting(&pool, &mid, &a2).await.unwrap();
        let attached = list_artifacts_for_meeting(&pool, &mid).await.unwrap();
        assert_eq!(attached.len(), 2);
    }

    #[sqlx::test]
    async fn detach_artifact_removes_join_row(pool: PgPool) {
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, Utc::now(), None, "{}", None)
            .await
            .unwrap();
        let aid = uuid::Uuid::new_v4().to_string();
        insert_artifact(&pool, &aid, &uid, "x.md", "text/markdown", "p", 1)
            .await
            .unwrap();
        attach_artifact_to_meeting(&pool, &mid, &aid).await.unwrap();
        detach_artifact_from_meeting(&pool, &mid, &aid)
            .await
            .unwrap();
        let attached = list_artifacts_for_meeting(&pool, &mid).await.unwrap();
        assert!(attached.is_empty());
        // Artifact row itself stays.
        assert!(get_artifact_for_user(&pool, &aid, &uid)
            .await
            .unwrap()
            .is_some());
    }

    #[sqlx::test]
    async fn attach_to_nonexistent_meeting_fails_fk(pool: PgPool) {
        let uid = test_user(&pool).await;
        let aid = uuid::Uuid::new_v4().to_string();
        insert_artifact(&pool, &aid, &uid, "x.md", "text/markdown", "p", 1)
            .await
            .unwrap();
        let res = attach_artifact_to_meeting(&pool, "no-such-meeting", &aid).await;
        assert!(res.is_err(), "expected FK violation");
    }
}
