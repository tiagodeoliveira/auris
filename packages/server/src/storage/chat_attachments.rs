//! Chat-attachment persistence: images uploaded mid-chat that the
//! agent receives as inline image content.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// Row shape for `chat_attachments`. `bytes_path` is relative to
/// `data_dir()` (e.g. `blobs/meetings/<mid>/chat/<aid>.png`).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChatAttachmentRow {
    pub id: String,
    pub meeting_id: String,
    pub user_id: String,
    pub mime: String,
    pub bytes_path: String,
    pub bytes_size: i64,
    pub created_at: DateTime<Utc>,
}

/// Insert a new chat-attachment row. Caller is responsible for having
/// already written the bytes to disk at `bytes_path` (relative to
/// `data_dir()`).
pub async fn insert_chat_attachment(
    pool: &PgPool,
    id: &str,
    meeting_id: &str,
    user_id: &str,
    mime: &str,
    bytes_path: &str,
    bytes_size: i64,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO chat_attachments (id, meeting_id, user_id, mime, bytes_path, bytes_size)
           VALUES ($1, $2, $3, $4, $5, $6)"#,
    )
    .bind(id)
    .bind(meeting_id)
    .bind(user_id)
    .bind(mime)
    .bind(bytes_path)
    .bind(bytes_size)
    .execute(pool)
    .await
    .with_context(|| format!("insert_chat_attachment(id={id})"))?;
    Ok(())
}

/// Fetch a chat-attachment row by id. Returns `None` if unknown. The
/// caller is responsible for verifying `meeting_id` + `user_id` match
/// the current chat context (the WS handler does this).
pub async fn get_chat_attachment(pool: &PgPool, id: &str) -> Result<Option<ChatAttachmentRow>> {
    let row = sqlx::query_as::<_, ChatAttachmentRow>(
        r#"SELECT id, meeting_id, user_id, mime, bytes_path, bytes_size, created_at
             FROM chat_attachments WHERE id = $1"#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .with_context(|| format!("get_chat_attachment(id={id})"))?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::meetings::{delete_meeting_for_user, insert_meeting};
    use crate::storage::users::upsert_user_by_auth0_sub;

    async fn test_user(pool: &PgPool) -> String {
        let sub = format!("test|{}", uuid::Uuid::new_v4());
        upsert_user_by_auth0_sub(pool, &sub, None, None)
            .await
            .unwrap()
            .id
    }

    #[sqlx::test]
    async fn insert_chat_attachment_round_trips(pool: PgPool) {
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, Utc::now(), None, "{}", None)
            .await
            .unwrap();
        let aid = uuid::Uuid::new_v4().to_string();

        insert_chat_attachment(
            &pool,
            &aid,
            &mid,
            &uid,
            "image/png",
            "blobs/meetings/m/chat/att.png",
            12345,
        )
        .await
        .unwrap();

        let got = get_chat_attachment(&pool, &aid)
            .await
            .unwrap()
            .expect("row exists");
        assert_eq!(got.id, aid);
        assert_eq!(got.meeting_id, mid);
        assert_eq!(got.user_id, uid);
        assert_eq!(got.mime, "image/png");
        assert_eq!(got.bytes_path, "blobs/meetings/m/chat/att.png");
        assert_eq!(got.bytes_size, 12345);
    }

    #[sqlx::test]
    async fn get_missing_chat_attachment_returns_none(pool: PgPool) {
        let got = get_chat_attachment(&pool, &uuid::Uuid::new_v4().to_string())
            .await
            .unwrap();
        assert!(got.is_none());
    }

    #[sqlx::test]
    async fn chat_attachment_cascade_deletes_with_meeting(pool: PgPool) {
        let uid = test_user(&pool).await;
        let mid = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &mid, &uid, Utc::now(), None, "{}", None)
            .await
            .unwrap();
        let aid = uuid::Uuid::new_v4().to_string();
        insert_chat_attachment(
            &pool,
            &aid,
            &mid,
            &uid,
            "image/png",
            "blobs/meetings/m/chat/att.png",
            1,
        )
        .await
        .unwrap();

        let deleted = delete_meeting_for_user(&pool, &mid, &uid).await.unwrap();
        assert!(deleted);

        let got = get_chat_attachment(&pool, &aid).await.unwrap();
        assert!(
            got.is_none(),
            "cascade should have removed the attachment row"
        );
    }
}
