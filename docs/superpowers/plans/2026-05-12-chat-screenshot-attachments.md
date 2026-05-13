# Chat screenshot attachments (Mac) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a Mac user capture multiple screenshots during an active meeting and attach them to a single chat message that the agent answers using vision.

**Architecture:** Mac client captures via the existing `ScreenshotCapture.capturePrimaryDisplay()` path, uploads each PNG to a new HTTP route `POST /meetings/:id/chat_attachments` (returns an id), queues thumbnail chips in the chat compose region (cap 4), then ships `Intent::Chat { text, attachment_ids: [...] }` over the existing WS. The server loads bytes from disk and threads them as `UserContent::Image` blocks into the agent's `rig::completion::Message`, which `agent.prompt()` already accepts via `Into<Message>`. Lifetime is meeting-scoped: cascade-deleted with the meeting both at the DB and filesystem layers.

**Tech Stack:** Rust (axum, sqlx/Postgres, rig 0.36 with three provider backends), Swift/SwiftUI (Mac client), existing `Intent::Chat` WebSocket flow.

**Spec:** `docs/superpowers/specs/2026-05-12-chat-screenshot-attachments-design.md`

---

## File Structure

### Server (Rust) — `packages/server/`

| Action | Path | Responsibility |
|---|---|---|
| Create | `migrations/0006_chat_attachments.sql` | DDL for `chat_attachments` table with cascade FK |
| Modify | `src/db.rs` | Add `ChatAttachmentRow` struct + `insert_chat_attachment` + `get_chat_attachment` helpers |
| Modify | `src/contract.rs` | Extend `Intent::Chat` with `#[serde(default)] attachment_ids: Vec<String>` |
| Modify | `src/api.rs` | Add `upload_chat_attachment` handler + route registration |
| Modify | `src/summarizer/agent.rs` | Add `AttachmentPayload`; extend `AgentKickReason::ChatMessage` + `KickBlock::Chat`; build typed `Message::User` with image content blocks |
| Modify | `src/ws.rs` | Extend `Intent::Chat` handler to resolve `attachment_ids` → bytes; emit new error events on failure |
| Create | `tests/chat_attachments.rs` | HTTP + WS integration tests |

### Mac client (Swift) — `packages/mac/`

| Action | Path | Responsibility |
|---|---|---|
| Modify | `Sources/Auris/Net/Protocol.swift` | Extend `ChatIntent` with `attachmentIds` |
| Modify | `Sources/Auris/Net/MeetingsAPI.swift` | Add `uploadChatAttachment(meetingId:png:)` |
| Modify | `Sources/Auris/AppModel.swift` | Add `ChatAttachmentDraft`, `pendingChatAttachments`, capture/upload/remove/send methods, idle-state cleanup |
| Modify | `Sources/Auris/MeetingOverlayView.swift` | Add `ChatAttachmentStrip` + `ChatAttachmentChip` views; slot camera button into chat compose row |
| Create | `Tests/AurisTests/AppModelChatAttachmentTests.swift` | Unit tests for AppModel state machine |

---

## Task 1: DB migration + db.rs helpers

**Files:**
- Create: `packages/server/migrations/0006_chat_attachments.sql`
- Modify: `packages/server/src/db.rs` (append new helpers near the moments helpers around line 390-422)

- [ ] **Step 1: Write the migration**

Create `packages/server/migrations/0006_chat_attachments.sql`:

```sql
-- 0006_chat_attachments.sql
--
-- Adds the per-meeting chat-attachment table backing the Mac "attach
-- screenshots to a chat message" feature. Cascade-deletes with the
-- meeting; the on-disk PNGs live under
--   <data_dir>/blobs/meetings/<meeting_id>/chat/<attachment_id>.png
-- and are wiped by the existing meetings-delete handler's recursive
-- remove_dir_all on <data_dir>/blobs/meetings/<meeting_id>.

CREATE TABLE chat_attachments (
    id          TEXT PRIMARY KEY,
    meeting_id  TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    user_id     TEXT NOT NULL,
    mime        TEXT NOT NULL,
    bytes_path  TEXT NOT NULL,
    bytes_size  BIGINT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX chat_attachments_meeting_id_idx ON chat_attachments (meeting_id);
```

- [ ] **Step 2: Add the row struct + helpers to `db.rs`**

Append after `update_moment_asset_path` / `MomentRow` in `packages/server/src/db.rs` (around line 422):

```rust
// ─── Chat attachment subsystem ──────────────────────────────────────────

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
pub async fn get_chat_attachment(
    pool: &PgPool,
    id: &str,
) -> Result<Option<ChatAttachmentRow>> {
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
```

- [ ] **Step 3: Write failing db tests**

Create `packages/server/tests/chat_attachments_db.rs`. Pattern matches the existing test harness: `#[tokio::test]` + `dotenvy::dotenv()` + `db::open_pool()` (the existing tests do NOT use `sqlx::test`).

```rust
//! DB-layer tests for chat_attachments table.

use auris_server::db;

async fn seed_meeting(user_id: &str) -> String {
    let pool = db::open_pool().await.expect("open pool");
    let id = ulid::Ulid::new().to_string();
    sqlx::query(r#"
        INSERT INTO meetings (id, user_id, started_at, metadata)
        VALUES ($1, $2, NOW(), '{}'::jsonb)
    "#)
    .bind(&id)
    .bind(user_id)
    .execute(&pool)
    .await
    .expect("insert meeting");
    id
}

#[tokio::test]
async fn insert_and_get_round_trip() {
    let _ = dotenvy::dotenv();
    let pool = db::open_pool().await.expect("open pool");
    let user_id = format!("test-user-{}", ulid::Ulid::new());
    let meeting_id = seed_meeting(&user_id).await;
    let att_id = ulid::Ulid::new().to_string();

    db::insert_chat_attachment(
        &pool, &att_id, &meeting_id, &user_id, "image/png",
        "blobs/meetings/m/chat/att.png", 12345,
    ).await.expect("insert ok");

    let got = db::get_chat_attachment(&pool, &att_id)
        .await
        .expect("get ok")
        .expect("row exists");

    assert_eq!(got.id, att_id);
    assert_eq!(got.meeting_id, meeting_id);
    assert_eq!(got.user_id, user_id);
    assert_eq!(got.mime, "image/png");
    assert_eq!(got.bytes_path, "blobs/meetings/m/chat/att.png");
    assert_eq!(got.bytes_size, 12345);
}

#[tokio::test]
async fn get_missing_returns_none() {
    let _ = dotenvy::dotenv();
    let pool = db::open_pool().await.expect("open pool");
    let got = db::get_chat_attachment(&pool, "nope-does-not-exist")
        .await
        .expect("get ok");
    assert!(got.is_none());
}

#[tokio::test]
async fn cascade_delete_with_meeting() {
    let _ = dotenvy::dotenv();
    let pool = db::open_pool().await.expect("open pool");
    let user_id = format!("test-user-{}", ulid::Ulid::new());
    let meeting_id = seed_meeting(&user_id).await;
    let att_id = ulid::Ulid::new().to_string();

    db::insert_chat_attachment(
        &pool, &att_id, &meeting_id, &user_id, "image/png",
        "blobs/meetings/m/chat/att.png", 1,
    ).await.unwrap();

    let deleted = db::delete_meeting_for_user(&pool, &meeting_id, &user_id)
        .await
        .expect("delete ok");
    assert!(deleted);

    let got = db::get_chat_attachment(&pool, &att_id).await.expect("get ok");
    assert!(got.is_none(), "cascade should have removed the attachment row");
}
```

Note: these tests share one Postgres DB, so each test generates fresh ulid-based ids to avoid collisions. The `meetings` INSERT shape may need additional columns — check `packages/server/migrations/0001_initial_schema.sql` for the canonical `meetings` columns and adapt the SQL above if any are NOT NULL without a default.

- [ ] **Step 4: Run tests; expect compile-then-fail because the helpers don't exist yet**

From repo root:

```bash
cd packages/server && cargo test --test chat_attachments_db -- --nocapture
```

