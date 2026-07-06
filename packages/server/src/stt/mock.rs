//! Mock STT backend — emits canned transcript chunks at a fixed cadence.
//! Enabled via `AURIS_STT_PROVIDER=mock`.

use crate::stt::TranscriptChunk;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

const CANNED: &[&str] = &[
    "Let's review the Q1 budget for the helix product launch.",
    "Engineering needs about three more weeks for the API.",
    "Design has the mockups ready for the team review on Friday.",
    "Finance flagged a fifteen percent overrun on infrastructure.",
    "We should sync with the mobile team before locking the spec.",
    "Action: Tiago to write up the migration plan by next Tuesday.",
    "Action: schedule a follow-up with legal for the compliance question.",
    "The launch date is still tentative; depends on the security review.",
];

pub struct MockStt {
    interval: Duration,
}

impl MockStt {
    pub fn from_env() -> Self {
        let interval_ms: u64 = std::env::var("AURIS_STT_MOCK_INTERVAL_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3000);
        Self {
            interval: Duration::from_millis(interval_ms),
        }
    }
}

impl crate::stt::SttProvider for MockStt {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn run(
        self: Box<Self>,
        _audio_rx: Option<tokio::sync::mpsc::Receiver<Vec<u8>>>,
        transcript_tx: broadcast::Sender<TranscriptChunk>,
        _events_tx: broadcast::Sender<crate::protocol::UserEvent>,
        user_id: String,
        cancel: CancellationToken,
        drain: CancellationToken,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(async move {
            run_mock_stt_inner(transcript_tx, user_id, cancel, drain, self.interval).await;
        })
    }
}

/// Private run loop — sends one canned `TranscriptChunk` per `interval`
/// tick to `tx`. Cycles through the canned utterance list. Stops cleanly when
/// `cancel` or `drain` fires (mock treats both as a hard stop).
async fn run_mock_stt_inner(
    tx: broadcast::Sender<TranscriptChunk>,
    user_id: String,
    cancel: CancellationToken,
    drain: CancellationToken,
    interval: Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await; // discard immediate tick
    let mut idx: usize = 0;
    let started = tokio::time::Instant::now();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = drain.cancelled() => break,
            _ = ticker.tick() => {
                let text = CANNED[idx % CANNED.len()].to_string();
                let elapsed_ms = started.elapsed().as_millis() as u64;
                let chunk = TranscriptChunk {
                    id: uuid::Uuid::new_v4().to_string(),
                    text,
                    t_start_ms: elapsed_ms.saturating_sub(2000),
                    t_end_ms: elapsed_ms,
                    speaker: None,
                    user_id: user_id.clone(),
                };
                let _ = tx.send(chunk);
                idx += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn mock_emits_chunks_on_cadence() {
        let (tx, mut rx) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task_tx = tx.clone();
        let task_drain = CancellationToken::new();
        let handle = tokio::spawn(async move {
            run_mock_stt_inner(
                task_tx,
                "test-user".into(),
                task_cancel,
                task_drain,
                Duration::from_millis(100),
            )
            .await;
        });

        // Advance virtual time by ~350 ms — should yield 3 chunks (at +100, +200, +300).
        tokio::time::sleep(Duration::from_millis(350)).await;
        cancel.cancel();
        handle.await.unwrap();

        let mut received = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            received.push(chunk);
        }
        assert!(
            received.len() >= 3,
            "expected ≥3 chunks, got {}",
            received.len()
        );
        assert!(!received[0].text.is_empty());
        assert!(received[0].t_end_ms > 0);
    }
}
