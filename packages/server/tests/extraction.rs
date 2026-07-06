mod common;

use common::*;
use serde_json::json;
use std::time::Duration;

const T: Duration = Duration::from_secs(3);

async fn drain_snapshot(ws: &mut Ws) {
    let _ = next_event(ws, T).await;
}

#[tokio::test]
async fn extraction_no_description() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    for _ in 0..3 {
        let _ = next_event(&mut ws, T).await;
    }
    // After 2.5s, no extraction event should arrive (description was empty,
    // and the LLM is disabled in tests anyway via AURIS_LLM_DISABLED).
    let res = tokio::time::timeout(Duration::from_millis(2500), async {
        loop {
            let evt = next_event(&mut ws, Duration::from_secs(10)).await;
            if evt["type"] == "metadata_changed" {
                return evt;
            }
        }
    })
    .await;
    assert!(
        res.is_err(),
        "extraction should not run without description"
    );
}
