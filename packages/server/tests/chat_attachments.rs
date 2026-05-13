//! HTTP integration tests for the chat-attachments upload route,
//! plus WS integration tests for the `Intent::Chat` handler resolving
//! `attachment_ids` into agent kick payloads.
//!
//! The auth-disabled test harness pins all requests to the dev user
//! resolved via auth0_sub "dev|local". The unauthenticated-path test
//! from the spec is NOT covered here — the harness can't represent it.

mod common;
use common::spawn_test_server;

use auris_server::db;
use std::time::Duration;

/// Seed (or fetch) the dev user that the auth-disabled handler will
/// resolve to, then create a fresh meeting owned by that user.
/// Returns (user_id_uuid, meeting_id).
async fn seed_dev_user_and_meeting() -> (String, String) {
    let pool = db::open_pool().await.expect("open pool");
    let dev_user =
        db::upsert_user_by_auth0_sub(&pool, "dev|local", Some("dev@local"), Some("Local Dev"))
            .await
            .expect("upsert dev user");
    let meeting_id = uuid::Uuid::new_v4().to_string();
    db::insert_meeting(
        &pool,
        &meeting_id,
        &dev_user.id,
        chrono::Utc::now(),
        None,
        "{}",
    )
    .await
    .expect("insert meeting");
    (dev_user.id, meeting_id)
}

/// Seed a meeting owned by a *different* user than dev|local. Used
/// to verify cross-tenant access returns 404 (not 403, not 200).
async fn seed_meeting_for_foreign_user() -> String {
    let pool = db::open_pool().await.expect("open pool");
    // Different auth0_sub → different users.id → different meeting owner.
    let foreign = db::upsert_user_by_auth0_sub(
        &pool,
        &format!("foreign|{}", uuid::Uuid::new_v4()),
        None,
        None,
    )
    .await
    .expect("upsert foreign user");
    let meeting_id = uuid::Uuid::new_v4().to_string();
    db::insert_meeting(
        &pool,
        &meeting_id,
        &foreign.id,
        chrono::Utc::now(),
        None,
        "{}",
    )
    .await
    .expect("insert meeting");
    meeting_id
}

