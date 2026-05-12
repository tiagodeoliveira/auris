# Chat screenshot attachments (Mac)

**Status:** Approved (design)
**Date:** 2026-05-12
**Scope:** Mac client only (v1). PWA and mobile do not get the attach UI in this iteration.

## 1. Problem

During a meeting, the user wants to capture multiple screenshots of slides (or any on-screen content) and send them — together with a single text question — to the chat agent. Today the chat only accepts text. Use case: _"give me a summary of these two slides."_

These are **not** moments. Moments are an existing concept (auto-summarized meaningful events in a meeting with optional auto-screenshot). Chat attachments are a separate, lighter mechanism: ephemeral attachments to a single chat exchange.

## 2. Goals

- Capture one or more screenshots during an active meeting and attach them to the next chat message.
- Reuse the existing `ScreenshotCapture.capturePrimaryDisplay()` path. No new TCC permission. No new picker UI. No new capture overlay.
- The agent receives the screenshots as vision content blocks and produces a normal chat reply.
- Smallest possible blast radius on the contract, DB, and other clients.

## 3. Non-goals (v1)

- PWA / mobile attach UI.
- Crop / region selection.
- Annotation overlays (arrows, highlights).
- Tap-to-retry on failed upload.
- Global hotkey for capture.
- Persisting chat (and therefore chat attachments) across meetings.
- `GET /chat_attachments/:id` and `DELETE /chat_attachments/:id` HTTP routes.
- JPEG / HEIC / non-PNG mimes.
- Showing attachments inline in chat-history bubbles (chat mode uses Replace strategy with a single Q+A pair).

## 4. End-to-end data flow

```
Mac:
  [📷 button] → ScreenshotCapture.capturePrimaryDisplay() → PNG bytes
                          ↓
            pendingChatAttachments: [ChatAttachmentDraft]   (max 4)
                          ↓
    POST /meetings/:id/chat_attachments   (Content-Type: image/png)
                          ↓
                      { id: "01HZ..." }
                          ↓
    WS  Intent::Chat { text, attachment_ids: [id, ...] }
                          ↓
Server:
  ws.rs Chat handler: load PNG bytes from disk per id (ownership-checked),
    then agent.run_chat(text, attachments).
  agent.rs: builds RigMessage::User with [Text?, Image, Image, ...]
    content parts; rig translates per provider (Anthropic/OpenAI/Bedrock).
  Reply: user-bubble Item (meta.attachment_ids) + assistant Item.
  Disk: <data_dir>/blobs/meetings/<meeting_id>/chat/<attachment_id>.png
        (parallel to existing moments path
         blobs/meetings/<meeting_id>/screenshots/<moment_id>.png)
  DB:   chat_attachments(id, meeting_id FK CASCADE, user_id, mime,
                         bytes_path, bytes_size, created_at)
```

## 5. Contract change

`packages/server/src/contract.rs`:

```rust
Chat {
    text: String,
    /// Attachment ids previously uploaded via
    /// `POST /meetings/:id/chat_attachments`. Server reads the PNG
    /// bytes from disk and threads them into the vision call.
    /// Empty by default → today's behavior. Mac-only producer in v1;
    /// PWA/mobile keep sending `Chat { text }` and serde defaults
    /// `attachment_ids` to `[]`.
    #[serde(default)]
    attachment_ids: Vec<String>,
}
```

Backwards-compatible. Existing PWA/mobile builds keep functioning unchanged.

`Item.meta` (already `serde_json::Value`) gains a soft convention: chat user-side bubbles emitted by the server include `meta.attachment_ids: [String]`. v1 UI does not render thumbnails from this field; it exists for future renderers.

## 6. Server changes

### 6.1 New DB table

```sql
CREATE TABLE chat_attachments (
    id          TEXT PRIMARY KEY,             -- ulid()
    meeting_id  TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    user_id     TEXT NOT NULL,                -- defense in depth
    mime        TEXT NOT NULL,                -- "image/png"
    bytes_path  TEXT NOT NULL,                -- relative to crate::db::data_dir()
                                              -- (e.g. "blobs/meetings/<mid>/chat/<aid>.png")
    bytes_size  INTEGER NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX chat_attachments_meeting_id_idx ON chat_attachments (meeting_id);
```