Expected (because Step 2 hasn't been applied yet if the implementer is doing strict TDD): compile errors referencing `insert_chat_attachment` / `get_chat_attachment` / `ChatAttachmentRow`.

If Step 2 was already applied (the cleaner ordering for this codebase where the DB helpers and the migration are interdependent and easier to write together), Step 4 should show the tests *passing*. Either ordering is fine; the key is that you ran the tests before and after the implementation.

- [ ] **Step 5: Run migrations and tests, expect pass**

```bash
cd packages/server && sqlx migrate run --source migrations
cargo test --test chat_attachments_db -- --nocapture
```

Expected:
```
test insert_and_get_round_trip ... ok
test get_missing_returns_none ... ok
test cascade_delete_with_meeting ... ok
```

- [ ] **Step 6: Commit**

```bash
git add packages/server/migrations/0006_chat_attachments.sql \
        packages/server/src/db.rs \
        packages/server/tests/chat_attachments_db.rs
git commit -m "$(cat <<'EOF'
feat(server): chat_attachments table + db helpers

Adds the per-meeting chat-attachment store backing the Mac
"attach screenshots to a chat message" feature. FK to meetings
with ON DELETE CASCADE means attachments are wiped with the
meeting (the existing remove_dir_all on
<data_dir>/blobs/meetings/<id> covers the on-disk PNGs).
EOF
)"
```

---

## Task 2: HTTP upload route

**Files:**
- Modify: `packages/server/src/api.rs` (handler + route registration)

- [ ] **Step 1: Add the response struct + handler to `api.rs`**

Add near `upload_moment_screenshot` (around line 619) in `packages/server/src/api.rs`:

```rust
#[derive(Debug, Serialize)]
struct UploadChatAttachmentResponse {
    id: String,
}

/// `POST /meetings/:id/chat_attachments` — raw PNG upload that stages
/// an image for inclusion in the next `Intent::Chat`. Body is raw
/// `image/png`; the response carries the assigned attachment id.
/// Bytes land at `<data_dir>/blobs/meetings/<id>/chat/<aid>.png`,
/// parallel to moments' `screenshots/` subdir.
async fn upload_chat_attachment(
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
    // Strip optional `; charset=…` parameter if a client adds one.
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
    let row: Option<(String,)> = sqlx::query_as(
        r#"SELECT id FROM meetings WHERE id = $1 AND user_id = $2"#,
    )
    .bind(&meeting_id)
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::Db)?;
    if row.is_none() {
        // 404 covers both "no such meeting" and "owned by someone else" —
        // mirrors the moment-screenshot path's "don't leak existence."
        return Err(ApiError::NotFound);
    }

    let attachment_id = ulid::Ulid::new().to_string();
    let rel = format!("blobs/meetings/{meeting_id}/chat/{attachment_id}.png");
    let dir = crate::db::data_dir().map_err(|e| ApiError::Internal(e.to_string()))?;
    let abs = dir.join(&rel);
    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::Internal(format!("mkdir: {e}")))?;
    }
    tokio::fs::write(&abs, &bytes)
        .await
        .map_err(|e| ApiError::Internal(format!("write attachment: {e}")))?;

    crate::db::insert_chat_attachment(
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
```

- [ ] **Step 2: Register the route in `make_router`**

In `packages/server/src/api.rs`, inside `make_router` (around line 132), add the route between the moments route and the artifacts routes:

```rust
.route(
    "/meetings/:id/chat_attachments",
    post(upload_chat_attachment),
)
```

The existing `.layer(DefaultBodyLimit::max(SCREENSHOT_BODY_LIMIT))` already covers all routes registered before it — no per-route body-limit change needed.

- [ ] **Step 3: Run the build to catch compile errors**

```bash
cd packages/server && cargo build --tests
```

Expected: clean build. Fix any import gaps (`ulid::Ulid` needs `ulid` already in `Cargo.toml` — it's used by moments, so no new dep).

- [ ] **Step 4: Write integration tests**

Create `packages/server/tests/chat_attachments.rs`. **Test harness notes** (verify by reading `packages/server/tests/common/mod.rs` and `packages/server/tests/handshake.rs` before writing):

- `spawn_test_server()` takes no args, runs auth-disabled (every request → synthetic user `dev|local`), and serves both WS and HTTP routes on `server.addr`.
- There is no fluent test-client wrapper. HTTP tests use `reqwest::Client::new().post(format!("http://{}/path", server.addr))`. WS tests use `common::connect()`, `common::send_intent()`, `common::next_event()`.
- DB state must be set up via `db::*` helpers directly because the harness shares one Postgres pool across tests. Tests must clean up after themselves (use distinct ulid-based ids per test to avoid collisions) — there is **no `sqlx::test` migration isolation** in this harness's WS-path tests.
- Cross-tenant HTTP testing requires inserting a meeting owned by a different user via `db::insert_user` + `db::insert_meeting` (or whichever helpers exist — search `packages/server/src/db.rs`), then hitting the HTTP route normally (which maps to `dev|local` and should 404).

```rust
//! HTTP integration tests for the chat-attachments upload route.

mod common;
use common::spawn_test_server;

use auris_server::db;

// Each test uses ulid-based ids to avoid colliding with other tests
// that share the same Postgres pool. The auth-disabled harness pins
// every request to the synthetic user "dev|local".
const TEST_USER: &str = "dev|local";

async fn seed_meeting_for(user_id: &str) -> String {
    // Open a pool to seed the meeting row directly. The harness
    // doesn't expose its pool, so we open a parallel one against
    // the same DATABASE_URL — sqlx pools are independent.
    let pool = db::open_pool().await.expect("open pool");
    let id = ulid::Ulid::new().to_string();
    // Use whichever helper inserts a meeting in the codebase
    // (search packages/server/src/db.rs for `pub async fn insert_meeting`
    // or similar). If only a higher-level `start_meeting` exists,
    // use raw SQL here — the goal is just to make the FK in
    // chat_attachments satisfiable.
    sqlx::query(r#"
        INSERT INTO meetings (id, user_id, started_at, metadata)
        VALUES ($1, $2, NOW(), '{}'::jsonb)
    "#)
    .bind(&id)
    .bind(user_id)
    .execute(&pool)
    .await
    .expect("insert meeting");
    id
}

#[tokio::test]
async fn upload_happy_path() {
    let _ = dotenvy::dotenv();
    let server = spawn_test_server().await;
    let meeting_id = seed_meeting_for(TEST_USER).await;

    let client = reqwest::Client::new();
    let png_bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    let resp = client
        .post(format!(
            "http://{}/meetings/{}/chat_attachments",
            server.addr, meeting_id
        ))
        .header("content-type", "image/png")
        .body(png_bytes.clone())
        .send()
        .await
        .expect("request ok");

    assert_eq!(resp.status().as_u16(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    let id = body["id"].as_str().expect("id present").to_string();

    let pool = db::open_pool().await.unwrap();
    let row = db::get_chat_attachment(&pool, &id)
        .await
        .expect("get ok")
        .expect("row exists");
    assert_eq!(row.meeting_id, meeting_id);
    assert_eq!(row.user_id, TEST_USER);
    assert_eq!(row.mime, "image/png");
    assert_eq!(row.bytes_size as usize, png_bytes.len());
    assert!(row
        .bytes_path
        .starts_with(&format!("blobs/meetings/{meeting_id}/chat/")));
}

#[tokio::test]
async fn rejects_wrong_mime() {
    let _ = dotenvy::dotenv();
    let server = spawn_test_server().await;
    let meeting_id = seed_meeting_for(TEST_USER).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{}/meetings/{}/chat_attachments",
            server.addr, meeting_id
        ))
        .header("content-type", "image/jpeg")
        .body(b"\xff\xd8\xff\xe0".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
}

#[tokio::test]
async fn rejects_empty_body() {
    let _ = dotenvy::dotenv();
    let server = spawn_test_server().await;
    let meeting_id = seed_meeting_for(TEST_USER).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{}/meetings/{}/chat_attachments",
            server.addr, meeting_id
        ))
        .header("content-type", "image/png")
        .body(Vec::<u8>::new())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
}

#[tokio::test]
async fn rejects_unknown_meeting() {
    let _ = dotenvy::dotenv();
    let server = spawn_test_server().await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{}/meetings/{}/chat_attachments",
            server.addr,
            ulid::Ulid::new()
        ))
        .header("content-type", "image/png")
        .body(b"\x89PNG\r\n\x1a\n".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn rejects_cross_tenant_meeting() {
    let _ = dotenvy::dotenv();
    let server = spawn_test_server().await;

    // Meeting owned by SOMEONE ELSE — not `dev|local`.
    let foreign_meeting_id = seed_meeting_for("foreign-user").await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{}/meetings/{}/chat_attachments",
            server.addr, foreign_meeting_id
        ))
        .header("content-type", "image/png")
        .body(b"\x89PNG\r\n\x1a\n".to_vec())
        .send()
        .await
        .unwrap();
    // Same as moments: 404 (not 403) to avoid leaking existence.
    assert_eq!(resp.status().as_u16(), 404);
}
```

> The `rejects_unauthenticated` HTTP case from the spec is **not** representable in this test harness — `AURIS_AUTH_DISABLED=1` is set unconditionally in `spawn_test_server_with_token`, so every request maps to `dev|local`. Coverage for the unauthenticated path stays as a manual verification step (or a new auth-on harness in a follow-up). Document this gap in the test file's module docstring.

- [ ] **Step 5: Run integration tests; expect pass**

```bash
cd packages/server && cargo test --test chat_attachments -- --nocapture
```

Expected: all five tests pass (the unauthenticated-path test from the spec is intentionally absent — see harness limitation note above).

- [ ] **Step 6: Commit**

```bash
git add packages/server/src/api.rs packages/server/tests/chat_attachments.rs
git commit -m "$(cat <<'EOF'
feat(server): POST /meetings/:id/chat_attachments upload route

Raw image/png upload that stages a screenshot for inclusion in the
next Intent::Chat. Mirrors the moment-screenshot upload pattern:
PNG-only, 64 MiB cap inherited from SCREENSHOT_BODY_LIMIT, 404 on
cross-tenant access (no existence leak). Bytes land under
blobs/meetings/<mid>/chat/<aid>.png; row tracks via FK with cascade.
EOF
)"
```

---

## Task 3: Contract change + AttachmentPayload + AgentKick extension

**Files:**
- Modify: `packages/server/src/contract.rs` (`Intent::Chat`)
- Modify: `packages/server/src/summarizer/agent.rs` (`AttachmentPayload`, `AgentKickReason::ChatMessage`, `KickBlock::Chat`)

- [ ] **Step 1: Extend `Intent::Chat` with `attachment_ids`**

In `packages/server/src/contract.rs`, modify the `Chat` variant (currently at line 148-156):

```rust
/// Chat with the agent during an active meeting. The user's
/// question is rendered as the user-side bubble in chat mode
/// (Replace strategy, single Q+A pair); the agent's reply
/// becomes the assistant-side bubble. Allowed only when a
/// meeting is active or paused — chat is per-meeting only,
/// no persistence across meetings in v1.
///
/// `attachment_ids` (added 2026-05-12) reference rows in
/// `chat_attachments`, uploaded via
/// `POST /meetings/:id/chat_attachments`. The server reads
/// the bytes from disk and threads them as vision content
/// blocks into the agent's LLM call. Empty list = today's
/// text-only behavior. Mac is the only producer in v1.
Chat {
    text: String,
    #[serde(default)]
    attachment_ids: Vec<String>,
},
```

- [ ] **Step 2: Add `AttachmentPayload` to `agent.rs`**

Near the top of `packages/server/src/summarizer/agent.rs` (above the `AgentKick`/`AgentKickReason` definitions around line 1565):

```rust
/// Bytes + mime for a chat attachment. Owns the bytes so it can
/// travel through the `AgentKick` broadcast channel without
/// dipping back into the DB or filesystem.
///
/// Custom `Debug` redacts the byte vector — `tracing::info!(?kick)`
/// elsewhere in the agent loop must not spill base64 into logs.
#[derive(Clone)]
pub struct AttachmentPayload {
    pub mime: String,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for AttachmentPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachmentPayload")
            .field("mime", &self.mime)
            .field("bytes_len", &self.bytes.len())
            .finish()
    }
}
```

- [ ] **Step 3: Extend `AgentKickReason::ChatMessage` with attachments**

In `packages/server/src/summarizer/agent.rs`, modify the variant (currently at line 1584):

```rust
/// User sent a chat message. The agent's text response becomes
/// the assistant-side reply, rendered alongside the user's
/// question in chat mode (Replace strategy, single Q+A pair).
/// Tool calls are still allowed during a chat fire — if the
/// user asks "record this as an action," the agent emits the
/// tool call AND the text reply.
///
/// `attachments` (added 2026-05-12) carries any screenshots the
/// user attached via the Mac compose strip. Empty for text-only
/// chats. Bytes are loaded by the WS handler before kicking; the
/// agent task threads them as `UserContent::Image` blocks.
ChatMessage {
    text: String,
    attachments: Vec<AttachmentPayload>,
},
```

- [ ] **Step 4: Extend `KickBlock::Chat` with attachments**

In `packages/server/src/summarizer/agent.rs`, modify `KickBlock::Chat` (currently at line 1437-1439):

```rust
Chat {
    user_text: String,
    attachments: Vec<AttachmentPayload>,
},
```

And update the matching arm in `reason_to_kick_block` (the function that builds `KickBlock` from `AgentKickReason`; around line 1493) to forward the new field:

```rust
AgentKickReason::ChatMessage { text, attachments } => Some(KickBlock::Chat {
    user_text: text.clone(),
    attachments: attachments.clone(),
}),
```

- [ ] **Step 5: Update `KickBlock::Chat` consumers**

Two spots inside `compose_and_fire` (the function that handles `kick_block` around line 974, also around line 983) reference `KickBlock::Chat { user_text }` via pattern match. Update both to ignore the new field:

```rust
let chat_user_text: Option<String> = match &kick_block {
    Some(KickBlock::Chat { user_text, .. }) => Some(user_text.clone()),
    _ => None,
};
```

And the `body()` impl (around line 1462):

```rust
KickBlock::Chat { user_text, .. } => format!("User: {user_text:?}"),
```

(The label() impl already ignores the payload via `Chat { .. }`, no change needed there.)

- [ ] **Step 6: Compile-check**

```bash
cd packages/server && cargo build --tests
```

Expected: clean build. If there are additional `KickBlock::Chat { user_text }` or `AgentKickReason::ChatMessage { text }` matches elsewhere in the codebase (search with `grep -rn "ChatMessage" packages/server/src/`), update each to use `text, attachments` or `text, ..`.

- [ ] **Step 7: Commit**

```bash
git add packages/server/src/contract.rs packages/server/src/summarizer/agent.rs
git commit -m "$(cat <<'EOF'
feat(server): thread chat attachments through Intent + AgentKick

Adds attachment_ids to Intent::Chat (additive, serde-default empty)
and AttachmentPayload bytes to AgentKickReason::ChatMessage and
KickBlock::Chat. AttachmentPayload's Debug impl redacts bytes so
tracing log lines for AgentKick never spill base64 image data.

No behavior change yet — wiring only.
EOF
)"
```

---

## Task 4: Agent vision message construction

**Files:**
- Modify: `packages/server/src/summarizer/agent.rs` (build typed `Message::User` when attachments present)

- [ ] **Step 1: Add a unit test for `build_user_message`**

Append to the `#[cfg(test)] mod tests { … }` block at the bottom of `packages/server/src/summarizer/agent.rs` (create one if absent):

```rust
#[cfg(test)]
mod build_user_message_tests {
    use super::*;
    use rig::completion::Message;
    use rig::message::{DocumentSourceKind, ImageMediaType, UserContent};

    #[test]
    fn text_only_produces_single_text_part() {
        let msg = build_user_message("hello".to_string(), vec![]);
        match msg {
            Message::User { content } => {
                let parts: Vec<UserContent> = content.into_iter().collect();
                assert_eq!(parts.len(), 1);
                assert!(matches!(parts[0], UserContent::Text(_)));
            }
            _ => panic!("expected Message::User"),
        }
    }

    #[test]
    fn attachments_only_produces_image_parts() {
        let attachments = vec![
            AttachmentPayload { mime: "image/png".into(), bytes: vec![0, 1, 2] },
            AttachmentPayload { mime: "image/png".into(), bytes: vec![3, 4, 5] },
        ];
        let msg = build_user_message("".to_string(), attachments);
        match msg {
            Message::User { content } => {
                let parts: Vec<UserContent> = content.into_iter().collect();
                assert_eq!(parts.len(), 2);
                for p in &parts {
                    assert!(matches!(p, UserContent::Image(_)));
                }
            }
            _ => panic!("expected Message::User"),
        }
    }

    #[test]
    fn mixed_produces_text_then_images_in_order() {
        let attachments = vec![
            AttachmentPayload { mime: "image/png".into(), bytes: vec![1] },
            AttachmentPayload { mime: "image/png".into(), bytes: vec![2] },
        ];
        let msg = build_user_message("compare these".to_string(), attachments);
        match msg {
            Message::User { content } => {
                let parts: Vec<UserContent> = content.into_iter().collect();
                assert_eq!(parts.len(), 3);
                assert!(matches!(parts[0], UserContent::Text(_)));
                assert!(matches!(parts[1], UserContent::Image(_)));
                assert!(matches!(parts[2], UserContent::Image(_)));
            }
            _ => panic!("expected Message::User"),
        }
    }

    #[test]
    fn whitespace_only_text_skipped() {
        let attachments = vec![AttachmentPayload {
            mime: "image/png".into(),
            bytes: vec![0],
        }];
        let msg = build_user_message("   \n  ".to_string(), attachments);
        match msg {
            Message::User { content } => {
                let parts: Vec<UserContent> = content.into_iter().collect();
                assert_eq!(parts.len(), 1, "whitespace text dropped, image kept");
                assert!(matches!(parts[0], UserContent::Image(_)));
            }
            _ => panic!("expected Message::User"),
        }
    }

    #[test]
    fn image_uses_base64_png_media_type() {
        let attachments = vec![AttachmentPayload {
            mime: "image/png".into(),
            bytes: vec![0xAA, 0xBB, 0xCC],
        }];
        let msg = build_user_message("".to_string(), attachments);
        if let Message::User { content } = msg {
            let parts: Vec<UserContent> = content.into_iter().collect();
            if let UserContent::Image(img) = &parts[0] {
                assert_eq!(img.media_type, Some(ImageMediaType::Png));
                assert!(matches!(img.data, DocumentSourceKind::Base64(_)));
            } else {
                panic!("expected Image part");
            }
        }
    }
}
```

- [ ] **Step 2: Run the test; expect compile failure (function not defined)**

```bash
cd packages/server && cargo test --lib build_user_message -- --nocapture
```

Expected: `error[E0425]: cannot find function ‘build_user_message’ in this scope`.

- [ ] **Step 3: Implement `build_user_message`**

Add to `packages/server/src/summarizer/agent.rs` near the other small helpers (e.g. above `compose_and_fire`):

```rust
use base64::Engine as _;
use rig::message::{DocumentSourceKind, Image as RigImage, ImageMediaType};

/// Build a `rig::completion::Message::User` from a text body plus a
/// list of attachments. Text comes first, followed by image content
/// blocks in caller order. Empty / whitespace-only text is skipped
/// (Anthropic rejects empty text blocks; OpenAI is unhappy about
/// them too). The caller MUST guarantee that at least one of (text,
/// attachments) is non-empty — `OneOrMany::many` panics otherwise.
fn build_user_message(text: String, attachments: Vec<AttachmentPayload>) -> RigMessage {
    let mut parts: Vec<rig::message::UserContent> = Vec::with_capacity(1 + attachments.len());
    if !text.trim().is_empty() {
        parts.push(rig::message::UserContent::Text(text.into()));
    }
    for a in attachments {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&a.bytes);
        parts.push(rig::message::UserContent::Image(RigImage {
            data: DocumentSourceKind::Base64(b64),
            media_type: Some(ImageMediaType::Png),
            detail: None,
            additional_params: None,
        }));
    }
    RigMessage::User {
        content: rig::OneOrMany::many(parts).expect("at least one content part"),
    }
}
```

- [ ] **Step 4: Run the unit tests; expect pass**

```bash
cd packages/server && cargo test --lib build_user_message -- --nocapture
```

Expected:
```
test summarizer::agent::build_user_message_tests::text_only_produces_single_text_part ... ok
test summarizer::agent::build_user_message_tests::attachments_only_produces_image_parts ... ok
test summarizer::agent::build_user_message_tests::mixed_produces_text_then_images_in_order ... ok
test summarizer::agent::build_user_message_tests::whitespace_only_text_skipped ... ok
test summarizer::agent::build_user_message_tests::image_uses_base64_png_media_type ... ok
```

- [ ] **Step 5: Switch `compose_and_fire` to typed `Message::User` when chat fire carries attachments**

In `packages/server/src/summarizer/agent.rs`, `compose_and_fire` currently builds `user_message: String` and calls `agent.prompt(user_message.clone())` for each provider arm (around lines 1010, 1054-1058, etc.).

Modify the composition path so attachments are pulled out of `kick_block` *before* the `sections.join("\n\n")`, then construct a typed `Message::User` that wraps the joined string + images:

Around line 974, where the chat-text capture happens, *also* capture attachments:

```rust
let (chat_user_text, chat_attachments): (Option<String>, Vec<AttachmentPayload>) =
    match &kick_block {
        Some(KickBlock::Chat { user_text, attachments }) => {
            (Some(user_text.clone()), attachments.clone())
        }
        _ => (None, Vec::new()),
    };
let is_chat_fire = chat_user_text.is_some();
```

Then after the existing `let user_message = sections.join("\n\n");` line (~1010), build the typed message:

```rust
// When the chat fire carries images, wrap the composed user text +
// images into a single typed Message::User. For text-only fires
// (the common case for non-chat kicks AND chat without attachments)
// we pass the String straight through — rig's
// `Into<Message>` impl wraps it for us, preserving current behavior.
let user_prompt: rig::completion::Message = if !chat_attachments.is_empty() {
    build_user_message(user_message.clone(), chat_attachments)
} else {
    user_message.clone().into()
};
```

Replace the three `agent.prompt(user_message.clone())` calls (one per provider arm, currently at ~1055, ~1077, ~1099 — verify line numbers with `grep -n "user_message.clone()" packages/server/src/summarizer/agent.rs`) with:

```rust
agent
    .prompt(user_prompt.clone())
    .with_history(history_input)
    .max_turns(MAX_TURNS_PER_FIRE)
    .extended_details()
    .await
```

Keep `user_message` (the String) available for any remaining `tracing::info!(... prompt = %user_message ...)` logging — the typed `Message` is harder to format and we don't want base64 in logs anyway.

- [ ] **Step 6: Run the full server build + tests, expect pass**

```bash
cd packages/server && cargo test --lib -- --nocapture
```

Expected: all existing tests still pass; the new `build_user_message_tests` group is green; nothing else regresses.

- [ ] **Step 7: Commit**

```bash
git add packages/server/src/summarizer/agent.rs
git commit -m "$(cat <<'EOF'
feat(server): agent threads chat attachments as vision content blocks

Adds build_user_message(text, attachments) -> Message::User and
routes the chat-with-attachments fire through it. Text-only fires
unchanged (still pass String → rig auto-wraps). Each image becomes
a base64 PNG UserContent::Image; rig translates per provider
(Anthropic content blocks / OpenAI image_url / Bedrock ImageBlock).
EOF
)"
```

---

## Task 5: WS chat handler — resolve attachments and dispatch

**Files:**
- Modify: `packages/server/src/ws.rs` (extend `Intent::Chat` arm at line 868)
- Modify: `packages/server/tests/chat_attachments.rs` (add WS-path integration tests)

- [ ] **Step 1: Extend the WS Chat handler**

In `packages/server/src/ws.rs`, replace the current `Intent::Chat { text }` arm (line 868-903) with:

```rust
if let Intent::Chat { text, attachment_ids } = intent {
    let trimmed = text.trim().to_string();
    // Allow empty text iff at least one attachment is present.
    if trimmed.is_empty() && attachment_ids.is_empty() {
        return Ok(());
    }

    // Active/paused gate (unchanged).
    let (active, current_meeting_id) = {
        let s = handle.state.lock().await;
        match s.user(user_id) {
            Some(u) if matches!(
                u.snapshot_meeting_state(),
                crate::contract::MeetingState::Active
                    | crate::contract::MeetingState::Paused
            ) => (true, u.active_meeting_id().map(|s| s.to_string())),
            _ => (false, None),
        }
    };
    if !active {
        let err = Event::Error {
            code: "no_active_meeting".into(),
            message: "Chat is only available during an active meeting".into(),
            intent_ref: None,
        };
        sink.send(Message::Text(serde_json::to_string(&err)?)).await.ok();
        return Ok(());
    }
    let Some(meeting_id) = current_meeting_id else {
        // Defensive: active state without a meeting_id should be impossible.
        return Ok(());
    };

    // Resolve attachments → bytes. Order is preserved.
    let data_dir = match crate::db::data_dir() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(error = %e, "data_dir() failed during chat attachment load");
            let err = Event::Error {
                code: "chat_attachment_unreadable".into(),
                message: "Server could not locate attachment storage".into(),
                intent_ref: None,
            };
            sink.send(Message::Text(serde_json::to_string(&err)?)).await.ok();
            return Ok(());
        }
    };
    let mut attachments: Vec<crate::summarizer::agent::AttachmentPayload> =
        Vec::with_capacity(attachment_ids.len());
    for id in &attachment_ids {
        let row = match crate::db::get_chat_attachment(&handle.db, id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                let err = Event::Error {
                    code: "chat_attachment_not_found".into(),
                    message: format!("attachment '{id}' not found"),
                    intent_ref: Some(id.clone()),
                };
                sink.send(Message::Text(serde_json::to_string(&err)?)).await.ok();
                return Ok(());
            }
            Err(e) => {
                tracing::error!(attachment_id = %id, error = ?e, "db lookup failed");
                let err = Event::Error {
                    code: "chat_attachment_unreadable".into(),
                    message: "Server failed to load attachment metadata".into(),
                    intent_ref: Some(id.clone()),
                };
                sink.send(Message::Text(serde_json::to_string(&err)?)).await.ok();
                return Ok(());
            }
        };
        if row.meeting_id != meeting_id || row.user_id != user_id {
            let err = Event::Error {
                code: "chat_attachment_forbidden".into(),
                message: format!("attachment '{id}' is not accessible in this meeting"),
                intent_ref: Some(id.clone()),
            };
            sink.send(Message::Text(serde_json::to_string(&err)?)).await.ok();
            return Ok(());
        }
        let abs = data_dir.join(&row.bytes_path);
        let bytes = match tokio::fs::read(&abs).await {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(
                    attachment_id = %id, path = %abs.display(), error = ?e,
                    "chat attachment bytes read failed"
                );
                let err = Event::Error {
                    code: "chat_attachment_unreadable".into(),
                    message: format!("Could not read attachment '{id}'"),
                    intent_ref: Some(id.clone()),
                };
                sink.send(Message::Text(serde_json::to_string(&err)?)).await.ok();
                return Ok(());
            }
        };
        attachments.push(crate::summarizer::agent::AttachmentPayload {
            mime: row.mime,
            bytes,
        });
    }

    let _ = handle
        .agent_kick_tx
        .send(crate::summarizer::agent::AgentKick {
            user_id: user_id.to_string(),
            reason: crate::summarizer::agent::AgentKickReason::ChatMessage {
                text: trimmed,
                attachments,
            },
        });
    return Ok(());
}
```

If `u.active_meeting_id()` doesn't exist on the `UserState` (or its equivalent), grep for whatever pattern the existing code uses to read the active meeting id (`grep -n "active_meeting_id\|snapshot_meeting_id\|current_meeting_id" packages/server/src/state.rs packages/server/src/ws.rs`). The exact accessor varies but the concept ("get the id of the currently-active meeting for this user") definitely exists — that's what the snapshot's `meeting_id` field is populated from in `Event::Snapshot`.

- [ ] **Step 2: Compile-check**

```bash
cd packages/server && cargo build --tests
```

Expected: clean. Fix any namespace nits.

- [ ] **Step 3: Add WS integration tests**

Append to `packages/server/tests/chat_attachments.rs`. Use the actual WS-test harness from `packages/server/tests/common/mod.rs`:

- `common::connect(server.addr, "test-token").await -> Ws` (NOT `open_ws`)
- `common::send_intent(&mut ws, json!(...)).await`
- `common::next_event(&mut ws, Duration::from_secs(2)).await -> serde_json::Value` (NOT `recv_event`)

To put a meeting in `active` state for the chat-intent gate, send the existing `start_meeting` intent over the same WS connection (look at `packages/server/tests/state_machine.rs:36` `start_meeting_with_metadata` for the canonical shape) and drain the resulting `Snapshot` / `MeetingStateChanged` events before sending the chat intent.

```rust
// --- WS-path tests ---

use std::time::Duration;

/// Helper: drain initial Snapshot / handshake events until a
/// MeetingStateChanged Active arrives, then return. Implementer:
/// the exact event sequence is in `state_machine.rs`; emulate it.
async fn drive_to_active_meeting(ws: &mut common::Ws, meeting_id: &str) {
    // Send the existing "start_meeting" intent. The intent's exact
    // shape lives in `packages/server/src/contract.rs` and a working
    // example is in `packages/server/tests/state_machine.rs`.
    common::send_intent(ws, serde_json::json!({
        "type": "start_meeting",
        "meeting_id": meeting_id,
        // ... any required fields (metadata etc.) — copy from state_machine.rs
    })).await;

    // Drain until we see the active-state confirmation.
    for _ in 0..10 {
        let evt = common::next_event(ws, Duration::from_secs(2)).await;
        if evt["type"] == "meeting_state_changed" && evt["meeting_state"] == "active" {
            return;
        }
    }
    panic!("never reached active state");
}

#[tokio::test]
async fn chat_with_one_attachment_happy() {
    let _ = dotenvy::dotenv();
    let server = spawn_test_server().await;
    let meeting_id = seed_meeting_for(TEST_USER).await;

    // Upload one attachment via HTTP first.
    let resp = reqwest::Client::new()
        .post(format!(
            "http://{}/meetings/{}/chat_attachments",
            server.addr, meeting_id
        ))
        .header("content-type", "image/png")
        .body(b"\x89PNG\r\n\x1a\nfake".to_vec())
        .send().await.unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let att_id = body["id"].as_str().unwrap().to_string();

    let mut ws = common::connect(server.addr, "test-token").await;
    drive_to_active_meeting(&mut ws, &meeting_id).await;

    common::send_intent(&mut ws, serde_json::json!({
        "type": "chat",
        "text": "describe this",
        "attachment_ids": [att_id],
    })).await;

    // No error event should arrive within 2s. (We don't assert on
    // the agent's text reply — that requires a real LLM; the harness
    // has AURIS_LLM_DISABLED=1.)
    let evt = tokio::time::timeout(
        Duration::from_secs(2),
        common::next_event(&mut ws, Duration::from_secs(5)),
    ).await;
    if let Ok(e) = evt {
        assert_ne!(e["type"], "error", "unexpected error event: {e:#?}");
    }
}

#[tokio::test]
async fn chat_with_unknown_attachment_emits_not_found() {
    let _ = dotenvy::dotenv();
    let server = spawn_test_server().await;
    let meeting_id = seed_meeting_for(TEST_USER).await;

    let mut ws = common::connect(server.addr, "test-token").await;
    drive_to_active_meeting(&mut ws, &meeting_id).await;

    common::send_intent(&mut ws, serde_json::json!({
        "type": "chat",
        "text": "hi",
        "attachment_ids": ["does-not-exist"],
    })).await;

    let evt = common::next_event(&mut ws, Duration::from_secs(2)).await;
    assert_eq!(evt["type"], "error");
    assert_eq!(evt["code"], "chat_attachment_not_found");
}

#[tokio::test]
async fn chat_with_cross_tenant_attachment_emits_forbidden() {
    let _ = dotenvy::dotenv();
    let server = spawn_test_server().await;

    // Attachment row + bytes owned by a different user.
    let foreign_meeting_id = seed_meeting_for("foreign-user").await;
    let pool = db::open_pool().await.unwrap();
    let att_id = ulid::Ulid::new().to_string();
    let rel = format!("blobs/meetings/{foreign_meeting_id}/chat/{att_id}.png");
    let dir = db::data_dir().expect("data_dir");
    let abs = dir.join(&rel);
    tokio::fs::create_dir_all(abs.parent().unwrap()).await.unwrap();
    tokio::fs::write(&abs, b"\x89PNG\r\n\x1a\nfake").await.unwrap();
    db::insert_chat_attachment(
        &pool, &att_id, &foreign_meeting_id, "foreign-user",
        "image/png", &rel, 11,
    ).await.unwrap();

    // dev|local tries to use foreign attachment in their own meeting.
    let own_meeting_id = seed_meeting_for(TEST_USER).await;
    let mut ws = common::connect(server.addr, "test-token").await;
    drive_to_active_meeting(&mut ws, &own_meeting_id).await;

    common::send_intent(&mut ws, serde_json::json!({
        "type": "chat",
        "text": "describe",
        "attachment_ids": [att_id],
    })).await;

    let evt = common::next_event(&mut ws, Duration::from_secs(2)).await;
    assert_eq!(evt["type"], "error");
    assert_eq!(evt["code"], "chat_attachment_forbidden");
}

#[tokio::test]
async fn chat_with_wrong_meeting_attachment_emits_forbidden() {
    let _ = dotenvy::dotenv();
    let server = spawn_test_server().await;

    let meeting_a = seed_meeting_for(TEST_USER).await;
    let meeting_b = seed_meeting_for(TEST_USER).await;

    // Upload under meeting_a.
    let resp = reqwest::Client::new()
        .post(format!(
            "http://{}/meetings/{}/chat_attachments",
            server.addr, meeting_a
        ))
        .header("content-type", "image/png")
        .body(b"\x89PNG\r\n\x1a\n".to_vec())
        .send().await.unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let att_id = body["id"].as_str().unwrap().to_string();

    // Now chat in meeting_b.
    let mut ws = common::connect(server.addr, "test-token").await;
    drive_to_active_meeting(&mut ws, &meeting_b).await;

    common::send_intent(&mut ws, serde_json::json!({
        "type": "chat",
        "text": "describe",
        "attachment_ids": [att_id],
    })).await;

    let evt = common::next_event(&mut ws, Duration::from_secs(2)).await;
    assert_eq!(evt["code"], "chat_attachment_forbidden");
}

#[tokio::test]
async fn chat_with_disk_read_failure_emits_unreadable() {
    let _ = dotenvy::dotenv();
    let server = spawn_test_server().await;
    let meeting_id = seed_meeting_for(TEST_USER).await;

    // Row with bogus bytes_path pointing to nothing on disk.
    let pool = db::open_pool().await.unwrap();
    let phantom_id = ulid::Ulid::new().to_string();
    db::insert_chat_attachment(
        &pool, &phantom_id, &meeting_id, TEST_USER,
        "image/png", "blobs/meetings/nope/chat/phantom.png", 42,
    ).await.unwrap();

    let mut ws = common::connect(server.addr, "test-token").await;
    drive_to_active_meeting(&mut ws, &meeting_id).await;

    common::send_intent(&mut ws, serde_json::json!({
        "type": "chat",
        "text": "?",
        "attachment_ids": [phantom_id],
    })).await;

    let evt = common::next_event(&mut ws, Duration::from_secs(2)).await;
    assert_eq!(evt["code"], "chat_attachment_unreadable");
}
```

- [ ] **Step 4: Run tests, expect pass**

```bash
cd packages/server && cargo test --test chat_attachments -- --nocapture
```

Expected: all eleven tests (six HTTP from Task 2 + five WS) green.

- [ ] **Step 5: Commit**

```bash
git add packages/server/src/ws.rs packages/server/tests/chat_attachments.rs
git commit -m "$(cat <<'EOF'
feat(server): WS chat handler resolves attachment_ids to bytes

Loads each attachment row, ownership-checks (meeting + user must
match), reads PNG bytes from disk, packs into AttachmentPayloads
on AgentKickReason::ChatMessage. New error event codes:
  chat_attachment_not_found
  chat_attachment_forbidden
  chat_attachment_unreadable

Empty text + non-empty attachments is now a valid send; empty +
empty is still a no-op.
EOF
)"
```

---

## Task 6: Mac Protocol + MeetingsAPI

**Files:**
- Modify: `packages/mac/Sources/Auris/Net/Protocol.swift` (extend `ChatIntent`)
- Modify: `packages/mac/Sources/Auris/Net/MeetingsAPI.swift` (add `uploadChatAttachment`)

- [ ] **Step 1: Extend `ChatIntent`**

Replace the current `ChatIntent` (around line 189 of `packages/mac/Sources/Auris/Net/Protocol.swift`):

```swift
/// User-typed question to the agent during an active meeting. The
/// server validates active/paused state, kicks the agent, and the
/// resulting Q+A pair lands in chat-mode `items_update` events.
///
/// `attachmentIds` (added 2026-05-12) carries chat-attachment ids
/// previously returned by `POST /meetings/:id/chat_attachments`.
/// Default `[]` matches today's text-only chats.
struct ChatIntent: Encodable {
    let type: String = "chat"
    let text: String
    let attachmentIds: [String]

    init(text: String, attachmentIds: [String] = []) {
        self.text = text
        self.attachmentIds = attachmentIds
    }

    enum CodingKeys: String, CodingKey {
        case type
        case text
        case attachmentIds = "attachment_ids"
    }
}
```

- [ ] **Step 2: Add `uploadChatAttachment` to `MeetingsAPI`**

Add right after `uploadMomentScreenshot` in `packages/mac/Sources/Auris/Net/MeetingsAPI.swift` (around line 245). Mirror the existing pattern exactly — `URLRequest` direct construction, `URLSession.shared.upload(for:from:)`, `MeetingsAPIError` enum:

```swift
/// POST raw PNG bytes to `/meetings/<id>/chat_attachments` and return
/// the server-assigned attachment id. Throws via `MeetingsAPIError`
/// on transport or non-2xx HTTP errors. Mirrors
/// `uploadMomentScreenshot` exactly except for the path and the
/// JSON-decoded response body.
func uploadChatAttachment(meetingId: String, png: Data) async throws -> String {
    let url = baseURL.appendingPathComponent(
        "meetings/\(meetingId)/chat_attachments"
    )
    var req = URLRequest(url: url)
    req.httpMethod = "POST"
    req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
    req.setValue("image/png", forHTTPHeaderField: "Content-Type")

    let data: Data
    let resp: URLResponse
    do {
        (data, resp) = try await URLSession.shared.upload(for: req, from: png)
    } catch {
        throw MeetingsAPIError.transport(error)
    }
    guard let http = resp as? HTTPURLResponse else {
        throw MeetingsAPIError.http(0)
    }
    switch http.statusCode {
    case 200..<300:
        struct UploadResponse: Decodable { let id: String }
        let decoded = try JSONDecoder().decode(UploadResponse.self, from: data)
        return decoded.id
    case 401: throw MeetingsAPIError.unauthorized
    case 404: throw MeetingsAPIError.notFound
    default:  throw MeetingsAPIError.http(http.statusCode)
    }
}
```

- [ ] **Step 3: Compile-check**

```bash
cd packages/mac && swift build
```

Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add packages/mac/Sources/Auris/Net/Protocol.swift \
        packages/mac/Sources/Auris/Net/MeetingsAPI.swift
git commit -m "$(cat <<'EOF'
feat(mac): ChatIntent.attachmentIds + uploadChatAttachment API

Extends the wire ChatIntent with attachment_ids (default []) and
adds a raw-PNG upload helper that mirrors uploadMomentScreenshot.
No callers yet — wiring only.
EOF
)"
```

---

## Task 7: Mac AppModel — capture, upload, queue, send

**Files:**
- Modify: `packages/mac/Sources/Auris/AppModel.swift`
- Create: `packages/mac/Tests/AurisTests/AppModelChatAttachmentTests.swift`

- [ ] **Step 1: Add `ChatAttachmentDraft` + state machine**

Add at the top of `packages/mac/Sources/Auris/AppModel.swift` (after the existing top-level type definitions, before the `AppModel` class):

```swift
/// Upload state for a single staged chat attachment.
enum ChatAttachmentUploadState: Equatable {
    case uploading
    case uploaded(id: String)
    case failed(message: String)
}

/// A screenshot the user has staged for the next chat send. Held in
/// `AppModel.pendingChatAttachments` between capture and send.
struct ChatAttachmentDraft: Identifiable, Equatable {
    /// Local-only id so SwiftUI can key the chip strip. Distinct from
    /// the server-assigned attachment id (which lives in `state`
    /// once upload completes).
    let id: UUID

    /// Source PNG for the thumbnail render. Retained for the chip's
    /// lifetime in the strip.
    let image: NSImage

    /// Raw PNG bytes, kept until upload succeeds. Cleared after
    /// `.uploaded` to free memory; the server has the only copy now.
    var pngBytes: Data

    /// Upload lifecycle.
    var state: ChatAttachmentUploadState

    static func == (lhs: ChatAttachmentDraft, rhs: ChatAttachmentDraft) -> Bool {
        lhs.id == rhs.id && lhs.state == rhs.state
    }
}
```

- [ ] **Step 2: Add `AppModel` state + the methods**

Inside the `AppModel` class in `packages/mac/Sources/Auris/AppModel.swift`, add (look for the existing chat-related properties / methods around `sendChat` at line 787 — keep the additions near them for cohesion):

```swift
// MARK: - Chat attachments

@Published private(set) var pendingChatAttachments: [ChatAttachmentDraft] = []
private let chatAttachmentLimit = 4

/// Transient toast-like message for chat-attachment / chat-send
/// status (mirrors `momentStatus` for the moment subsystem). Cleared
/// by `scheduleChatStatusClear()` after a short delay so users see
/// the message but it doesn't linger.
@Published var chatStatus: String? = nil

private func scheduleChatStatusClear() {
    Task { [weak self] in
        try? await Task.sleep(for: .seconds(2))
        self?.chatStatus = nil
    }
}

/// Capture the primary display, append a chip in `.uploading` state,
/// and fire the upload Task. Surfaces capture errors via `chatStatus`.
/// Gated: must be in an active or paused meeting, must be below the
/// 4-cap. The caller (UI) is responsible for disabling the button
/// when these gates fail, but the function still rejects defensively.
func captureChatAttachment() async {
    guard meetingState == .active || meetingState == .paused else { return }
    guard pendingChatAttachments.count < chatAttachmentLimit else { return }
    guard let meetingId = currentMeetingId else { return }

    let png: Data
    do {
        png = try await ScreenshotCapture.capturePrimaryDisplay()
    } catch {
        chatStatus = "Capture failed: \(error.localizedDescription)"
        scheduleChatStatusClear()
        return
    }

    let draft = ChatAttachmentDraft(
        id: UUID(),
        image: NSImage(data: png) ?? NSImage(),
        pngBytes: png,
        state: .uploading
    )
    pendingChatAttachments.append(draft)

    Task { [weak self] in
        await self?.uploadChatAttachment(draftId: draft.id, meetingId: meetingId, png: png)
    }
}

/// Remove a chip from the local queue. The server-side row (if upload
/// already landed) is left as an orphan and cleaned up when the
/// parent meeting is deleted.
func removeChatAttachment(draftId: UUID) {
    pendingChatAttachments.removeAll { $0.id == draftId }
}

private func uploadChatAttachment(draftId: UUID, meetingId: String, png: Data) async {
    let api: MeetingsAPI
    do {
        api = try await makeMeetingsAPI()
    } catch {
        updateDraft(draftId: draftId) { d in
            d.state = .failed(message: "API setup failed: \(error.localizedDescription)")
        }
        return
    }
    do {
        let id = try await api.uploadChatAttachment(meetingId: meetingId, png: png)
        updateDraft(draftId: draftId) { d in
            d.state = .uploaded(id: id)
            d.pngBytes = Data()        // we have the id; drop the bytes
        }
    } catch {
        updateDraft(draftId: draftId) { d in
            d.state = .failed(message: error.localizedDescription)
        }
    }
}

private func updateDraft(draftId: UUID, mutate: (inout ChatAttachmentDraft) -> Void) {
    guard let idx = pendingChatAttachments.firstIndex(where: { $0.id == draftId }) else {
        return
    }
    var d = pendingChatAttachments[idx]
    mutate(&d)
    pendingChatAttachments[idx] = d
}

/// Whether `sendChat` is currently allowed to dispatch. Blocked while
/// any attachment is mid-upload.
var canSendChatNow: Bool {
    !pendingChatAttachments.contains { if case .uploading = $0.state { true } else { false } }
}
```

`currentMeetingId` is the existing accessor on `AppModel` that the moments code uses to attach a moment to the right meeting (`grep -n "currentMeetingId\|activeMeetingId" packages/mac/Sources/Auris/AppModel.swift`). If the field is named differently, use the correct one.

- [ ] **Step 3: Extend `sendChat` to drain attachments and ship the typed intent**

Find `sendChat` at line 787 in `packages/mac/Sources/Auris/AppModel.swift`. Replace its body:

```swift
func sendChat(_ text: String) async {
    let trimmed = text.trimmingCharacters(in: .whitespaces)
    let hasText = !trimmed.isEmpty
    let hasAttachments = !pendingChatAttachments.isEmpty
    guard hasText || hasAttachments else { return }

    // Block while uploads are in flight; preserve the user's typing.
    if !canSendChatNow {
        chatStatus = "Waiting for screenshots to upload…"
        scheduleChatStatusClear()
        return
    }

    // Drain BEFORE send (atomic). Mirrors meeting-picker pattern.
    let uploadedIds: [String] = pendingChatAttachments.compactMap { d in
        if case .uploaded(let id) = d.state { return id } else { return nil }
    }
    let failedCount = pendingChatAttachments.count - uploadedIds.count
    pendingChatAttachments = []

    if failedCount > 0 {
        // Soft warning: we silently dropped failed chips. If the user
        // wanted them they would have removed-and-retried already.
        chatStatus = "Skipped \(failedCount) failed screenshot\(failedCount == 1 ? "" : "s")"
        scheduleChatStatusClear()
    }

    do {
        try await webSocket.send(intent: ChatIntent(text: trimmed, attachmentIds: uploadedIds))
    } catch {
        // Preserve text in the UI; do not re-stage attachments
        // (server has the bytes; chips would need fresh state).
        chatStatus = "Send failed: \(error.localizedDescription)"
        scheduleChatStatusClear()
    }
}
```

- [ ] **Step 4: Clear `pendingChatAttachments` on meeting-end**

Locate the existing handler that resets per-meeting state when `meetingState` transitions to `.idle` (e.g. inside `clearTranscript()` around `AppModel.swift:328`, or wherever the existing chat scroll is reset). Add:

```swift
pendingChatAttachments = []
```

to that same reset point.

- [ ] **Step 5: Write Mac unit tests**

Create `packages/mac/Tests/AurisTests/AppModelChatAttachmentTests.swift`:

```swift
import XCTest
@testable import Auris

@MainActor
final class AppModelChatAttachmentTests: XCTestCase {
    func test_remove_attachment_drops_chip() async throws {
        let model = AppModel.makeTestInstance(meetingState: .active, meetingId: "m1")
        let draft = ChatAttachmentDraft(
            id: UUID(),
            image: NSImage(),
            pngBytes: Data([0x89, 0x50, 0x4E, 0x47]),
            state: .uploaded(id: "att-1")
        )
        // Use the test hook to seed a draft directly (see below).
        model.testInjectChatAttachment(draft)
        XCTAssertEqual(model.pendingChatAttachments.count, 1)

        model.removeChatAttachment(draftId: draft.id)
        XCTAssertTrue(model.pendingChatAttachments.isEmpty)
    }

    func test_canSendChatNow_false_while_uploading() async throws {
        let model = AppModel.makeTestInstance(meetingState: .active, meetingId: "m1")
        model.testInjectChatAttachment(ChatAttachmentDraft(
            id: UUID(), image: NSImage(), pngBytes: Data(), state: .uploading
        ))
        XCTAssertFalse(model.canSendChatNow)
    }

    func test_canSendChatNow_true_when_all_uploaded() async throws {
        let model = AppModel.makeTestInstance(meetingState: .active, meetingId: "m1")
        model.testInjectChatAttachment(ChatAttachmentDraft(
            id: UUID(), image: NSImage(), pngBytes: Data(), state: .uploaded(id: "x")
        ))
        XCTAssertTrue(model.canSendChatNow)
    }

    func test_canSendChatNow_true_when_failed() async throws {
        // Failed uploads don't block send — they just get skipped on drain.
        let model = AppModel.makeTestInstance(meetingState: .active, meetingId: "m1")
        model.testInjectChatAttachment(ChatAttachmentDraft(
            id: UUID(), image: NSImage(), pngBytes: Data(), state: .failed(message: "nope")
        ))
        XCTAssertTrue(model.canSendChatNow)
    }
}
```

Add the test hooks to `AppModel`:

```swift
#if DEBUG
extension AppModel {
    static func makeTestInstance(meetingState: MeetingState, meetingId: String?) -> AppModel {
        let m = AppModel(/* whatever the canonical test init takes */)
        m.meetingState = meetingState
        m.currentMeetingId = meetingId
        return m
    }
    func testInjectChatAttachment(_ draft: ChatAttachmentDraft) {
        pendingChatAttachments.append(draft)
    }
}
#endif
```

If `AppModel`'s initializer requires arguments (websocket, etc.), grep for an existing test that constructs an `AppModel` (`grep -rn "AppModel(" packages/mac/Tests/`) and copy the canonical construction shape.

- [ ] **Step 6: Run Mac tests; expect pass**

```bash
cd packages/mac && swift test
```

Expected: the four new tests pass; no existing tests regress.

- [ ] **Step 7: Commit**

```bash
git add packages/mac/Sources/Auris/AppModel.swift \
        packages/mac/Tests/AurisTests/AppModelChatAttachmentTests.swift
git commit -m "$(cat <<'EOF'
feat(mac): AppModel staging + send-time drain of chat attachments

Adds ChatAttachmentDraft state machine, captureChatAttachment /
removeChatAttachment / uploadChatAttachment, canSendChatNow gate,
and a sendChat extension that drains uploaded ids into
ChatIntent.attachmentIds before WS dispatch. Cap of 4 enforced
defensively. Strip cleared on meeting-state → idle.
EOF
)"
```

---

## Task 8: Mac UI — chip strip + camera button

**Files:**
- Modify: `packages/mac/Sources/Auris/MeetingOverlayView.swift`

- [ ] **Step 1: Add `ChatAttachmentStrip` and `ChatAttachmentChip` views**

Append to `packages/mac/Sources/Auris/MeetingOverlayView.swift` (near where other compose-related views live):

```swift
struct ChatAttachmentStrip: View {
    @Bindable var model: AppModel

