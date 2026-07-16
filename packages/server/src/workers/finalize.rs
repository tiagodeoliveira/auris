//! Detached meeting-finalize orchestrator.
//!
//! When a meeting stops it flips to Idle instantly (clients close at
//! once). This task then owns the `MeetingRuntime` and drains the STT
//! pipeline gracefully so the last spoken sentences aren't lost:
//!
//!   1. Subscribe to the transcript-chunk stream (before draining).
//!   2. Fire the STT drain signal (provider sends its end-of-stream, reads
//!      trailing finals — see `stt::soniox`); cancel the reactive chat +
//!      active agents so they don't burn tokens firing on the drained tail.
//!   3. Await the STT task, bounded by `AURIS_FINALIZE_DRAIN_MS` (~6s).
//!   4. Collect the chunks the drain produced; append to the pre-stop
//!      transcript snapshot to form the COMPLETE transcript.
//!   5. Parallel: `summarize::run` (summary + highlights) ∥ `wrap_up::extract`
//!      (actions + open_questions), both on the COMPLETE transcript.
//!   6. Drain LLM usage for both pools (captures the wrap-up + summarize
//!      tokens, which previously drained at stop and were dropped).
//!   7. `runtime.shutdown()` — awaits the transcript summarizer (a
//!      happens-before barrier) then cancels + awaits the remaining tasks.
//!   8. Broadcast `Event::MeetingFinalized` so the mnemo pusher resets its
//!      session — only AFTER the drained tail's events are all enqueued.
//!
//! The drained tail does NOT reach persistence via the summarizer — its
//! active-meeting guard (`append_transcript_chunk_if_active`) drops
//! post-stop chunks by design (anti-bleed). Instead, step 4 broadcasts a
//! server-internal `Event::TranscriptTail` with the tail as full `Item`s:
//! the persistence loop appends them to the meeting's transcription.jsonl
//! (addressed by meeting_id) and the mnemo pusher pushes them to the
//! still-open session — so the past-meeting view AND mnemo recall both
//! include the final sentences.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast::error::TryRecvError;
use tracing::{info, warn};

use crate::llm::LlmClient;
use crate::protocol::{Event, Item, UserEvent};
use crate::session::MeetingRuntime;
use crate::stt::TranscriptChunk;

/// Bounded wait for the STT graceful drain before we proceed regardless.
/// Slightly larger than the soniox-internal `AURIS_SONIOX_DRAIN_MS` (5s)
/// so the provider's own clean finish normally wins and this is a true
/// backstop. Override via `AURIS_FINALIZE_DRAIN_MS` (default 6000).
fn finalize_drain_ms() -> u64 {
    crate::config::var_u64_or("AURIS_FINALIZE_DRAIN_MS", 6000)
}

/// Append drained tail chunks to the pre-stop snapshot, newline-joined.
fn assemble_transcript(pre_stop: String, tail: &[String]) -> String {
    let mut complete = pre_stop;
    for t in tail {
        if !complete.is_empty() {
            complete.push('\n');
        }
        complete.push_str(t);
    }
    complete
}

/// Build the server-internal `TranscriptTail` event from the drained
/// chunks, or `None` when nothing was drained. Items are built with
/// `TranscriptChunk::to_item`, so the JSONL lines the persistence loop
/// appends for the tail are shaped exactly like the live ones.
fn tail_event(user_id: &str, meeting_id: &str, tail: &[TranscriptChunk]) -> Option<UserEvent> {
    if tail.is_empty() {
        return None;
    }
    let items: Vec<Item> = tail.iter().map(TranscriptChunk::to_item).collect();
    Some(UserEvent::new(
        user_id.to_string(),
        Event::TranscriptTail {
            meeting_id: meeting_id.to_string(),
            items,
        },
    ))
}

