//! Transcript summarizer — pass-through, no LLM.
//! Each TranscriptChunk arriving on the broadcast channel becomes a single
//! Item appended to the transcript-mode buffer; the resulting payload is
//! broadcast to all WS clients via Event::ItemsUpdate.

use crate::contract::{Event, Item};
use crate::state::ServerState;
use crate::stt::TranscriptChunk;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::warn;

pub async fn run_transcript_summarizer(
    state: Arc<Mutex<ServerState>>,
    mut rx: broadcast::Receiver<TranscriptChunk>,
    events_tx: broadcast::Sender<Event>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            chunk = rx.recv() => match chunk {
                Ok(c) => {
                    let item = Item {
                        id: c.id.clone(),
                        text: c.text.clone(),
                        detail: None,
                        t: c.t_start_ms,
                        meta: c.speaker.as_ref().map(|s| serde_json::json!({ "speaker": s })),
                    };
                    let payload = {
                        let mut s = state.lock().await;
                        s.append_transcript_chunk(c);
                        s.push_item_for_mode("transcript", item)
                    };
                    if !payload.is_empty() {
                        let _ = events_tx.send(Event::ItemsUpdate {
                            mode: "transcript".into(),
                            items: payload,
                        });
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(lagged = n, "transcript summarizer lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::Intent;

    #[tokio::test]
    async fn transcript_summarizer_emits_item_per_chunk() {
        let state = Arc::new(Mutex::new(ServerState::new()));
        {
            let mut s = state.lock().await;
            s.apply_intent(Intent::StartMeeting {
                description: None,
                metadata: None,
                audio_source_device_id: None,
            });
            // Switch to transcript mode (default is highlights).
            s.apply_intent(Intent::SetMode {
                mode: "transcript".into(),
            });
        }
        let (chunk_tx, chunk_rx) = broadcast::channel::<TranscriptChunk>(16);
        let (event_tx, mut event_rx) = broadcast::channel::<Event>(16);
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task_state = Arc::clone(&state);
        let handle = tokio::spawn(async move {
            run_transcript_summarizer(task_state, chunk_rx, event_tx, task_cancel).await;
        });

        chunk_tx
            .send(TranscriptChunk {
                id: "c1".into(),
                text: "first utterance".into(),
                t_start_ms: 100,
                t_end_ms: 500,
                speaker: None,
            })
            .unwrap();

        let evt = tokio::time::timeout(std::time::Duration::from_millis(500), event_rx.recv())
            .await
            .unwrap()
            .unwrap();

        match evt {
            Event::ItemsUpdate { mode, items } => {
                assert_eq!(mode, "transcript");
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].text, "first utterance");
                assert_eq!(items[0].id, "c1");
                assert_eq!(items[0].t, 100);
            }
            _ => panic!("expected ItemsUpdate"),
        }

        // Verify state was mutated too.
        {
            let s = state.lock().await;
            assert_eq!(s.rolling_transcript_text(), "first utterance");
        }

        cancel.cancel();
        handle.await.unwrap();
    }
}