Filesystem cascade is handled by the existing `DELETE /meetings/:id` cleanup path, which recursively wipes the meeting's screenshot directory.

### 6.2 New HTTP route

`POST /meetings/:id/chat_attachments`

- Auth: existing JWT bearer.
- `Content-Type: image/png` (case-insensitive) — anything else returns 400.
- Body: raw PNG bytes (no multipart). Reuses the existing
  `SCREENSHOT_BODY_LIMIT` (currently 64 MiB, defined at `api.rs:130`) via
  the same `DefaultBodyLimit::max(SCREENSHOT_BODY_LIMIT)` layer pattern
  the moment route already uses (`api.rs:156`). Empty body returns 400.
- Ownership: rejects if `meetings.user_id != caller`.
- Writes to `<data_dir>/blobs/meetings/<meeting_id>/chat/<attachment_id>.png`
  using `crate::db::data_dir()` (same helper as the moment upload path).
- Inserts a `chat_attachments` row with `bytes_path` set to the relative
  path `blobs/meetings/<meeting_id>/chat/<attachment_id>.png`.
- Returns `201 { "id": "<ulid>" }`.
- Errors: 400 (mime/empty), 401 (no JWT), 403 (cross-tenant), 404 (meeting missing), 413 (oversize, surfaced by axum's body-limit layer).

### 6.3 WS `Intent::Chat` handler

In `packages/server/src/ws.rs`, when handling `Intent::Chat { text, attachment_ids }`:

1. Existing checks (meeting active/paused) still apply; additionally allow empty `text` if `attachment_ids` is non-empty.
2. For each `attachment_id` in order:
   - Look up row by id; reject with `chat_attachment_not_found` if missing.
   - Verify `row.meeting_id == current_meeting_id` AND `row.user_id == caller`; reject with `chat_attachment_forbidden` otherwise.
   - Read bytes from `crate::db::data_dir().join(&row.bytes_path)`; on failure send `chat_attachment_unreadable` and abort the chat call (don't half-run).
3. Build `user_item_meta = { "role": "user", "attachment_ids": [...] }` and pass to the agent.
4. Call `agent.run_chat(user_id, current_meeting_id, text, attachments, user_item_meta)`.

### 6.4 Agent vision message construction

`packages/server/src/summarizer/agent.rs` gains a `build_user_message(text, attachments) -> RigMessage` helper:

```rust
fn build_user_message(text: String, attachments: Vec<AttachmentPayload>) -> RigMessage {
    let mut parts: Vec<UserContent> = Vec::with_capacity(1 + attachments.len());
    if !text.trim().is_empty() {
        parts.push(UserContent::Text(text.into()));
    }
    for a in attachments {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&a.bytes);
        parts.push(UserContent::Image(Image {
            data: DocumentSourceKind::Base64(b64),
            media_type: Some(ImageMediaType::Png),
            detail: None,
            additional_params: None,
        }));
    }
    RigMessage::User {
        content: OneOrMany::many(parts).expect("at least one content part"),
    }
}
```

`OneOrMany::many` is safe: the WS handler guarantees text-or-attachments is non-empty.

The rest of the agent loop (tool calls, fetch_meeting_summary, etc.) is unchanged. Images appear only in the initial user turn; subsequent turns are tool-result + assistant text.

## 7. Mac client changes

### 7.1 State machine

```swift
enum ChatAttachmentUploadState: Equatable {
    case uploading
    case uploaded(id: String)
    case failed(message: String)
}

struct ChatAttachmentDraft: Identifiable, Equatable {
    let id: UUID                    // local-only tempId
    let image: NSImage              // for thumbnail rendering
    var pngBytes: Data              // cleared after upload succeeds
    var state: ChatAttachmentUploadState
}
```

### 7.2 AppModel additions

```swift
private(set) var pendingChatAttachments: [ChatAttachmentDraft] = []
private let chatAttachmentLimit = 4

func captureChatAttachment() async              // capture + start upload
private func uploadChatAttachment(...) async    // POST + mutate draft
func removeChatAttachment(draftId: UUID)        // drop from local array
```

`captureChatAttachment` is gated on:

- `meetingState in {active, paused}`
- `pendingChatAttachments.count < chatAttachmentLimit`
- non-nil `currentMeetingId`

On success it appends a draft in `.uploading` state and fires the upload Task. On capture failure it surfaces an inline status toast (`chatStatus`) and does not add a chip.

### 7.3 Send flow

`sendChat(text)` (existing) is extended:

- Send is allowed when `text` non-empty OR `pendingChatAttachments` non-empty (server enforces same).
- Send is **blocked** while any draft is in `.uploading` state — `chatStatus` shows "Waiting for screenshots to upload…", user's text is preserved.
- Drains `pendingChatAttachments` _before_ the WS send (atomic clear, mirrors meeting-picker pattern).
- WS payload: `ChatIntent { text, attachmentIds: [ids from .uploaded drafts] }`.
- On WS-send failure the user's text is preserved; chips are _not_ re-staged (server already accepted the upload; orphans cascade-clean with the meeting).
- On meetingState → idle, `pendingChatAttachments` is cleared.

### 7.4 Protocol additions

`packages/mac/Sources/MeetingCompanion/Net/Protocol.swift`:

```swift
struct ChatIntent: Encodable {
    let type: String = "chat"
    let text: String
    let attachmentIds: [String]
    enum CodingKeys: String, CodingKey {
        case type, text, attachmentIds = "attachment_ids"
    }
}
```

### 7.5 MeetingsAPI addition

```swift
// POST /meetings/:id/chat_attachments  Content-Type: image/png
func uploadChatAttachment(meetingId: String, png: Data) async throws -> String
```

Mirrors `uploadMomentScreenshot` exactly (raw PNG body, JWT bearer).

## 8. Mac UI

### 8.1 Layout (chat compose region, only in `currentMode == "chat"` while active/paused)

```
┌────────────────────────────────────────────────────────────────────────┐
│ ╭─────╮  ╭─────╮  ╭─────╮          ← pendingChatAttachments strip      │
│ │ 📷  │  │ 📷  │  │ ⚠   │             (hidden when count == 0)         │
│ │ × 1 │  │ × 2 │  │retry│                                              │
│ ╰─────╯  ╰─────╯  ╰─────╯                                              │
│                                                                         │
│ ┌──────────────────────────────────────────────────────────┬─────────┐ │
│ │  ask the agent anything…                                  │ [📷][→]│ │
│ └──────────────────────────────────────────────────────────┴─────────┘ │
└────────────────────────────────────────────────────────────────────────┘
```

### 8.2 Components

- **`ChatAttachmentStrip`** — horizontal `ScrollView` (no scroll bar) over `pendingChatAttachments`, hidden when empty. Each cell is a `ChatAttachmentChip`.
- **`ChatAttachmentChip`** — 64×64 thumbnail (`Image(nsImage:)`), state overlay (`ProgressView` while uploading / yellow ⚠ on failed), red border on failure, X-close button in the top-right corner. Tooltip surfaces the state and any error message.
- **Camera button** — slotted in the chat input row's trailing controls, before the existing send button. Disabled when not in chat mode, no active meeting, or 4-cap reached. Tooltip explains _why_ it's disabled.

### 8.3 Empty/error states

| State                                     | Behavior                                                                                             |
| ----------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| 0 chips                                   | Strip hidden, camera button enabled (if other gates pass)                                            |
| 4 chips                                   | Strip visible, camera button disabled with "Maximum 4 screenshots per message" tooltip               |
| Capture failure (TCC denied / no display) | No chip; `chatStatus` toast shows error                                                              |
| Upload in flight                          | Chip shows spinner overlay; X removes locally (server-side completes anyway)                         |
| Upload failure                            | Chip stays with red border + ⚠ overlay; tooltip = error message; X removes; no in-place retry button |
| Send while upload pending                 | Send blocked; status: "Waiting for screenshots to upload…"                                           |

## 9. Errors — server → client mapping

| Server event `code`          | Mac status message                       |
| ---------------------------- | ---------------------------------------- |
| `chat_attachment_not_found`  | "Attachment expired — re-capture"        |
| `chat_attachment_forbidden`  | "Attachment unavailable"                 |
| `chat_attachment_unreadable` | "Could not read screenshot — re-capture" |

HTTP upload errors (mime/size/auth/cross-tenant) are surfaced inline on the failing chip's tooltip.

## 10. Lifecycle / GC

- DB cascade: `chat_attachments.meeting_id` FK with `ON DELETE CASCADE`.
- Filesystem cascade: `delete_meeting` (`api.rs:524`) calls `remove_dir_all` on `<data_dir>/blobs/meetings/<meeting_id>` — which wipes both moments' `screenshots/` and our new `chat/` subdirectory in one shot. No additional cleanup code is needed.
- Therefore: no separate GC job, no TTL. Orphans (uploaded then never sent, or removed from the chip strip) live until the meeting is deleted.
- Disk pressure: bounded per-meeting at ~4 attachments × N chat exchanges × ~800 KB. Acceptable.

## 11. Testing

| Layer                                          | Coverage                                                                                                                                                                          |
| ---------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Rust unit (`agent.rs`)                         | `build_user_message`: text-only, attachments-only, mixed; ordering; `OneOrMany` invariant                                                                                         |
| Rust unit (`db.rs`)                            | `insert_chat_attachment` round-trip; `get_chat_attachment` by id; cascade-delete on parent meeting deletion                                                                       |
| Rust integration (`tests/`)                    | HTTP `POST /meetings/:id/chat_attachments`: happy (201 + id + file + row), wrong mime (400), oversize (400), empty (400), unauth (401), cross-tenant (403), unknown meeting (404) |
| Rust integration (`tests/chat_attachments.rs`) | WS `Intent::Chat`: 1 + N attachments happy path, unknown id, cross-tenant id, attachment-from-wrong-meeting id, disk-read failure (graceful error event)                          |
| Mac unit                                       | `captureChatAttachment` honors 4-cap, meeting-state gate; draft state mutates correctly on upload success/failure                                                                 |
| Manual smoke                                   | Real meeting: capture 2 slides, ask "compare these", verify agent reply references both                                                                                           |

No Mac UI snapshot tests (no framework in repo). Visual correctness verified manually.

## 12. Open behavior note (intentional)

Chat mode uses Replace strategy — sending a new chat message replaces the prior user/assistant Item pair. With attachments, the prior user-bubble's `meta.attachment_ids` field is dropped from the active items list along with the bubble; the DB rows and on-disk PNGs remain until the meeting is deleted. This is consistent with how chat history already behaves for text; v1 has no UI surface to browse historical attachments.

## 13. Fast-follows (not in v1)

1. Global hotkey (`Cmd+Shift+M` or similar) for capture while focused on the meeting app.
2. PWA file picker (drag-drop or paperclip) — same HTTP + contract.
3. Inline display of attachments in chat history bubbles, if/when chat scrollback is added.
4. Annotation overlay (arrow, highlight) pre-send.

## 14. Build sequence (informational — final order is in the plan)

1. Server: migration, DB layer, HTTP route, unit + integration tests.
2. Server: `build_user_message`, WS handler change, unit + integration tests.
3. Mac: `Protocol.swift` + `MeetingsAPI.swift` additions.
4. Mac: `AppModel` state + capture/upload/send methods + unit tests.
5. Mac: `ChatAttachmentStrip` + chip + camera button in overlay.
6. End-to-end manual smoke test in a real meeting.