/// Run the detached finalize for a stopped meeting. Consumes the runtime.
pub async fn run(
    mut runtime: MeetingRuntime,
    db: sqlx::PgPool,
    chat_llm: Arc<LlmClient>,
    background_llm: Arc<LlmClient>,
    bus: crate::context::EventBus,
    user_id: String,
    pre_stop_transcript: String,
) {
    let meeting_id = runtime.meeting_id.clone();
    // Subscribe BEFORE triggering the drain so no drained chunk is missed.
    let mut chunk_rx = runtime.subscribe_chunks();

    info!(
        user_id = %user_id,
        meeting_id = %meeting_id,
        pre_stop_chars = pre_stop_transcript.len(),
        "finalize starting; draining STT",
    );

    // Mark "running" up front so `GET /meetings/:id` shows a "wrapping
    // up…" hint for the whole drain window, not just once the wrap-up
    // LLM call begins (which is up to AURIS_FINALIZE_DRAIN_MS later).
    // Crash-recovery notes: if the process dies AFTER this write, the
    // boot sweep (`ws::control::sweep_orphaned_wrap_ups`) flips the row
    // to 'failed' on the next start. If it dies BEFORE this write (but
    // after `end_meeting`), the status stays NULL — the meeting renders
    // like a legacy/pre-extractor one with no banner; that residual gap
    // is non-wedging because regenerate works on any ended meeting.
    if let Err(e) = crate::storage::meetings::set_wrap_up_status(&db, &meeting_id, "running").await
    {
        warn!(meeting_id = %meeting_id, error = ?e, "finalize: failed to mark wrap_up running");
    }

    runtime.trigger_drain();

    // Stop the reactive chat + active agents now — they'd otherwise fire
    // on the drained tail and burn tokens on writes that no-op once Idle.
    // STT + transcript-summarizer stay alive (children of `cancel`) to
    // flush the drain.
    runtime.cancel_reactive_agents();

    // Await the STT drain, bounded. On timeout, hard-cancel so the
    // provider's cancel branch flushes + exits; we proceed regardless.
    let drain_timeout = Duration::from_millis(finalize_drain_ms());
    if let Some(stt) = runtime.take_stt_task() {
        tokio::select! {
            joined = stt => {
                if let Err(e) = joined {
                    warn!(meeting_id = %meeting_id, error = %e, "finalize: STT task join error");
                }
            }
            _ = tokio::time::sleep(drain_timeout) => {
                warn!(
                    meeting_id = %meeting_id,
                    timeout_ms = %drain_timeout.as_millis(),
                    "finalize: STT drain timed out; proceeding with partial transcript",
                );
                runtime.cancel.cancel();
            }
        }
    }

    // Collect the chunks the drain produced (buffered in the broadcast).
    // The broadcast is per-user-pipeline but stamps user_id on each chunk;
    // filter defensively in case the channel ever carries another user's.
    let mut tail: Vec<TranscriptChunk> = Vec::new();
    loop {
        match chunk_rx.try_recv() {
            Ok(chunk) => {
                if chunk.user_id == user_id {
                    tail.push(chunk);
                }
            }
            Err(TryRecvError::Empty) | Err(TryRecvError::Closed) => break,
            Err(TryRecvError::Lagged(n)) => {
                warn!(
                    meeting_id = %meeting_id,
                    skipped = n,
                    "finalize: chunk receiver lagged; summarize + wrap-up will run on a PARTIAL transcript"
                );
            }
        }
    }

    // Durability FIRST, before the (slow) LLM pass: broadcast the
    // drained tail as a server-internal `TranscriptTail`. The
    // persistence loop appends it to this meeting's transcription.jsonl
    // (addressed by meeting_id — no active-session lookup, so it can't
    // bleed into a next meeting or get dropped now that we're Idle) and
    // the mnemo pusher pushes it to the still-open session. The durable
    // queue (FIFO) preserves order vs the summarizer's pre-stop
    // ItemsUpdates; sent before `MeetingFinalized` ⇒ the pusher sees
    // the tail before its session reset.
    if let Some(envelope) = tail_event(&user_id, &meeting_id, &tail) {
        bus.emit(envelope.user_id, envelope.event).await;
    }

    // Assemble the COMPLETE transcript: pre-stop snapshot + drained tail.
    let tail_texts: Vec<String> = tail.iter().map(|c| c.text.clone()).collect();
    let complete = assemble_transcript(pre_stop_transcript, &tail_texts);

    info!(
        user_id = %user_id,
        meeting_id = %meeting_id,
        complete_chars = complete.len(),
        "finalize: STT drained; running wrap-up on complete transcript",
    );

    // Offline pass on the COMPLETE transcript: summary+highlights,
    // wrap-up (actions+open_questions), and backfill of a missing
    // title/description — all in parallel. Independent, all on the
    // background pool, all write straight to the DB.
    if !complete.trim().is_empty() {
        let chat_text = crate::workers::chat_context::load_chat_context(&db, &meeting_id).await;
        tokio::join!(
            crate::workers::summarize::run(
                &user_id,
                &meeting_id,
                &complete,
                &chat_text,
                &background_llm,
                &db
            ),
            crate::workers::wrap_up::extract(
                &user_id,
                &meeting_id,
                &complete,
                &background_llm,
                &db
            ),
            crate::workers::backfill::run(&user_id, &meeting_id, &complete, &background_llm, &db),
        );
    } else if let Err(e) =
        crate::storage::meetings::set_wrap_up_status(&db, &meeting_id, "success").await
    {
        warn!(meeting_id = %meeting_id, error = ?e, "finalize: failed to mark empty wrap_up success");
    }

    // Drain per-pool LLM usage NOW — after summarize + wrap-up — so their
    // tokens land in the per-meeting usage row (they were dropped before,
    // when usage drained at the instant of stop).
    let records = crate::llm::usage::drain_meeting_usage(&user_id, &chat_llm, &background_llm);
    for record in records {
        let billable_input = record
            .input_tokens
            .saturating_sub(record.cached_input_tokens);
        info!(
            user_id = %user_id,
            pool = record.pool,
            calls = record.calls,
            input_tokens = record.input_tokens,
            output_tokens = record.output_tokens,
            cached_input_tokens = record.cached_input_tokens,
            billable_input_tokens = billable_input,
            provider = %record.provider,
            model_id = %record.model_id,
            "llm_usage_at_finalize"
        );
        if let Err(e) = crate::storage::meetings::insert_meeting_llm_usage(
            &db,
            &meeting_id,
            record.pool,
            &record.provider,
            &record.model_id,
            record.calls,
            record.input_tokens,
            record.output_tokens,
            record.cached_input_tokens,
        )
        .await
        {
            warn!(error = ?e, %meeting_id, pool = %record.pool, "insert_meeting_llm_usage failed");
        }
    }

    // Tear down the remaining live tasks (summarizer, chat, active) FIRST.
    // The STT task was already awaited/cancelled above. Awaiting the
    // summarizer here is still a happens-before barrier: once shutdown
    // returns, every PRE-STOP `ItemsUpdate{transcript}` the summarizer was
    // going to broadcast is already on `events_tx`. (The post-stop drain
    // tail itself was broadcast above as `TranscriptTail`, before the LLM
    // pass.) Sending `MeetingFinalized` only after this keeps the mnemo
    // pusher's session reset strictly after every transcript event for
    // this meeting.
    runtime.shutdown().await;

    // Now tell the mnemo pusher the offline pass is done so it resets its
    // session. Server-internal (filtered from clients in the WS forward
    // loop). Sent last, after the summarizer's events are all enqueued.
    bus.emit(
        user_id.clone(),
        Event::MeetingFinalized {
            meeting_id: meeting_id.clone(),
        },
    )
    .await;

    info!(user_id = %user_id, meeting_id = %meeting_id, "finalize complete");
}

