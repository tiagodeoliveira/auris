//! `MeetingRuntime` — meeting-scoped state owned by `UserSession`.
//!
//! Exists only while a meeting is active (`UserSession.meeting.is_some()`).
//! Dropping this struct is sufficient to release all meeting-only state
//! in one move — including the cancellation token (tasks must call
//! `cancel.cancel()` before drop to signal workers) and the audio pipe
//! (each meeting gets a fresh `RemoteAudioSource`; the old one is
//! destroyed when the runtime drops).

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::audio::RemoteAudioSource;
use crate::mnemo::RecalledContext;
use crate::protocol::AssistSensitivity;
use crate::stt::TranscriptChunk;

pub struct MeetingRuntime {
    /// Stable UUID for this meeting — mirrors `meetings.id` in the DB.
    pub meeting_id: String,
    /// Monotonic start time. Process-local; can't be reconstructed
    /// after a crash. Use the DB row's `started_at` for wall-clock needs.
    pub started_at_instant: Instant,
    /// Wall-clock start time. Stored here so callers don't need a DB
    /// round-trip for display / log purposes.
    pub started_at_wall: DateTime<Utc>,

    /// Rolling buffer of finalized transcript chunks since meeting start.
    /// Cleared implicitly when the `MeetingRuntime` is dropped.
    pub rolling_transcript: Vec<TranscriptChunk>,
    /// Memories recalled from mnemo at meeting start. `None` until
    /// recall completes (or if mnemo is disabled / failed).
    pub recalled_context: Option<RecalledContext>,
    /// Device that's currently feeding audio into the meeting.
    /// `None` until a `/audio` client is bound.
    pub audio_source_device_id: Option<String>,
    /// How aggressively the agent surfaces proactive assist
    /// suggestions for this meeting. Drives both the server-side
    /// confidence threshold in `agent::tools::assist` and the
    /// nudge included in the agent's bootstrap system prompt.
    /// Defaults to `Moderate`; updated mid-meeting via the
    /// `SetAssistSensitivity` intent (persisted to the meeting row
    /// so the value survives reconnect).
    pub assist_sensitivity: AssistSensitivity,

    /// Cancellation token for all tasks spawned by `spawn_live_pipeline`.
    /// Cancelled explicitly in `handle_stop_meeting` before the runtime
    /// drops so workers see the signal synchronously rather than racing
    /// with the drop.
    pub cancel: CancellationToken,
    /// Graceful-drain signal for the STT task ONLY. Distinct from
    /// `cancel` (which is a hard stop for every task). When fired, the
    /// STT provider stops accepting new audio, flushes its socket to
    /// the provider's end-of-stream, captures trailing finals, then
    /// exits cleanly. Used by `workers::finalize` so the last spoken
    /// sentences aren't lost when a meeting stops.
    pub drain: CancellationToken,
    /// Cancellation scope for the REACTIVE agents only (chat + active
    /// extractor). A child of `cancel`. The finalize task fires this
    /// right after `trigger_drain()` so those agents stop firing on the
    /// drained tail (their writes no-op once Idle — pure wasted tokens),
    /// while the STT + transcript-summarizer tasks (children of `cancel`)
    /// stay alive to flush the drain. `cancel.cancel()` still covers them
    /// as a catch-all.
    pub reactive_cancel: CancellationToken,
    /// Broadcast sender for finalized transcript chunks. Created here
    /// (rather than in `spawn_live_pipeline`) so the finalize task can
    /// `subscribe()` and collect the chunks produced during the drain.
    pub chunk_tx: broadcast::Sender<TranscriptChunk>,
    /// The STT task handle, kept separate from `tasks` so finalize can
    /// await ONLY the STT drain (bounded) before tearing the rest down.
    pub stt_task: Option<JoinHandle<()>>,
    /// Per-meeting PCM pipe. Created fresh for each meeting so audio
    /// from one meeting never bleeds into the next. Dropped with the
    /// runtime — callers that hold an `Arc` clone (e.g., spawned STT
    /// pipelines) continue working until they also release their share.
    pub audio_source: Arc<RemoteAudioSource>,
    /// All meeting-scoped `JoinHandle`s registered via `register_task`.
    /// `shutdown` drains this list after cancelling the token.
    pub tasks: Vec<JoinHandle<()>>,
}