    var body: some View {
        if !model.pendingChatAttachments.isEmpty {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(model.pendingChatAttachments) { draft in
                        ChatAttachmentChip(draft: draft) {
                            model.removeChatAttachment(draftId: draft.id)
                        }
                    }
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
            }
            .frame(height: 72)
        }
    }
}

struct ChatAttachmentChip: View {
    let draft: ChatAttachmentDraft
    let onRemove: () -> Void

    var body: some View {
        ZStack(alignment: .topTrailing) {
            Image(nsImage: draft.image)
                .resizable()
                .aspectRatio(contentMode: .fill)
                .frame(width: 64, height: 64)
                .clipShape(RoundedRectangle(cornerRadius: 6))
                .overlay(stateOverlay)
                .overlay(
                    RoundedRectangle(cornerRadius: 6)
                        .stroke(borderColor, lineWidth: 1)
                )

            Button(action: onRemove) {
                Image(systemName: "xmark.circle.fill")
                    .foregroundStyle(.white, .black.opacity(0.65))
                    .font(.system(size: 14))
            }
            .buttonStyle(.plain)
            .offset(x: 4, y: -4)
        }
        .help(tooltip)
    }

    @ViewBuilder private var stateOverlay: some View {
        switch draft.state {
        case .uploading:
            ProgressView()
                .controlSize(.small)
                .padding(4)
                .background(.black.opacity(0.4), in: Circle())
        case .failed:
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.yellow)
                .padding(4)
                .background(.black.opacity(0.4), in: Circle())
        case .uploaded:
            EmptyView()
        }
    }

    private var borderColor: Color {
        switch draft.state {
        case .uploading: return .secondary.opacity(0.5)
        case .failed:    return .red
        case .uploaded:  return .clear
        }
    }

    private var tooltip: String {
        switch draft.state {
        case .uploading:               return "Uploading screenshot…"
        case .failed(let msg):         return "Upload failed: \(msg)"
        case .uploaded:                return "Screenshot ready"
        }
    }
}
```

- [ ] **Step 2: Slot the strip and camera button into the chat compose region**

In `packages/mac/Sources/Auris/MeetingOverlayView.swift`, find where the chat input is rendered (search for `TextField` or `sendChat` to locate it — there's only one chat input in the codebase). Wrap the existing input row in a `VStack` so the strip sits above it. Add the camera button just before the existing send button inside the input row:

```swift
VStack(spacing: 4) {
    ChatAttachmentStrip(model: model)

    HStack(spacing: 6) {
        TextField("ask the agent anything…", text: $chatInputText)
            // ... existing modifiers ...

        Button {
            Task { await model.captureChatAttachment() }
        } label: {
            Image(systemName: "camera.fill")
                .font(.system(size: 14, weight: .medium))
        }
        .buttonStyle(.plain)
        .disabled(!canCaptureChatAttachment)
        .help(captureButtonTooltip)

        Button {
            // existing send action
        } label: {
            // existing send label
        }
        // ... existing send modifiers ...
    }
}
```

Add the gating computed properties near the existing `chatInputText` state in the same view:

```swift
private var canCaptureChatAttachment: Bool {
    (model.meetingState == .active || model.meetingState == .paused)
        && model.pendingChatAttachments.count < 4
}

