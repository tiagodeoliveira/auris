use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
};

use super::{require_user, ApiError, ApiState};

/// `DELETE /moments/:moment_id` — drop a single moment row. Best-
/// effort screenshot cleanup runs after the DB delete; if it fails
/// we still return 204 (the row is gone, the file is an orphan).
pub(crate) async fn delete_moment(
    State(state): State<ApiState>,
    Path(moment_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    let result = crate::storage::moments::delete_moment_for_user(&state.db, &moment_id, &user_id)
        .await
        .map_err(|e| ApiError::Db(super::downcast_db(e)))?;
    let Some(asset_path) = result else {
        return Err(ApiError::NotFound);
    };
    if let Some(rel) = asset_path {
        if let Ok(dir) = crate::storage::data_dir() {
            let abs = dir.join(&rel);
            if abs.exists() {
                if let Err(e) = tokio::fs::remove_file(&abs).await {
                    tracing::warn!(
                        error = ?e, moment_id = %moment_id, path = %abs.display(),
                        "delete_moment: screenshot cleanup failed (row already removed)"
                    );
                }
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /meetings/:id/moments/:moment_id/screenshot` — serves the
/// PNG bytes from disk. 404 when the row has no `asset_path` or
/// the file is missing (would happen if `<DATA_DIR>` was wiped).
pub(crate) async fn get_moment_screenshot(
    State(state): State<ApiState>,
    Path((meeting_id, moment_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    // Join to meetings so we only return screenshots the caller owns.
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        r#"SELECT m.meeting_id, m.asset_path
             FROM moments m
             JOIN meetings me ON me.id = m.meeting_id
            WHERE m.id = $1 AND me.user_id = $2"#,
    )
    .bind(&moment_id)
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::Db)?;
    let Some((row_meeting, Some(asset))) = row else {
        return Err(ApiError::NotFound);
    };
    if row_meeting != meeting_id {
        // Path-mismatch: the moment exists but under a different
        // meeting. Treat as not-found rather than leaking shape.
        return Err(ApiError::NotFound);
    }
    let dir = crate::storage::data_dir().map_err(|e| ApiError::Internal(e.to_string()))?;
    let abs = dir.join(&asset);
    let bytes = match tokio::fs::read(&abs).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ApiError::NotFound),
        Err(e) => return Err(ApiError::Internal(format!("read screenshot: {e}"))),
    };
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/png")
        .header(header::CACHE_CONTROL, "private, max-age=86400")
        .body(Body::from(bytes))
        .unwrap())
}

/// `POST /meetings/:id/moments/:moment_id/screenshot` — late-binding
/// screenshot upload for moments created via the WS `mark_moment`
/// intent. The Mac with `screen_capture` capability that's bound as
/// the audio source receives `Event::CaptureMomentScreenshot` and
/// posts the resulting PNG here. Body is raw `image/png` (the same
/// shape as a multipart `screenshot` field, but without the multipart
/// envelope — keeps the upload small and the client-side encode trivial).
pub(crate) async fn upload_moment_screenshot(
    State(state): State<ApiState>,
    Path((meeting_id, moment_id)): Path<(String, String)>,
    headers: HeaderMap,
    bytes: axum::body::Bytes,
) -> Result<StatusCode, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    if bytes.is_empty() {
        return Err(ApiError::BadRequest("empty screenshot body".into()));
    }
    // Confirm the (meeting, moment) pair exists *and* belongs to
    // this user — otherwise reject with 404 (don't leak ownership).
    let row: Option<(String,)> = sqlx::query_as(
        r#"SELECT m.meeting_id
             FROM moments m
             JOIN meetings me ON me.id = m.meeting_id
            WHERE m.id = $1 AND me.user_id = $2"#,
    )
    .bind(&moment_id)
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::Db)?;
    let Some((row_meeting,)) = row else {
        return Err(ApiError::NotFound);
    };
    if row_meeting != meeting_id {
        return Err(ApiError::NotFound);
    }

    let rel = format!("blobs/meetings/{meeting_id}/screenshots/{moment_id}.png");
    let dir = crate::storage::data_dir().map_err(|e| ApiError::Internal(e.to_string()))?;
    let abs = dir.join(&rel);
    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::Internal(format!("mkdir: {e}")))?;
    }
    tokio::fs::write(&abs, &bytes)
        .await
        .map_err(|e| ApiError::Internal(format!("write screenshot: {e}")))?;

    crate::storage::moments::update_moment_asset_path(&state.db, &moment_id, &rel)
        .await
        .map_err(|e| ApiError::Db(super::downcast_db(e)))?;

    Ok(StatusCode::NO_CONTENT)
}
