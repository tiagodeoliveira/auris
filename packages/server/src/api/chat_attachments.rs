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

/// Maps an accepted image content-type (already stripped of params)
/// to its (canonical mime, file extension). `None` = unsupported.
/// v1 accepts PNG (Mac screenshots) and JPEG (mobile camera photos).
fn accepted_image(mime_main: &str) -> Option<(&'static str, &'static str)> {
    if mime_main.eq_ignore_ascii_case("image/png") {
        Some(("image/png", "png"))
    } else if mime_main.eq_ignore_ascii_case("image/jpeg") {
        Some(("image/jpeg", "jpg"))
    } else {
        None
    }
}

/// `POST /meetings/:id/chat_attachments` — raw image upload that stages
/// an image for inclusion in the next `Intent::Chat`. Body is raw image/png (Mac) or image/jpeg (mobile); the response carries the assigned attachment id.
/// Bytes land at `<data_dir>/blobs/meetings/<id>/chat/<aid>.{png,jpg}`,
/// parallel to moments' `screenshots/` subdir.
pub(crate) async fn upload_chat_attachment(
    State(state): State<ApiState>,
    Path(meeting_id): Path<String>,
    headers: HeaderMap,
    bytes: axum::body::Bytes,
) -> Result<(StatusCode, Json<UploadChatAttachmentResponse>), ApiError> {
    let user_id = require_user(&headers, &state).await?;

    // Mime: image/png (Mac) or image/jpeg (mobile) in v1, case-insensitive.
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mime_main = mime.split(';').next().unwrap_or("").trim();
    let (canonical_mime, ext) = accepted_image(mime_main).ok_or_else(|| {
        ApiError::BadRequest(format!(
            "only image/png and image/jpeg are supported in v1 (got {mime_main:?})"
        ))
    })?;

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
    let rel = format!("blobs/meetings/{meeting_id}/chat/{attachment_id}.{ext}");
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
        canonical_mime,
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

#[cfg(test)]
mod tests {
    use super::accepted_image;

    #[test]
    fn accepts_png_and_jpeg() {
        assert_eq!(accepted_image("image/png"), Some(("image/png", "png")));
        assert_eq!(accepted_image("image/jpeg"), Some(("image/jpeg", "jpg")));
    }

    #[test]
    fn accepts_case_insensitively() {
        assert_eq!(accepted_image("IMAGE/JPEG"), Some(("image/jpeg", "jpg")));
    }

    #[test]
    fn rejects_unsupported_types() {
        assert_eq!(accepted_image("image/gif"), None);
        assert_eq!(accepted_image("application/pdf"), None);
        assert_eq!(accepted_image(""), None);
    }
}
