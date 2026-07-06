mod common;

use common::*;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Error;

#[tokio::test]
async fn handshake_token_match() {
    let server = spawn_test_server().await;
    let (mut ws, _) = connect_async(ws_url(server.addr, "test-token"))
        .await
        .expect("connect");
    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        futures_util::StreamExt::next(&mut ws),
    )
    .await
    .expect("timeout")
    .expect("frame")
    .expect("msg");
    assert!(msg.is_text(), "expected text frame, got {:?}", msg);
    let json: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(json["type"], "snapshot");
}

/// In Live mode an unparseable token must be rejected with a plain
/// HTTP 401 *before* the WS upgrade — no silent close, no snapshot.
#[tokio::test]
async fn handshake_live_rejects_garbage_token() {
    let live = spawn_test_server_live().await;
    match connect_async(ws_url(live.server.addr, "not-a-jwt")).await {
        Ok(_) => panic!("garbage token must be rejected before upgrade"),
        Err(Error::Http(resp)) => {
            assert_eq!(resp.status().as_u16(), 401, "expected HTTP 401")
        }
        Err(e) => panic!("expected HTTP 401 rejection, got: {e}"),
    }
}

#[tokio::test]
async fn handshake_live_rejects_missing_token() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let live = spawn_test_server_live().await;
    let req = format!("ws://{}/", live.server.addr)
        .into_client_request()
        .unwrap();
    match connect_async(req).await {
        Ok(_) => panic!("missing token must be rejected before upgrade"),
        Err(Error::Http(resp)) => {
            assert_eq!(resp.status().as_u16(), 401, "expected HTTP 401")
        }
        Err(e) => panic!("expected HTTP 401 rejection, got: {e}"),
    }
}

/// Full pairing flow against the shared dev DB, then a Live-mode WS
/// connect with the minted device token: must upgrade and deliver the
/// initial snapshot.
#[tokio::test]
async fn handshake_live_accepts_paired_device_token() {
    let live = spawn_test_server_live().await;
    let db = auris_server::storage::open_pool().await.expect("pool");
    let sub = format!("test|{}", uuid::Uuid::new_v4());
    let user = auris_server::storage::users::upsert_user_by_auth0_sub(&db, &sub, None, None)
        .await
        .expect("user");
    let code = auris_server::auth::pairing::mint_code(&db, &user.id)
        .await
        .expect("code");
    let pair = auris_server::auth::pairing::redeem_code(
        &db,
        &live.issuer,
        &code.code,
        Some("handshake-test".into()),
    )
    .await
    .expect("redeem");

    let mut ws = connect(live.server.addr, &pair.access_token).await;
    let snapshot = next_event(&mut ws, std::time::Duration::from_secs(2)).await;
    assert_eq!(snapshot["type"], "snapshot");
}

/// Same provisioning, then revoke the device: the still-valid JWT must
/// be rejected with HTTP 401 at the next connect.
#[tokio::test]
async fn handshake_live_rejects_revoked_device_token() {
    let live = spawn_test_server_live().await;
    let db = auris_server::storage::open_pool().await.expect("pool");
    let sub = format!("test|{}", uuid::Uuid::new_v4());
    let user = auris_server::storage::users::upsert_user_by_auth0_sub(&db, &sub, None, None)
        .await
        .expect("user");
    let code = auris_server::auth::pairing::mint_code(&db, &user.id)
        .await
        .expect("code");
    let pair = auris_server::auth::pairing::redeem_code(
        &db,
        &live.issuer,
        &code.code,
        Some("handshake-test-revoked".into()),
    )
    .await
    .expect("redeem");
    let revoked = auris_server::auth::pairing::revoke_device(&db, &user.id, &pair.device_id)
        .await
        .expect("revoke");
    assert_eq!(revoked, 1);

    match connect_async(ws_url(live.server.addr, &pair.access_token)).await {
        Ok(_) => panic!("revoked device token must be rejected before upgrade"),
        Err(Error::Http(resp)) => {
            assert_eq!(resp.status().as_u16(), 401, "expected HTTP 401")
        }
        Err(e) => panic!("expected HTTP 401 rejection, got: {e}"),
    }
}
