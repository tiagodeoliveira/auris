//! HTTP integration tests for the chat-attachments upload route.
//!
//! The auth-disabled test harness pins all requests to the dev user
//! resolved via auth0_sub "dev|local". The unauthenticated-path test
//! from the spec is NOT covered here — the harness can't represent it.

mod common;
use common::spawn_test_server;

use meeting_companion_server::db;

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
