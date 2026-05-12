//! DB-layer tests for chat_attachments table.
//!
//! These tests share one Postgres DB (the dev instance via `.env`'s
//! `DATABASE_URL`), so each test mints fresh UUIDs for users / meetings /
//! attachments to avoid PK collisions. Matches the integration-test
//! pattern in `tests/common/mod.rs`: `#[tokio::test]` + `dotenvy::dotenv()`
//! + `db::open_pool()`, not `sqlx::test`.

use meeting_companion_server::db;

/// Seed a `users` row + a `meetings` row owned by that user and return
/// `(user_id, meeting_id)`. The meetings FK to users(id) requires the
/// user to exist first, so we go through `upsert_user_by_auth0_sub` to
/// mint the internal user id rather than fabricating one.
async fn seed_meeting() -> (String, String) {
    let pool = db::open_pool().await.expect("open pool");
    let auth0_sub = format!("test|{}", uuid::Uuid::new_v4());
    let user = db::upsert_user_by_auth0_sub(&pool, &auth0_sub, None, None)
        .await
        .expect("upsert user");
    let meeting_id = uuid::Uuid::new_v4().to_string();
    db::insert_meeting(&pool, &meeting_id, &user.id, chrono::Utc::now(), None, "{}")
        .await
        .expect("insert meeting");
    (user.id, meeting_id)
}

#[tokio::test]
async fn insert_and_get_round_trip() {
    let _ = dotenvy::dotenv();
    let pool = db::open_pool().await.expect("open pool");
    let (user_id, meeting_id) = seed_meeting().await;
    let att_id = uuid::Uuid::new_v4().to_string();

    db::insert_chat_attachment(
        &pool,
        &att_id,
        &meeting_id,
        &user_id,
        "image/png",
        "blobs/meetings/m/chat/att.png",
        12345,
    )
    .await
    .expect("insert ok");

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
    let (user_id, meeting_id) = seed_meeting().await;
    let att_id = uuid::Uuid::new_v4().to_string();

    db::insert_chat_attachment(
        &pool,
        &att_id,
        &meeting_id,
        &user_id,
        "image/png",
        "blobs/meetings/m/chat/att.png",
        1,
    )
    .await
    .unwrap();

    let deleted = db::delete_meeting_for_user(&pool, &meeting_id, &user_id)
        .await
        .expect("delete ok");
    assert!(deleted);

    let got = db::get_chat_attachment(&pool, &att_id)
        .await
        .expect("get ok");
    assert!(
        got.is_none(),
        "cascade should have removed the attachment row"
    );
}
