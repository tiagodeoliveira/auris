/// MIME types we accept for artifact uploads. Matches PLAN.md §3.7
/// "Format support (v1)" — text formats land via rig's `Document`,
/// PDFs via `document_raw`, images via `image_base64`. Anything else
/// is 415.
const ALLOWED_ARTIFACT_MIMES: &[&str] = &[
    "text/plain",
    "text/markdown",
    "text/html",
    "text/csv",
    "application/json",
    "application/pdf",
    "image/png",
    "image/jpeg",
];

/// Cap individual artifact uploads. Personal-use ceiling — most docs
/// fit in MB, not GB; very large PDFs still go through.
const MAX_ARTIFACT_BYTES: usize = 50 * 1024 * 1024;

use axum::{
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;

use super::{downcast_db, require_user, ApiError, ApiState, ArtifactCreated, ArtifactDto};

#[derive(Debug, Deserialize)]
pub(crate) struct AttachArtifactBody {
    artifact_id: String,
}

/// Wrapper for `GET /meetings/:id/artifacts`. The list endpoint hands
/// back a `{ artifacts: [...] }` object instead of a bare array so
/// future fields (pagination cursors, totals) can be added without a
/// breaking client change. Documented in
/// `docs/cross-surface-coordination.md` §"Wire contract additions still
/// needed."
#[derive(Debug, serde::Serialize)]
pub(crate) struct MeetingArtifactsList {
    artifacts: Vec<ArtifactDto>,
}

pub(crate) async fn list_artifacts(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ArtifactDto>>, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    let rows = crate::storage::artifacts::list_artifacts_for_user(&state.db, &user_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    Ok(Json(rows.into_iter().map(ArtifactDto::from).collect()))
}

pub(crate) async fn get_artifact(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ArtifactDto>, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    let row = crate::storage::artifacts::get_artifact_for_user(&state.db, &id, &user_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(ArtifactDto::from(row)))
}

/// Multipart upload. Expects exactly one field named `file` carrying
/// the bytes; `filename` becomes the artifact's display name and
/// `Content-Type` its mime. Returns the freshly-inserted row with
/// `summary_status: pending` — the async summarizer worker (PLAN.md
/// §3.12 step 3d) populates the summaries.
///
/// On any failure after the blob is on disk, we leave it: a future
/// cleanup task can scan for orphan blobs (rows with `summary_status
/// = 'failed'` that never got a paired DB row). For personal-use v1,
/// this is rare enough that a manual rm is fine.
pub(crate) async fn upload_artifact(
    State(state): State<ApiState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<ArtifactDto>, ApiError> {
    let user_id = require_user(&headers, &state).await?;

    let mut file_bytes: Option<axum::body::Bytes> = None;
    let mut file_name: Option<String> = None;
    let mut mime_type: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("multipart: {e}")))?
    {
        let field_name = field.name().unwrap_or("").to_string();
        if field_name != "file" {
            // Ignore unknown fields — don't fail the upload over a
            // stray client-side metadata field.
            continue;
        }
        file_name = field.file_name().map(|s| s.to_string());
        mime_type = field
            .content_type()
            .map(|s| s.to_string())
            .or_else(|| Some("application/octet-stream".to_string()));
        let bytes = field
            .bytes()
            .await
            .map_err(|e| ApiError::BadRequest(format!("read bytes: {e}")))?;
        file_bytes = Some(bytes);
    }

    let bytes = file_bytes.ok_or_else(|| ApiError::BadRequest("missing file field".into()))?;
    let name = file_name.ok_or_else(|| ApiError::BadRequest("missing filename".into()))?;
    let mime = mime_type.ok_or_else(|| ApiError::BadRequest("missing content type".into()))?;
    if bytes.is_empty() {
        return Err(ApiError::BadRequest("empty file".into()));
    }
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(ApiError::BadRequest(format!(
            "file exceeds {} bytes",
            MAX_ARTIFACT_BYTES
        )));
    }
    if !ALLOWED_ARTIFACT_MIMES.iter().any(|m| *m == mime) {
        return Err(ApiError::BadRequest(format!(
            "unsupported mime type: {mime}"
        )));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let rel = format!("blobs/artifacts/{user_id}/{id}");
    let dir = crate::storage::data_dir().map_err(|e| ApiError::Internal(e.to_string()))?;
    let abs = dir.join(&rel);
    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::Internal(format!("mkdir: {e}")))?;
    }
    tokio::fs::write(&abs, &bytes)
        .await
        .map_err(|e| ApiError::Internal(format!("write artifact: {e}")))?;

    let size = bytes.len() as i64;
    crate::storage::artifacts::insert_artifact(&state.db, &id, &user_id, &name, &mime, &rel, size)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;

    let row = crate::storage::artifacts::get_artifact_for_user(&state.db, &id, &user_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?
        .ok_or_else(|| ApiError::Internal("artifact vanished after insert".into()))?;

    // Wake the summary worker. `summary_status` stays `pending`
    // until the worker writes back via `update_artifact_summaries`.
    // Send is `let _ =` because a closed channel (e.g. tests with no
    // worker spawned) shouldn't fail the upload.
    let _ = state.artifact_created_tx.send(ArtifactCreated {
        artifact_id: row.id.clone(),
        user_id: user_id.clone(),
        name: row.name.clone(),
        mime_type: row.mime_type.clone(),
        asset_path: row.asset_path.clone(),
    });

    Ok(Json(ArtifactDto::from(row)))
}