impl MeetingRuntime {
    pub fn new(meeting_id: String, started_at_wall: DateTime<Utc>) -> Self {
        let cancel = CancellationToken::new();
        let reactive_cancel = cancel.child_token();
        Self {
            meeting_id,
            started_at_instant: Instant::now(),
            started_at_wall,
            rolling_transcript: Vec::new(),
            recalled_context: None,
            audio_source_device_id: None,
            assist_sensitivity: AssistSensitivity::default(),
            cancel,
            drain: CancellationToken::new(),
            reactive_cancel,
            chunk_tx: broadcast::channel(64).0,
            stt_task: None,
            audio_source: Arc::new(RemoteAudioSource::new()),
            tasks: Vec::new(),
        }
    }

    /// Register a spawned task with this runtime so `shutdown` can
    /// await it.
    pub fn register_task(&mut self, handle: JoinHandle<()>) {
        self.tasks.push(handle);
    }

    /// Clone of the drain signal, for the STT spawn site.
    pub fn drain_token(&self) -> CancellationToken {
        self.drain.clone()
    }

    /// Clone of the transcript-chunk sender, for the live pipeline's
    /// STT/summarizer/active subscribers.
    pub fn chunk_sender(&self) -> broadcast::Sender<TranscriptChunk> {
        self.chunk_tx.clone()
    }

    /// Fresh subscriber to the transcript-chunk stream, for the
    /// finalize task to collect chunks produced during the drain.
    pub fn subscribe_chunks(&self) -> broadcast::Receiver<TranscriptChunk> {
        self.chunk_tx.subscribe()
    }

    /// Register the STT task handle (kept apart from `tasks`).
    pub fn set_stt_task(&mut self, handle: JoinHandle<()>) {
        self.stt_task = Some(handle);
    }

    /// Take the STT task handle out for a bounded await.
    pub fn take_stt_task(&mut self) -> Option<JoinHandle<()>> {
        self.stt_task.take()
    }

    /// Fire the graceful-drain signal for the STT task.
    pub fn trigger_drain(&self) {
        self.drain.cancel();
    }

    /// Clone of the reactive-agent cancel token, for the chat/active
    /// spawn sites.
    pub fn reactive_token(&self) -> CancellationToken {
        self.reactive_cancel.clone()
    }

    /// Cancel ONLY the reactive agents (chat + active). The STT and
    /// transcript-summarizer tasks keep running.
    pub fn cancel_reactive_agents(&self) {
        self.reactive_cancel.cancel();
    }