#[cfg(test)]
mod tests {
    use super::{assemble_transcript, tail_event};
    use crate::protocol::Event;
    use crate::stt::TranscriptChunk;

    fn chunk(id: &str, text: &str, t: u64, speaker: Option<&str>) -> TranscriptChunk {
        TranscriptChunk {
            id: id.into(),
            text: text.into(),
            t_start_ms: t,
            t_end_ms: t + 1000,
            speaker: speaker.map(str::to_string),
            user_id: "u-1".into(),
        }
    }

    #[test]
    fn assemble_appends_tail_with_newlines() {
        let out = assemble_transcript(
            "first line\nsecond line".into(),
            &["third line".to_string(), "fourth line".to_string()],
        );
        assert_eq!(out, "first line\nsecond line\nthird line\nfourth line");
    }

    #[test]
    fn assemble_handles_empty_pre_stop() {
        let out = assemble_transcript(String::new(), &["only line".to_string()]);
        assert_eq!(out, "only line");
    }

    #[test]
    fn assemble_handles_empty_tail() {
        let out = assemble_transcript("just the snapshot".into(), &[]);
        assert_eq!(out, "just the snapshot");
    }

    /// Regression (improvement #19): an empty drain must NOT broadcast —
    /// a zero-item TranscriptTail would just churn the persistence loop.
    #[test]
    fn tail_event_is_none_for_empty_tail() {
        assert!(tail_event("u-1", "m-1", &[]).is_none());
    }

    /// Regression (improvement #19): the drained tail must leave finalize
    /// as a server-internal TranscriptTail event carrying FULL items
    /// (id / start-time / speaker preserved), addressed by meeting_id —
    /// this is what the persistence loop appends to the stopped
    /// meeting's JSONL and what the mnemo pusher pushes. Before this
    /// fix the tail existed only as bare `Vec<String>` for the LLM pass
    /// and never reached either durable sink.
    #[test]
    fn tail_event_carries_meeting_id_and_full_items() {
        let tail = vec![
            chunk("c1", "so let's wrap up", 90_000, Some("2")),
            chunk("c2", "I'll send that tomorrow", 93_000, None),
        ];
        let ev = tail_event("u-1", "m-1", &tail).expect("non-empty tail must build an event");
        assert_eq!(ev.user_id, "u-1");
        match ev.event {
            Event::TranscriptTail { meeting_id, items } => {
                assert_eq!(meeting_id, "m-1");
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].id, "c1");
                assert_eq!(items[0].text, "so let's wrap up");
                assert_eq!(items[0].t, 90_000);
                assert_eq!(
                    items[0].meta.as_ref().unwrap()["speaker"],
                    serde_json::json!("2")
                );
                assert_eq!(items[1].id, "c2");
                assert!(items[1].meta.is_none());
            }
            other => panic!("expected TranscriptTail, got {other:?}"),
        }
    }
}
