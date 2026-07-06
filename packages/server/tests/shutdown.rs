mod common;

use common::*;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn graceful_shutdown_sends_close() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    let _ = next_event(&mut ws, Duration::from_secs(1)).await;

    drop(server); // triggers shutdown_tx via TestServer::Drop

    use futures_util::StreamExt;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut got_close = false;
    while std::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), ws.next()).await {
            Ok(Some(Ok(Message::Close(_)))) => {
                got_close = true;
                break;
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(_))) | Ok(None) => break,
            Err(_) => continue,
        }
    }
    assert!(got_close, "expected close frame on graceful shutdown");
}

#[tokio::test]
async fn shutdown_waits_for_tracked_finalize_task() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let mut server = spawn_test_server().await;
    let join = server
        .join
        .take()
        .expect("harness exposes the server task join handle");

    // Stand-in for a detached finalize: it outlives the fixed 2 s
    // connection drain (real finalizes run a >=6 s STT drain plus
    // three LLM calls). Before the TaskTracker fix, run_server
    // returned at ~2 s and this task was killed with the runtime —
    // which in production meant the meeting summary was lost.
    let finished = Arc::new(AtomicBool::new(false));
    let flag = finished.clone();
    server.handle.tasks.spawn(async move {
        tokio::time::sleep(Duration::from_millis(3500)).await;
        flag.store(true, Ordering::SeqCst);
    });

    drop(server); // fires shutdown via TestServer::Drop
    join.await.expect("server task panicked");
    assert!(
        finished.load(Ordering::SeqCst),
        "run_server_with_listener returned before the tracked finalize \
         task completed — summaries would be lost on deploy"
    );
}

#[tokio::test]
async fn shutdown_with_no_tracked_tasks_stays_prompt() {
    let mut server = spawn_test_server().await;
    let join = server
        .join
        .take()
        .expect("harness exposes the server task join handle");

    let start = std::time::Instant::now();
    drop(server);
    join.await.expect("server task panicked");
    // Empty tracker → wait() returns immediately; total shutdown stays
    // at the ~2 s connection drain. 5 s bound leaves CI slack.
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "empty tracker must not add shutdown latency (got {:?})",
        start.elapsed()
    );
}