    /// Cancel the meeting token, then await every registered task
    /// up to `SHUTDOWN_TASK_TIMEOUT`. Logs (but doesn't fail) on
    /// any task that misses the deadline.
    pub async fn shutdown(self) {
        const SHUTDOWN_TASK_TIMEOUT: Duration = Duration::from_secs(2);
        let Self {
            cancel,
            tasks,
            stt_task,
            meeting_id,
            ..
        } = self;
        cancel.cancel();
        // Abort any STT task the finalize path didn't already take. After
        // `take_stt_task()` this is `None` and the abort is a no-op; on
        // non-finalize shutdowns (server stop, disconnect) it prevents the
        // STT task from detaching and running past teardown.
        if let Some(stt) = stt_task {
            stt.abort();
            let _ = stt.await;
        }
        for (i, task) in tasks.into_iter().enumerate() {
            match tokio::time::timeout(SHUTDOWN_TASK_TIMEOUT, task).await {
                Ok(Ok(())) => {}
                Ok(Err(join_err)) => {
                    tracing::warn!(
                        meeting_id = %meeting_id,
                        task_idx = i,
                        error = %join_err,
                        "meeting task panicked or was cancelled abnormally"
                    );
                }
                Err(_timeout) => {
                    tracing::warn!(
                        meeting_id = %meeting_id,
                        task_idx = i,
                        timeout_ms = SHUTDOWN_TASK_TIMEOUT.as_millis(),
                        "meeting task did not exit within shutdown timeout"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn shutdown_cancels_and_awaits_tasks() {
        let mut rt = MeetingRuntime::new("m1".into(), Utc::now());
        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..3 {
            let cancel = rt.cancel.clone();
            let c = counter.clone();
            rt.register_task(tokio::spawn(async move {
                cancel.cancelled().await;
                c.fetch_add(1, Ordering::SeqCst);
            }));
        }
        rt.shutdown().await;
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn shutdown_timeout_on_stuck_task_does_not_hang() {
        let mut rt = MeetingRuntime::new("m1".into(), Utc::now());
        rt.register_task(tokio::spawn(async {
            sleep(Duration::from_secs(60)).await;
        }));
        let start = std::time::Instant::now();
        rt.shutdown().await;
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn shutdown_aborts_lingering_stt_task() {
        // An STT task the finalize path never took must be aborted by
        // shutdown(), not detached. A 60s sleep would outlive the test
        // if it leaked; shutdown must return promptly.
        let mut rt = MeetingRuntime::new("m1".into(), Utc::now());
        let done = Arc::new(AtomicUsize::new(0));
        let d = done.clone();
        rt.set_stt_task(tokio::spawn(async move {
            sleep(Duration::from_secs(60)).await;
            d.fetch_add(1, Ordering::SeqCst); // only runs if NOT aborted
        }));
        let start = std::time::Instant::now();
        rt.shutdown().await;
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "shutdown hung on STT task"
        );
        assert_eq!(
            done.load(Ordering::SeqCst),
            0,
            "STT task should have been aborted, not allowed to complete"
        );
    }

    #[tokio::test]
    async fn trigger_drain_fires_only_the_drain_token() {
        let rt = MeetingRuntime::new("m1".into(), Utc::now());
        assert!(!rt.drain.is_cancelled());
        assert!(!rt.cancel.is_cancelled());
        rt.trigger_drain();
        assert!(rt.drain.is_cancelled(), "drain should be fired");
        assert!(
            !rt.cancel.is_cancelled(),
            "hard cancel must NOT fire on drain"
        );
    }

    #[tokio::test]
    async fn chunk_sender_and_subscriber_share_the_channel() {
        let rt = MeetingRuntime::new("m1".into(), Utc::now());
        let mut rx = rt.subscribe_chunks();
        let tx = rt.chunk_sender();
        let chunk = TranscriptChunk {
            id: "c1".into(),
            text: "hello".into(),
            t_start_ms: 0,
            t_end_ms: 100,
            speaker: None,
            user_id: "u1".into(),
        };
        tx.send(chunk).unwrap();
        let got = rx.recv().await.unwrap();
        assert_eq!(got.text, "hello");
    }

    #[tokio::test]
    async fn set_then_take_stt_task_round_trips() {
        let mut rt = MeetingRuntime::new("m1".into(), Utc::now());
        assert!(rt.take_stt_task().is_none());
        rt.set_stt_task(tokio::spawn(async {}));
        let h = rt.take_stt_task();
        assert!(h.is_some());
        h.unwrap().await.unwrap();
        assert!(rt.take_stt_task().is_none(), "take must clear the slot");
    }

    #[tokio::test]
    async fn cancel_reactive_agents_fires_reactive_not_parent() {
        let rt = MeetingRuntime::new("m1".into(), Utc::now());
        assert!(!rt.reactive_cancel.is_cancelled());
        rt.cancel_reactive_agents();
        assert!(rt.reactive_cancel.is_cancelled(), "reactive should fire");
        assert!(
            !rt.cancel.is_cancelled(),
            "parent cancel must NOT fire when only reactive is cancelled"
        );
    }

    #[tokio::test]
    async fn parent_cancel_propagates_to_reactive_children() {
        let rt = MeetingRuntime::new("m1".into(), Utc::now());
        let child = rt.reactive_token().child_token();
        rt.cancel.cancel();
        assert!(
            rt.reactive_cancel.is_cancelled(),
            "reactive_cancel fires as a child of cancel"
        );
        assert!(
            child.is_cancelled(),
            "reactive children fire when parent cancels"
        );
    }
}
