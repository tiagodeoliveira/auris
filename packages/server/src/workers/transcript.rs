//! Transcript summarizer — pass-through, no LLM.
//! Each TranscriptChunk arriving on the broadcast channel becomes a single
//! Item appended to the transcript-mode buffer; the resulting payload is
//! broadcast to all WS clients via Event::ItemsUpdate.
//!
//! Per-meeting: each user's `spawn_live_pipeline` spawns its own
//! transcript summarizer keyed to that user AND that meeting. The
//! chunks it receives already carry `user_id`; we route through
//! `state.append_transcript_chunk_if_active(uid, meeting_id, ...)`,
//! which mutates *only* that user's `UserSession` and *only* while
//! `meeting_id` is still active. Cross-user contamination is
//! structurally prevented; cross-*meeting* contamination (a stopped
//! meeting's STT drain tail bleeding into the next meeting) is too —
//! see `append_transcript_chunk_if_active`.

use crate::protocol::Event;
use crate::session::SessionRegistry;
use crate::stt::TranscriptChunk;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::warn;

pub async fn run_transcript_summarizer(
    state: Arc<Mutex<SessionRegistry>>,
    mut rx: broadcast::Receiver<TranscriptChunk>,
    bus: crate::context::EventBus,
    user_id: String,
    meeting_id: String,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            chunk = rx.recv() => match chunk {
                Ok(c) => {
                    // Belt-and-suspenders: each summarizer instance is
                    // bound to one user; chunks should always carry
                    // that same user_id. Drop any that don't (defends
                    // against future plumbing mistakes).
                    if c.user_id != user_id {
                        warn!(expected = %user_id, got = %c.user_id, "transcript chunk user_id mismatch — dropping");
                        continue;
                    }
                    let item = c.to_item();
                    let payload = {
                        let mut s = state.lock().await;
                        s.append_transcript_chunk_if_active(&user_id, &meeting_id, c, item)
                    };
                    // `None` means this meeting is no longer active (it
                    // stopped, or a new one started) — the chunk is a
                    // post-stop drain-tail straggler. Drop it from the
                    // live + persistence path so it can't bleed into the
                    // next meeting. The tail is NOT lost: finalize's own
                    // chunk subscription collects it and broadcasts a
                    // server-internal `Event::TranscriptTail`, which the
                    // persistence loop appends to the stopped meeting's
                    // JSONL and the mnemo pusher pushes to its session.
                    if let Some(payload) = payload {
                        if !payload.is_empty() {
                            // Durable: this await is the backpressure
                            // point — if the writer stalls, transcript
                            // emission slows instead of losing lines.
                            // Stamp the meeting_id so a line straddling a
                            // stop/start boundary can't be persisted into
                            // the NEXT meeting's JSONL by a registry
                            // lookup at consume time.
                            bus.emit_for_meeting(
                                user_id.clone(),
                                meeting_id.clone(),
                                Event::ItemsUpdate {
                                    mode: "transcript".into(),
                                    items: payload,
                                },
                            )
                            .await;
                        }
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
    use crate::context::EventBus;
    use crate::protocol::{Intent, UserEvent};
    use tokio::sync::mpsc;

    /// (bus, fanout receiver, durable receiver) — receivers created
    /// BEFORE any emit so nothing is missed.
    fn test_bus() -> (
        EventBus,
        broadcast::Receiver<UserEvent>,
        mpsc::Receiver<UserEvent>,
    ) {
        let (fanout, fanout_rx) = broadcast::channel::<UserEvent>(16);
        let (durable_tx, durable_rx) = mpsc::channel::<UserEvent>(16);
        (EventBus::new(fanout, durable_tx), fanout_rx, durable_rx)
    }

    /// THE regression for improvement #18: committed transcript lines
    /// are the system of record and must reach the durable queue —
    /// not only the lossy client broadcast.
    #[tokio::test]
    async fn summarizer_routes_items_update_to_durable_queue() {
        let state = Arc::new(Mutex::new(SessionRegistry::new()));
        let uid = "test-user".to_string();
        let meeting_id = {
            let mut s = state.lock().await;
            s.apply_intent(
                &uid,
                Intent::StartMeeting {
                    description: None,
                    metadata: None,
                    audio_source_device_id: None,
                    assist_sensitivity: None,
                },
            );
            s.active_meeting_id_for(&uid).expect("meeting active")
        };
        let (chunk_tx, chunk_rx) = broadcast::channel::<TranscriptChunk>(16);
        let (bus, _fanout_rx, mut durable_rx) = test_bus();
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task_state = Arc::clone(&state);
        let task_uid = uid.clone();
        let task_meeting_id = meeting_id.clone();
        let handle = tokio::spawn(async move {
            run_transcript_summarizer(
                task_state,
                chunk_rx,
                bus,
                task_uid,
                task_meeting_id,
                task_cancel,
            )
            .await;
        });

        chunk_tx
            .send(TranscriptChunk {
                id: "c1".into(),
                text: "durable sentence.".into(),
                t_start_ms: 100,
                t_end_ms: 500,
                speaker: None,
                user_id: uid.clone(),
            })
            .unwrap();

        let envelope =
            tokio::time::timeout(std::time::Duration::from_millis(500), durable_rx.recv())
                .await
                .expect("durable queue received within timeout")
                .expect("queue open");
        assert_eq!(envelope.user_id, uid);
        assert_eq!(
            envelope.meeting_id.as_deref(),
            Some(meeting_id.as_str()),
            "summarizer must stamp its meeting_id on the envelope"
        );
        match envelope.event {
            Event::ItemsUpdate { mode, items } => {
                assert_eq!(mode, "transcript");
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].text, "durable sentence.");
            }
            other => panic!("expected ItemsUpdate on durable queue, got {other:?}"),
        }

        cancel.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn transcript_summarizer_emits_item_per_chunk() {
        let state = Arc::new(Mutex::new(SessionRegistry::new()));
        let uid = "test-user".to_string();
        let meeting_id = {
            let mut s = state.lock().await;
            s.apply_intent(
                &uid,
                Intent::StartMeeting {
                    description: None,
                    metadata: None,
                    audio_source_device_id: None,
                    assist_sensitivity: None,
                },
            );
            s.apply_intent(
                &uid,
                Intent::SetMode {
                    mode: "transcript".into(),
                },
            );
            s.active_meeting_id_for(&uid).expect("meeting active")
        };
        let (chunk_tx, chunk_rx) = broadcast::channel::<TranscriptChunk>(16);
        let (event_tx, mut event_rx) = broadcast::channel::<UserEvent>(16);
        let (durable_tx, _durable_rx) = mpsc::channel::<UserEvent>(16);
        let bus = EventBus::new(event_tx, durable_tx);
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task_state = Arc::clone(&state);
        let task_uid = uid.clone();
        let task_meeting_id = meeting_id.clone();
        let handle = tokio::spawn(async move {
            run_transcript_summarizer(
                task_state,
                chunk_rx,
                bus,
                task_uid,
                task_meeting_id,
                task_cancel,
            )
            .await;
        });

        chunk_tx
            .send(TranscriptChunk {
                id: "c1".into(),
                text: "first utterance".into(),
                t_start_ms: 100,
                t_end_ms: 500,
                speaker: None,
                user_id: uid.clone(),
            })
            .unwrap();

        let envelope = tokio::time::timeout(std::time::Duration::from_millis(500), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(envelope.user_id, uid);
        match envelope.event {
            Event::ItemsUpdate { mode, items } => {
                assert_eq!(mode, "transcript");
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].text, "first utterance");
                assert_eq!(items[0].id, "c1");
                assert_eq!(items[0].t, 100);
            }
            _ => panic!("expected ItemsUpdate"),
        }

        // Verify state was mutated for that user.
        {
            let s = state.lock().await;
            assert_eq!(
                s.rolling_transcript_text_for(&uid).as_deref(),
                Some("first utterance")
            );
        }

        cancel.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn transcript_summarizer_propagates_speaker_to_meta() {
        let state = Arc::new(Mutex::new(SessionRegistry::new()));
        let uid = "test-user".to_string();
        let meeting_id = {
            let mut s = state.lock().await;
            s.apply_intent(
                &uid,
                Intent::StartMeeting {
                    description: None,
                    metadata: None,
                    audio_source_device_id: None,
                    assist_sensitivity: None,
                },
            );
            s.apply_intent(
                &uid,
                Intent::SetMode {
                    mode: "transcript".into(),
                },
            );
            s.active_meeting_id_for(&uid).expect("meeting active")
        };
        let (chunk_tx, chunk_rx) = broadcast::channel::<TranscriptChunk>(16);
        let (event_tx, mut event_rx) = broadcast::channel::<UserEvent>(16);
        let (durable_tx, _durable_rx) = mpsc::channel::<UserEvent>(16);
        let bus = EventBus::new(event_tx, durable_tx);
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task_state = Arc::clone(&state);
        let task_uid = uid.clone();
        let task_meeting_id = meeting_id.clone();
        let handle = tokio::spawn(async move {
            run_transcript_summarizer(
                task_state,
                chunk_rx,
                bus,
                task_uid,
                task_meeting_id,
                task_cancel,
            )
            .await;
        });

        chunk_tx
            .send(TranscriptChunk {
                id: "c1".into(),
                text: "first utterance".into(),
                t_start_ms: 100,
                t_end_ms: 500,
                speaker: Some("1".into()),
                user_id: uid.clone(),
            })
            .unwrap();

        let envelope = tokio::time::timeout(std::time::Duration::from_millis(500), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        match envelope.event {
            Event::ItemsUpdate { items, .. } => {
                assert_eq!(items.len(), 1);
                let meta = items[0].meta.as_ref().expect("meta should be populated");
                assert_eq!(meta["speaker"], serde_json::json!("1"));
            }
            _ => panic!("expected ItemsUpdate"),
        }

        cancel.cancel();
        handle.await.unwrap();
    }

    /// Regression: a stopped meeting's STT drain tail must not bleed
    /// into the next meeting. The meeting-1 summarizer outlives stop
    /// (finalize keeps it alive to flush the drain); a late chunk it
    /// processes after meeting 2 has started must be dropped — no
    /// broadcast, and meeting 2's rolling transcript left untouched.
    #[tokio::test]
    async fn drain_tail_does_not_bleed_into_next_meeting() {
        let state = Arc::new(Mutex::new(SessionRegistry::new()));
        let uid = "test-user".to_string();

        // Meeting 1 starts; capture its id and key the summarizer to it.
        let meeting_1 = {
            let mut s = state.lock().await;
            s.apply_intent(
                &uid,
                Intent::StartMeeting {
                    description: None,
                    metadata: None,
                    audio_source_device_id: None,
                    assist_sensitivity: None,
                },
            );
            s.active_meeting_id_for(&uid).expect("meeting 1 active")
        };

        let (chunk_tx, chunk_rx) = broadcast::channel::<TranscriptChunk>(16);
        let (event_tx, mut event_rx) = broadcast::channel::<UserEvent>(16);
        let (durable_tx, _durable_rx) = mpsc::channel::<UserEvent>(16);
        let bus = EventBus::new(event_tx, durable_tx);
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task_state = Arc::clone(&state);
        let task_uid = uid.clone();
        let task_meeting_id = meeting_1.clone();
        let handle = tokio::spawn(async move {
            run_transcript_summarizer(
                task_state,
                chunk_rx,
                bus,
                task_uid,
                task_meeting_id,
                task_cancel,
            )
            .await;
        });

        // Meeting 1 stops, meeting 2 starts — the world the drain tail
        // arrives into.
        {
            let mut s = state.lock().await;
            s.apply_intent(&uid, Intent::StopMeeting);
            s.apply_intent(
                &uid,
                Intent::StartMeeting {
                    description: None,
                    metadata: None,
                    audio_source_device_id: None,
                    assist_sensitivity: None,
                },
            );
        }

        // A straggler chunk on meeting-1's channel (Soniox flushing its
        // last buffered utterance during finalize drain).
        chunk_tx
            .send(TranscriptChunk {
                id: "stale".into(),
                text: "tail of meeting one".into(),
                t_start_ms: 100,
                t_end_ms: 500,
                speaker: None,
                user_id: uid.clone(),
            })
            .unwrap();

        // No broadcast should fire for the stale chunk.
        let got =
            tokio::time::timeout(std::time::Duration::from_millis(200), event_rx.recv()).await;
        assert!(
            got.is_err(),
            "stale meeting-1 chunk must not broadcast into meeting 2"
        );

        // Meeting 2's rolling transcript stays empty — no in-memory bleed.
        {
            let s = state.lock().await;
            assert_eq!(
                s.rolling_transcript_text_for(&uid).as_deref(),
                Some(""),
                "meeting 2's transcript must not contain meeting 1's tail"
            );
        }

        cancel.cancel();
        handle.await.unwrap();
    }
}