private var captureButtonTooltip: String {
    guard model.meetingState == .active || model.meetingState == .paused else {
        return "Start a meeting to attach screenshots"
    }
    if model.pendingChatAttachments.count >= 4 {
        return "Maximum 4 screenshots per message"
    }
    return "Capture screen"
}
```

- [ ] **Step 3: Build the Mac app and visually verify**

```bash
cd packages/mac && swift build
```

Expected: clean build.

Then manually:

```bash
just mac-run    # or whatever recipe launches the Mac app from a dev session
```

In the running app:
1. Start a meeting (any flow — local or PWA-initiated against a dev server).
2. Switch to chat mode in the overlay.
3. Click the camera button — confirm a chip appears with a thumbnail of the current display, with an uploading spinner that resolves to a clean thumbnail.
4. Click the X on a chip — chip disappears.
5. Capture 4 — confirm the button greys out and the tooltip says "Maximum 4 screenshots per message".
6. Type a message + click send — chips clear, the chat shows the user's text bubble (no attachment thumbs in chat history is expected per spec section 12).
7. End the meeting — confirm the strip is empty for the next meeting.

- [ ] **Step 4: Commit**

```bash
git add packages/mac/Sources/Auris/MeetingOverlayView.swift
git commit -m "$(cat <<'EOF'
feat(mac): chat compose strip + camera button for screenshot attachments