#[tokio::test]
async fn upload_happy_path() {
    let _ = dotenvy::dotenv();
    let server = spawn_test_server().await;
    let (dev_user_id, meeting_id) = seed_dev_user_and_meeting().await;

    let png_bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    let resp = reqwest::Client::new()
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
        .unwrap()
        .expect("row exists");
    assert_eq!(row.meeting_id, meeting_id);
    // user_id is the resolved UUID, not the auth0_sub "dev|local".
    assert_eq!(row.user_id, dev_user_id);
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
    let (_, meeting_id) = seed_dev_user_and_meeting().await;

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
    let (_, meeting_id) = seed_dev_user_and_meeting().await;

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
    // Don't seed any meeting; just hit a random id.
    let resp = reqwest::Client::new()
        .post(format!(
            "http://{}/meetings/{}/chat_attachments",
            server.addr,
            uuid::Uuid::new_v4()
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

    // Meeting owned by a different user, not dev|local.
    let foreign_meeting_id = seed_meeting_for_foreign_user().await;

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
    // 404 (not 403) — don't leak existence. Same convention as moments.
    assert_eq!(resp.status().as_u16(), 404);
}

// ─── WS tests: Intent::Chat resolves attachment_ids → bytes ──────────────
//
// Unlike the HTTP tests above, these drive the full WS handler.
// `start_meeting_via_ws` is required because the WS state machine
// only tracks meetings it created itself via `start_meeting`; a raw
// DB-seeded meeting won't satisfy the Active gate.

/// Drain the initial Snapshot, send start_meeting, drain the 3-event
/// handshake (meeting_state_changed / metadata_changed / mode_changed)
/// and return the server-allocated meeting_id.
async fn start_meeting_via_ws(ws: &mut common::Ws) -> String {
    // Snapshot is sent on connect.
    let _ = common::next_event(ws, Duration::from_secs(2)).await;

    common::send_intent(ws, serde_json::json!({"type":"start_meeting"})).await;
    let e1 = common::next_event(ws, Duration::from_secs(2)).await;
    assert_eq!(e1["type"], "meeting_state_changed");
    assert_eq!(e1["meeting_state"], "active");
    let meeting_id = e1["meeting_id"]
        .as_str()
        .expect("meeting_id present on state change")
        .to_string();
    // Drain the other two handshake events.
    let _ = common::next_event(ws, Duration::from_secs(2)).await; // metadata_changed
    let _ = common::next_event(ws, Duration::from_secs(2)).await; // mode_changed
    meeting_id
}

#[tokio::test]
async fn chat_with_one_attachment_happy() {
    let _ = dotenvy::dotenv();
    let server = spawn_test_server().await;
    let mut ws = common::connect(server.addr, "test-token").await;
    let meeting_id = start_meeting_via_ws(&mut ws).await;

    // Upload via HTTP (the user owns the active meeting → 201).
    let resp = reqwest::Client::new()
        .post(format!(
            "http://{}/meetings/{}/chat_attachments",
            server.addr, meeting_id
        ))
        .header("content-type", "image/png")
        .body(b"\x89PNG\r\n\x1a\nfake".to_vec())
        .send()
        .await
        .expect("upload");
    assert_eq!(resp.status().as_u16(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    let att_id = body["id"].as_str().unwrap().to_string();

    common::send_intent(
        &mut ws,
        serde_json::json!({
            "type": "chat",
            "text": "describe this",
            "attachment_ids": [att_id],
        }),
    )
    .await;

    // No real LLM in the test harness (AURIS_LLM_DISABLED=1),
    // so we never see the agent's reply. What we DO see — if the
    // handler rejects the attachment — is an Event::Error within ~100ms.
    // Grace period of 1s: if nothing arrives, the handler accepted.
    let res = tokio::time::timeout(
        Duration::from_secs(1),
        common::next_event(&mut ws, Duration::from_secs(2)),
    )
    .await;
    if let Ok(e) = res {
        assert_ne!(
            e["type"], "error",
            "unexpected error event from happy-path: {e:#?}"
        );
    }
}

#[tokio::test]
async fn chat_with_unknown_attachment_emits_not_found() {
    let _ = dotenvy::dotenv();
    let server = spawn_test_server().await;
    let mut ws = common::connect(server.addr, "test-token").await;
    let _ = start_meeting_via_ws(&mut ws).await;

    common::send_intent(
        &mut ws,
        serde_json::json!({
            "type": "chat",
            "text": "hi",
            "attachment_ids": ["definitely-does-not-exist"],
        }),
    )
    .await;

    let e = common::next_event(&mut ws, Duration::from_secs(2)).await;
    assert_eq!(e["type"], "error");
    assert_eq!(e["code"], "chat_attachment_not_found");
}

#[tokio::test]
async fn chat_with_cross_tenant_attachment_emits_forbidden() {
    let _ = dotenvy::dotenv();
    let server = spawn_test_server().await;
    let mut ws = common::connect(server.addr, "test-token").await;
    let _active_meeting_id = start_meeting_via_ws(&mut ws).await;

    // Seed a foreign user's meeting + attachment via direct DB+disk
    // insert. The HTTP upload route would 404 for cross-tenant, so
    // we bypass it entirely.
    let pool = db::open_pool().await.expect("open pool");
    let foreign_user = db::upsert_user_by_auth0_sub(
        &pool,
        &format!("foreign|{}", uuid::Uuid::new_v4()),
        None,
        None,
    )
    .await
    .expect("upsert foreign user");
    let foreign_meeting_id = uuid::Uuid::new_v4().to_string();
    db::insert_meeting(
        &pool,
        &foreign_meeting_id,
        &foreign_user.id,
        chrono::Utc::now(),
        None,
        "{}",
    )
    .await
    .expect("insert foreign meeting");
    let att_id = uuid::Uuid::new_v4().to_string();
    let rel = format!("blobs/meetings/{foreign_meeting_id}/chat/{att_id}.png");
    let dir = db::data_dir().expect("data_dir");
    let abs = dir.join(&rel);
    tokio::fs::create_dir_all(abs.parent().unwrap())
        .await
        .expect("mkdir");
    tokio::fs::write(&abs, b"\x89PNG\r\n\x1a\nfake")
        .await
        .expect("write png");
    db::insert_chat_attachment(
        &pool,
        &att_id,
        &foreign_meeting_id,
        &foreign_user.id,
        "image/png",
        &rel,
        11,
    )
    .await
    .expect("insert chat attachment row");

    common::send_intent(
        &mut ws,
        serde_json::json!({
            "type": "chat",
            "text": "describe",
            "attachment_ids": [att_id],
        }),
    )
    .await;

    let e = common::next_event(&mut ws, Duration::from_secs(2)).await;
    assert_eq!(e["type"], "error");
    assert_eq!(e["code"], "chat_attachment_forbidden");
}

#[tokio::test]
async fn chat_with_wrong_meeting_attachment_emits_forbidden() {
    let _ = dotenvy::dotenv();
    let server = spawn_test_server().await;
    let mut ws = common::connect(server.addr, "test-token").await;
    let _active_meeting_id = start_meeting_via_ws(&mut ws).await;

    // Create a DIFFERENT meeting owned by the SAME dev user (raw
    // seed). HTTP upload against IT will succeed — dev|local owns it.
    let (_dev_user_id, other_meeting_id) = seed_dev_user_and_meeting().await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://{}/meetings/{}/chat_attachments",
            server.addr, other_meeting_id
        ))
        .header("content-type", "image/png")
        .body(b"\x89PNG\r\n\x1a\n".to_vec())
        .send()
        .await
        .expect("upload");
    assert_eq!(resp.status().as_u16(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    let att_id = body["id"].as_str().unwrap().to_string();

    // Chat is active on a different meeting_id; the attachment row's
    // meeting_id doesn't match → chat_attachment_forbidden.
    common::send_intent(
        &mut ws,
        serde_json::json!({
            "type": "chat",
            "text": "describe",
            "attachment_ids": [att_id],
        }),
    )
    .await;

    let e = common::next_event(&mut ws, Duration::from_secs(2)).await;
    assert_eq!(e["type"], "error");
    assert_eq!(e["code"], "chat_attachment_forbidden");
}

#[tokio::test]
async fn chat_with_disk_read_failure_emits_unreadable() {
    let _ = dotenvy::dotenv();
    let server = spawn_test_server().await;
    let mut ws = common::connect(server.addr, "test-token").await;
    let meeting_id = start_meeting_via_ws(&mut ws).await;

    // Insert a row whose bytes_path doesn't exist on disk.
    let pool = db::open_pool().await.expect("open pool");
    let dev_user =
        db::upsert_user_by_auth0_sub(&pool, "dev|local", Some("dev@local"), Some("Local Dev"))
            .await
            .expect("upsert dev user");
    let phantom_id = uuid::Uuid::new_v4().to_string();
    db::insert_chat_attachment(
        &pool,
        &phantom_id,
        &meeting_id,
        &dev_user.id,
        "image/png",
        "blobs/meetings/nope/chat/phantom.png",
        42,
    )
    .await
    .expect("insert phantom row");

    common::send_intent(
        &mut ws,
        serde_json::json!({
            "type": "chat",
            "text": "?",
            "attachment_ids": [phantom_id],
        }),
    )
    .await;

    let e = common::next_event(&mut ws, Duration::from_secs(2)).await;
    assert_eq!(e["type"], "error");
    assert_eq!(e["code"], "chat_attachment_unreadable");
}
