use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    Json,
};
use serde::Serialize;

use super::{downcast_db, require_user, ApiError, ApiState};

#[derive(Debug, Serialize)]
pub(crate) struct UploadChatAttachmentResponse {
    id: String,
}

/// `POST /meetings/:id/chat_attachments` — raw PNG upload that stages
/// an image for inclusion in the next `Intent::Chat`. Body is raw
/// `image/png`; the response carries the assigned attachment id.
/// Bytes land at `<data_dir>/blobs/meetings/<id>/chat/<aid>.png`,
/// parallel to moments' `screenshots/` subdir.
pub(crate) async fn upload_chat_attachment(
    State(state): State<ApiState>,
    Path(meeting_id): Path<String>,
    headers: HeaderMap,
    bytes: axum::body::Bytes,
) -> Result<(StatusCode, Json<UploadChatAttachmentResponse>), ApiError> {
    let user_id = require_user(&headers, &state).await?;

    // Mime: image/png only in v1 (case-insensitive).
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mime_main = mime.split(';').next().unwrap_or("").trim();
    if !mime_main.eq_ignore_ascii_case("image/png") {
        return Err(ApiError::BadRequest(format!(
            "only image/png is supported in v1 (got {mime_main:?})"
        )));
    }

    if bytes.is_empty() {
        return Err(ApiError::BadRequest("empty attachment body".into()));
    }

    // Ownership: meeting must exist and belong to caller.
    let row: Option<(String,)> =
        sqlx::query_as(r#"SELECT id FROM meetings WHERE id = $1 AND user_id = $2"#)
            .bind(&meeting_id)
            .bind(&user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::Db)?;
    if row.is_none() {
        // 404 covers both "no such meeting" and "owned by someone else"
        // — mirrors the moment-screenshot path's "don't leak existence."
        return Err(ApiError::NotFound);
    }

    let attachment_id = uuid::Uuid::new_v4().to_string();
    let rel = format!("blobs/meetings/{meeting_id}/chat/{attachment_id}.png");
    let dir = crate::storage::data_dir().map_err(|e| ApiError::Internal(e.to_string()))?;
    let abs = dir.join(&rel);
    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::Internal(format!("mkdir: {e}")))?;
    }
    tokio::fs::write(&abs, &bytes)
        .await
        .map_err(|e| ApiError::Internal(format!("write attachment: {e}")))?;

    crate::storage::chat_attachments::insert_chat_attachment(
        &state.db,
        &attachment_id,
        &meeting_id,
        &user_id,
        "image/png",
        &rel,
        bytes.len() as i64,
    )
    .await
    .map_err(|e| ApiError::Db(downcast_db(e)))?;

    Ok((
        StatusCode::CREATED,
        Json(UploadChatAttachmentResponse { id: attachment_id }),
    ))
}