Adds ChatAttachmentStrip (horizontal scrollable, hidden when empty)
and ChatAttachmentChip (64×64 thumb, upload-state overlay, X-close).
Camera button slotted in the chat input row's trailing controls,
disabled when out of an active meeting or at the 4-cap with tooltips
that explain why. Capture is instant on click (no delay, no countdown).
EOF
)"
```

---

## Task 9: End-to-end smoke

**Files:** none (manual verification).

- [ ] **Step 1: Build + start the server against a real LLM provider**

```bash
just server-run   # or however the project's recipe brings the server up with a real Anthropic/OpenAI/Bedrock key
```

Confirm in logs the LLM provider is initialized and no migrations are pending.

- [ ] **Step 2: Launch the Mac client and sign in**

```bash
just mac-run
```

Sign in to Auth0; ensure you can see your existing meetings (PWA + Mac are on the same `sub`).

- [ ] **Step 3: Real-meeting smoke**

1. Start a meeting — let it run live with real audio, or PWA-initiate one.
2. Open a slide deck (Keynote, PowerPoint, browser-rendered) in the foreground.
3. Switch focus to the Mac overlay; switch mode to **CHAT**.
4. Without speaking, place the first slide visible behind the overlay, click the camera button. The Mac overlay is excluded from the capture (per `ScreenshotCapture.swift:62-73`), so the chip should show the slide, not the overlay.
5. Switch to a second slide, click camera again. Two chips queued.
6. Type "compare these two slides" and send.
7. Confirm the agent reply references both slides specifically (not just "I see images").

Expected: a coherent comparison referencing visual elements from both slides. If the reply is generic ("I see two images"), confirm in server logs that `agent fetch_meeting_summary` style logs *and* the build_user_message path was exercised (`grep -i "image" packages/server/server.log` for any rig provider-side errors).

- [ ] **Step 4: Permission-revocation smoke (optional)**

In **System Settings → Privacy & Security → Screen Recording**, toggle the Mac app off, then click the camera button in the running app. Expected: status toast "Capture failed: Screen Recording permission not granted." No chip added. Re-grant and confirm the next click works.

- [ ] **Step 5: Meeting-end cleanup smoke**

While 2-3 chips are queued, stop the meeting. Confirm the chip strip clears immediately. Start a fresh meeting; the strip is empty. (Disk-side, the orphaned PNGs are cleaned when the previous meeting row is deleted — verify after a `DELETE /meetings/:id` that `<data_dir>/blobs/meetings/<previous_id>` is gone.)

- [ ] **Step 6: Tag the milestone**

If everything passes, no commit is needed (Task 8 was the final code commit). Optionally:

```bash
git log --oneline --grep="chat.*attachment\|chat_attachments" | head -10
```

…to confirm the commit chain is contiguous and readable.

---

## Plan self-review

Mapping spec sections → tasks:

| Spec section | Covered by |
|---|---|
| §4 data flow | All tasks combined; explicit in Task 5 (WS handler) and Task 7 (Mac send) |
| §5 contract change | Task 3 step 1 |
| §6.1 DB table | Task 1 step 1-2 |
| §6.2 HTTP route | Task 2 |
| §6.3 WS handler | Task 5 |
| §6.4 agent vision construction | Task 4 |
| §7.1 state machine | Task 7 step 1 |
| §7.2 AppModel additions | Task 7 step 2 |
| §7.3 send flow | Task 7 step 3 |
| §7.4 ChatIntent | Task 6 step 1 |
| §7.5 MeetingsAPI | Task 6 step 2 |
| §8 UI strip + chip + button | Task 8 |
| §9 errors mapping | Task 5 (server emit), Task 8 (UI surfacing via existing `chatStatus`) |
| §10 lifecycle / GC | Task 1 (DB cascade), Task 5 (no new GC code needed), Task 7 step 4 (clear on idle) |
| §11 testing pyramid | Tasks 1 (db tests), 2 (HTTP), 4 (agent helper), 5 (WS), 7 (Mac unit), 9 (manual smoke) |
| §13 fast-follows | Not implemented — out of scope by design |