pub(crate) async fn delete_artifact(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    // Read the row first so we know the on-disk path; the DB cascade
    // drops join rows automatically, but we own the blob lifecycle.
    let row = crate::storage::artifacts::get_artifact_for_user(&state.db, &id, &user_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?
        .ok_or(ApiError::NotFound)?;
    crate::storage::artifacts::delete_artifact_for_user(&state.db, &id, &user_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    let dir = crate::storage::data_dir().map_err(|e| ApiError::Internal(e.to_string()))?;
    let abs = dir.join(&row.asset_path);
    if let Err(e) = tokio::fs::remove_file(&abs).await {
        // Best-effort: blob may already be gone (manual cleanup, OS
        // restart wiped /tmp, etc.). DB row is the source of truth.
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(error = ?e, path = %abs.display(), "artifact blob delete failed");
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Re-queue summary generation for an artifact whose previous attempt
/// failed. Resets `summary_status` to `pending` (clearing any stale
/// short/long text) and re-publishes on `artifact_created_tx` so the
/// async summary worker picks it up exactly the way it would on a
/// fresh upload.
///
/// State guard: only `failed` artifacts can be retried.
///   - `pending` is in-flight — retrying would race the worker.
///   - `done` already has a summary — nothing to retry.
///
/// Both yield 400 with a self-describing message; the Mac UI only
/// surfaces the button for `failed` rows, so a 400 here means the
/// state flipped between fetch and click (concurrent retry, or the
/// worker just finished).
pub(crate) async fn retry_artifact_summary(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ArtifactDto>, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    let row = crate::storage::artifacts::get_artifact_for_user(&state.db, &id, &user_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?
        .ok_or(ApiError::NotFound)?;
    if row.summary_status != "failed" {
        return Err(ApiError::BadRequest(format!(
            "artifact summary_status is `{}`; only `failed` artifacts can be retried",
            row.summary_status
        )));
    }
    // Flip back to pending. Empty short/long are correct here —
    // a failed row never wrote useful summary text, and even if it
    // had, the retry is meant to supersede.
    crate::storage::artifacts::update_artifact_summaries(&state.db, &row.id, "", "", "pending")
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    let _ = state.artifact_created_tx.send(ArtifactCreated {
        artifact_id: row.id.clone(),
        user_id: user_id.clone(),
        name: row.name.clone(),
        mime_type: row.mime_type.clone(),
        asset_path: row.asset_path.clone(),
    });
    // Return the freshly-pending row so the client can update local
    // state without a follow-up GET.
    let updated = crate::storage::artifacts::get_artifact_for_user(&state.db, &row.id, &user_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?
        .ok_or_else(|| ApiError::Internal("artifact vanished after retry update".into()))?;
    Ok(Json(ArtifactDto::from(updated)))
}

/// List the artifacts currently attached to one meeting, in attach
/// order. 404 if the meeting is unknown or owned by a different user
/// (same leak-prevention pattern as attach/detach).
pub(crate) async fn list_meeting_artifacts(
    State(state): State<ApiState>,
    Path(meeting_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<MeetingArtifactsList>, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    let meeting_owned: Option<(String,)> =
        sqlx::query_as(r#"SELECT id FROM meetings WHERE id = $1 AND user_id = $2"#)
            .bind(&meeting_id)
            .bind(&user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::Db)?;
    if meeting_owned.is_none() {
        return Err(ApiError::NotFound);
    }
    let rows = crate::storage::artifacts::list_artifacts_for_meeting(&state.db, &meeting_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    let artifacts = rows.into_iter().map(ArtifactDto::from).collect();
    Ok(Json(MeetingArtifactsList { artifacts }))
}

pub(crate) async fn attach_artifact(
    State(state): State<ApiState>,
    Path(meeting_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AttachArtifactBody>,
) -> Result<StatusCode, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    // Belt-and-suspenders ownership: both the meeting and the
    // artifact must belong to this caller. 404 (not 403) on miss so
    // we don't leak existence.
    let meeting_owned: Option<(String,)> =
        sqlx::query_as(r#"SELECT id FROM meetings WHERE id = $1 AND user_id = $2"#)
            .bind(&meeting_id)
            .bind(&user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::Db)?;
    if meeting_owned.is_none() {
        return Err(ApiError::NotFound);
    }
    let artifact =
        crate::storage::artifacts::get_artifact_for_user(&state.db, &body.artifact_id, &user_id)
            .await
            .map_err(|e| ApiError::Db(downcast_db(e)))?
            .ok_or(ApiError::NotFound)?;
    // PLAN.md §3.7 — only `done` artifacts are attachable. Pending
    // ones aren't fully indexed yet; failed ones never will be.
    if artifact.summary_status != "done" {
        return Err(ApiError::BadRequest(format!(
            "artifact summary_status is {}; only `done` artifacts can be attached",
            artifact.summary_status
        )));
    }
    crate::storage::artifacts::attach_artifact_to_meeting(
        &state.db,
        &meeting_id,
        &body.artifact_id,
    )
    .await
    .map_err(|e| ApiError::Db(downcast_db(e)))?;
    // Kick the agent so it sees the new artifact on its next fire
    // immediately, rather than waiting for the next transcript
    // trigger. Closed-channel send is silent (test routers without
    // a worker still serve attaches).
    let _ = state.agent_kick_tx.send(crate::agent::AgentKick {
        user_id: user_id.clone(),
        reason: crate::agent::AgentKickReason::ArtifactAttached {
            artifact_id: body.artifact_id.clone(),
        },
    });
    broadcast_artifacts_changed(&state, &meeting_id, &user_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Read the current attached-artifact set from DB and broadcast it
/// to all WS clients of `user_id` as `Event::ArtifactsChanged`.
/// Called from attach + detach handlers so Mac and PWA stay in
/// sync without either polling or trusting their own local mirror.
/// DB miss is logged and dropped — the broadcast was best-effort
/// anyway, and the next snapshot path will eventually carry the
/// truth.
async fn broadcast_artifacts_changed(state: &ApiState, meeting_id: &str, user_id: &str) {
    let attached =
        match crate::storage::artifacts::list_artifacts_for_meeting(&state.db, meeting_id).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = ?e, meeting_id, "broadcast_artifacts_changed: list failed");
                return;
            }
        };
    let artifact_ids: Vec<String> = attached.into_iter().map(|a| a.id).collect();
    state
        .bus
        .emit(
            user_id.to_string(),
            crate::protocol::Event::ArtifactsChanged { artifact_ids },
        )
        .await;
}

pub(crate) async fn detach_artifact(
    State(state): State<ApiState>,
    Path((meeting_id, artifact_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let user_id = require_user(&headers, &state).await?;
    // Ownership check on the meeting side is sufficient — the join
    // row only exists if both pieces were owned at attach time, and
    // the artifact_id path param can't bypass that.
    let meeting_owned: Option<(String,)> =
        sqlx::query_as(r#"SELECT id FROM meetings WHERE id = $1 AND user_id = $2"#)
            .bind(&meeting_id)
            .bind(&user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::Db)?;
    if meeting_owned.is_none() {
        return Err(ApiError::NotFound);
    }
    crate::storage::artifacts::detach_artifact_from_meeting(&state.db, &meeting_id, &artifact_id)
        .await
        .map_err(|e| ApiError::Db(downcast_db(e)))?;
    broadcast_artifacts_changed(&state, &meeting_id, &user_id).await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sqlx::PgPool;
    use std::sync::Arc;
    use tokio::sync::broadcast;
    use tower::ServiceExt;

    async fn router_with_dev_user(pool: PgPool) -> (axum::Router, String) {
        let (moment_created_tx, _) = broadcast::channel::<crate::api::MomentCreated>(8);
        let (artifact_created_tx, _) = broadcast::channel::<crate::api::ArtifactCreated>(8);
        let (wrap_up_retry_tx, _) = broadcast::channel::<crate::api::WrapUpRetry>(8);
        let (agent_kick_tx, _) = broadcast::channel::<crate::agent::AgentKick>(8);
        let (fanout, _) = broadcast::channel::<crate::protocol::UserEvent>(8);
        let (durable_tx, _durable_rx) = tokio::sync::mpsc::channel::<crate::protocol::UserEvent>(8);
        let bus = crate::context::EventBus::new(fanout, durable_tx);
        let dev_user = crate::storage::users::upsert_user_by_auth0_sub(
            &pool,
            crate::auth::DEV_AUTH0_SUB,
            Some("dev@local"),
            Some("Local Dev"),
        )
        .await
        .unwrap();
        let router = crate::api::make_router(ApiState {
            db: pool,
            auth: Arc::new(crate::auth::AuthMode::Disabled),
            moment_created_tx,
            artifact_created_tx,
            wrap_up_retry_tx,
            agent_kick_tx,
            bus,
        });
        (router, dev_user.id)
    }

    async fn body_string(resp: axum::response::Response) -> String {
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    /// Build a minimal `multipart/form-data` body with a single `file`
    /// field. Boundary chosen to be unambiguous against the payload.
    fn multipart_body(filename: &str, mime: &str, content: &[u8]) -> (String, Vec<u8>) {
        let boundary = "----auristest";
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(format!("Content-Type: {mime}\r\n\r\n").as_bytes());
        body.extend_from_slice(content);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        (format!("multipart/form-data; boundary={boundary}"), body)
    }

    /// Point `AURIS_DATA_DIR` at a unique per-test path so
    /// blob writes don't leak between concurrent runs. Avoids a
    /// `tempfile` dep — the dir lives under `/tmp` and gets recycled
    /// by the OS. Tests don't bother cleaning it up; the volume is
    /// small (a handful of bytes per artifact).
    fn scoped_data_dir() {
        let path = std::env::temp_dir().join(format!("auris-test-{}", uuid::Uuid::new_v4()));
        std::env::set_var("AURIS_DATA_DIR", &path);
    }

    #[sqlx::test]
    async fn upload_artifact_creates_pending_row(pool: PgPool) {
        scoped_data_dir();
        let (app, _uid) = router_with_dev_user(pool).await;
        let (ct, body) = multipart_body("agenda.md", "text/markdown", b"# Agenda\n- A\n- B\n");
        let resp = app
            .oneshot(
                Request::post("/artifacts")
                    .header("authorization", "Bearer dev")
                    .header("content-type", ct)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(v["name"], "agenda.md");
        assert_eq!(v["mime_type"], "text/markdown");
        assert_eq!(v["summary_status"], "pending");
        assert!(v["short_summary"].is_null());
    }

    #[sqlx::test]
    async fn upload_artifact_rejects_unsupported_mime(pool: PgPool) {
        scoped_data_dir();
        let (app, _uid) = router_with_dev_user(pool).await;
        let (ct, body) = multipart_body("evil.exe", "application/x-msdownload", b"MZ");
        let resp = app
            .oneshot(
                Request::post("/artifacts")
                    .header("authorization", "Bearer dev")
                    .header("content-type", ct)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[sqlx::test]
    async fn list_artifacts_returns_users_library(pool: PgPool) {
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::storage::artifacts::insert_artifact(
            &pool,
            "a1",
            &uid,
            "x.md",
            "text/markdown",
            "p1",
            10,
        )
        .await
        .unwrap();
        crate::storage::artifacts::insert_artifact(
            &pool,
            "a2",
            &uid,
            "y.md",
            "text/markdown",
            "p2",
            20,
        )
        .await
        .unwrap();
        let resp = app
            .oneshot(
                Request::get("/artifacts")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: Vec<serde_json::Value> = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(v.len(), 2);
    }

    #[sqlx::test]
    async fn get_artifact_404_for_other_users_artifact(pool: PgPool) {
        let other =
            crate::storage::users::upsert_user_by_auth0_sub(&pool, "other|user", None, None)
                .await
                .unwrap();
        crate::storage::artifacts::insert_artifact(
            &pool,
            "a1",
            &other.id,
            "x.md",
            "text/markdown",
            "p",
            1,
        )
        .await
        .unwrap();
        let (app, _uid) = router_with_dev_user(pool).await;
        let resp = app
            .oneshot(
                Request::get("/artifacts/a1")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn delete_artifact_removes_row(pool: PgPool) {
        scoped_data_dir();
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::storage::artifacts::insert_artifact(
            &pool,
            "a1",
            &uid,
            "x.md",
            "text/markdown",
            "p",
            1,
        )
        .await
        .unwrap();
        let resp = app
            .clone()
            .oneshot(
                Request::delete("/artifacts/a1")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        // Row gone.
        let resp2 = app
            .oneshot(
                Request::get("/artifacts/a1")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn attach_artifact_rejects_pending_status(pool: PgPool) {
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::storage::meetings::insert_meeting(
            &pool,
            "m1",
            &uid,
            chrono::Utc::now(),
            None,
            "{}",
            None,
        )
        .await
        .unwrap();
        crate::storage::artifacts::insert_artifact(
            &pool,
            "a1",
            &uid,
            "x.md",
            "text/markdown",
            "p",
            1,
        )
        .await
        .unwrap();
        // Status is `pending` by default — attach should reject.
        let resp = app
            .oneshot(
                Request::post("/meetings/m1/artifacts")
                    .header("authorization", "Bearer dev")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"artifact_id":"a1"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[sqlx::test]
    async fn attach_then_detach_round_trips(pool: PgPool) {
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::storage::meetings::insert_meeting(
            &pool,
            "m1",
            &uid,
            chrono::Utc::now(),
            None,
            "{}",
            None,
        )
        .await
        .unwrap();
        crate::storage::artifacts::insert_artifact(
            &pool,
            "a1",
            &uid,
            "x.md",
            "text/markdown",
            "p",
            1,
        )
        .await
        .unwrap();
        // Mark as done so attach is allowed.
        crate::storage::artifacts::update_artifact_summaries(&pool, "a1", "short", "long", "done")
            .await
            .unwrap();
        // Attach.
        let resp = app
            .clone()
            .oneshot(
                Request::post("/meetings/m1/artifacts")
                    .header("authorization", "Bearer dev")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"artifact_id":"a1"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let attached = crate::storage::artifacts::list_artifacts_for_meeting(&pool, "m1")
            .await
            .unwrap();
        assert_eq!(attached.len(), 1);
        // Detach.
        let resp2 = app
            .oneshot(
                Request::delete("/meetings/m1/artifacts/a1")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::NO_CONTENT);
        let attached2 = crate::storage::artifacts::list_artifacts_for_meeting(&pool, "m1")
            .await
            .unwrap();
        assert!(attached2.is_empty());
    }

    #[sqlx::test]
    async fn list_meeting_artifacts_returns_attached_set(pool: PgPool) {
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::storage::meetings::insert_meeting(
            &pool,
            "m1",
            &uid,
            chrono::Utc::now(),
            None,
            "{}",
            None,
        )
        .await
        .unwrap();
        crate::storage::artifacts::insert_artifact(
            &pool,
            "a1",
            &uid,
            "x.md",
            "text/markdown",
            "p",
            1,
        )
        .await
        .unwrap();
        crate::storage::artifacts::update_artifact_summaries(&pool, "a1", "short", "long", "done")
            .await
            .unwrap();
        crate::storage::artifacts::attach_artifact_to_meeting(&pool, "m1", "a1")
            .await
            .unwrap();
        let resp = app
            .oneshot(
                Request::get("/meetings/m1/artifacts")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        let artifacts = v
            .get("artifacts")
            .and_then(|x| x.as_array())
            .expect("artifacts array");
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0]["id"], "a1");
    }

    #[sqlx::test]
    async fn list_meeting_artifacts_empty_when_none_attached(pool: PgPool) {
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::storage::meetings::insert_meeting(
            &pool,
            "m1",
            &uid,
            chrono::Utc::now(),
            None,
            "{}",
            None,
        )
        .await
        .unwrap();
        let resp = app
            .oneshot(
                Request::get("/meetings/m1/artifacts")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
        assert_eq!(v["artifacts"].as_array().unwrap().len(), 0);
    }

    #[sqlx::test]
    async fn list_meeting_artifacts_404_for_other_users_meeting(pool: PgPool) {
        let other =
            crate::storage::users::upsert_user_by_auth0_sub(&pool, "other|user", None, None)
                .await
                .unwrap();
        crate::storage::meetings::insert_meeting(
            &pool,
            "other-m",
            &other.id,
            chrono::Utc::now(),
            None,
            "{}",
            None,
        )
        .await
        .unwrap();
        let (app, _uid) = router_with_dev_user(pool).await;
        let resp = app
            .oneshot(
                Request::get("/meetings/other-m/artifacts")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn list_meeting_artifacts_404_for_unknown_meeting(pool: PgPool) {
        let (app, _uid) = router_with_dev_user(pool).await;
        let resp = app
            .oneshot(
                Request::get("/meetings/nonesuch/artifacts")
                    .header("authorization", "Bearer dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test]
    async fn attach_404s_when_meeting_belongs_to_other_user(pool: PgPool) {
        let other =
            crate::storage::users::upsert_user_by_auth0_sub(&pool, "other|user", None, None)
                .await
                .unwrap();
        crate::storage::meetings::insert_meeting(
            &pool,
            "other-m",
            &other.id,
            chrono::Utc::now(),
            None,
            "{}",
            None,
        )
        .await
        .unwrap();
        let (app, uid) = router_with_dev_user(pool.clone()).await;
        crate::storage::artifacts::insert_artifact(
            &pool,
            "a1",
            &uid,
            "x.md",
            "text/markdown",
            "p",
            1,
        )
        .await
        .unwrap();
        crate::storage::artifacts::update_artifact_summaries(&pool, "a1", "s", "l", "done")
            .await
            .unwrap();
        let resp = app
            .oneshot(
                Request::post("/meetings/other-m/artifacts")
                    .header("authorization", "Bearer dev")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"artifact_id":"a1"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
