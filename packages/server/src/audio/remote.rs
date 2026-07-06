//! `RemoteAudioSource` — accepts PCM frames over the `/audio`
//! WebSocket endpoint. The server itself does not capture; a separate
//! client (the Mac app, or `wscat` for testing) opens `/audio` and
//! streams binary frames of 16 kHz mono S16LE PCM, ~640 bytes each.
//!
//! Lifecycle (per-meeting, late-binding):
//!   1. A meeting begins: `MeetingRuntime::new` creates a fresh
//!      `RemoteAudioSource` owned by that runtime — one per meeting,
//!      so audio buffered for one meeting never bleeds into the next.
//!   2. `spawn_live_pipeline` calls `start()`, which allocates an
//!      mpsc channel, stores the *sender* in the slot, and hands the
//!      *receiver* to the STT pipeline.
//!   3. A client connects to `/audio`: the handler resolves the
//!      active meeting's source via
//!      `SessionRegistry::audio_source_for_active_meeting`, queries
//!      the slot for the current sender, and forwards each PCM frame
//!      into it. Sender-cache refreshes re-resolve through the
//!      registry every time (see `resolve_audio_sender` in
//!      `ws/control.rs`) — never through a cached Arc, because the
//!      next meeting's source is a different object. Multiple
//!      connections (e.g., reconnect mid-meeting) can sequentially
//!      pick up the same sender — the pipeline's receiver stays
//!      alive across them.
//!   4. Meeting ends: STT drops the receiver. The stored sender
//!      becomes Closed; the next `current_sender()` self-cleans the
//!      slot to None. The source itself drops with the
//!      `MeetingRuntime`; any handler still holding an Arc clone just
//!      sees `current_sender() == None` forever — which is exactly
//!      why handlers must re-resolve instead of caching.
//!
//! This shape replaces two earlier designs: the "install rx, take rx"
//! pattern (broke when `/audio` reconnected mid-meeting — the new rx
//! sat unconsumed in the slot while STT held the dead one), and the
//! process-singleton source stored on `ServerHandle` (replaced by the
//! per-meeting source above; the singleton let one meeting's stale
//! buffered audio leak into the next).

use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

#[derive(Clone, Default)]
pub struct RemoteAudioSource {
    /// Current meeting's audio sink. Set by `start()`; cleared
    /// lazily by `current_sender()` once its rx has been dropped
    /// (meeting ended). `Arc<Mutex<...>>` because the meeting
    /// starter and the `/audio` handler(s) live on different async
    /// tasks.
    inner: Arc<Mutex<Option<mpsc::Sender<Vec<u8>>>>>,
}

impl RemoteAudioSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called by `AudioSource::start` when a meeting begins.
    /// Allocates the audio mpsc channel, stores its `Sender` in
    /// the slot for `/audio` handlers to forward into, and returns
    /// the `Receiver` for the STT pipeline.
    ///
    /// Late-binding: always succeeds. If no `/audio` client is
    /// connected, the returned rx simply yields nothing until one
    /// arrives. If the active `/audio` disconnects mid-meeting, the
    /// rx pauses until it reconnects.
    pub async fn start(&self) -> mpsc::Receiver<Vec<u8>> {
        // 80 frames ≈ 1.6 s of audio at 50 fps, 640 B each. Same
        // budget the per-connection forwarder used to use.
        let (tx, rx) = mpsc::channel::<Vec<u8>>(80);
        let mut slot = self.inner.lock().await;
        *slot = Some(tx);
        rx
    }

    /// Returns the active meeting's audio `Sender`, or `None` if
    /// no meeting is running. Self-cleans when the stored sender
    /// is closed (the rx was dropped — meeting ended), so callers
    /// don't need to know about meeting lifecycle.
    pub async fn current_sender(&self) -> Option<mpsc::Sender<Vec<u8>>> {
        let mut slot = self.inner.lock().await;
        if let Some(tx) = slot.as_ref() {
            if tx.is_closed() {
                *slot = None;
            }
        }
        slot.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn start_creates_an_open_channel_with_no_audio_client() {
        // Late-binding: start() must succeed regardless of whether
        // anyone has connected to /audio. The rx simply yields
        // nothing until a sender forwards frames into the slot.
        let src = RemoteAudioSource::new();
        let mut rx = src.start().await;

        // Forward into the slot via current_sender — simulates an
        // `/audio` client connecting *after* start.
        let tx = src.current_sender().await.expect("slot populated by start");
        tx.send(b"frame".to_vec()).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received, b"frame");
    }

    #[tokio::test]
    async fn current_sender_self_cleans_when_meeting_ends() {
        // Drop the rx (meeting ended) and confirm the slot
        // transitions back to None on the next current_sender lookup.
        let src = RemoteAudioSource::new();
        let rx = src.start().await;
        assert!(src.current_sender().await.is_some());

        drop(rx);
        let after = src.current_sender().await;
        assert!(
            after.is_none(),
            "slot should self-clean once its rx is dropped"
        );
    }

    #[tokio::test]
    async fn current_sender_none_when_no_meeting() {
        let src = RemoteAudioSource::new();
        assert!(src.current_sender().await.is_none());
    }

    #[tokio::test]
    async fn reconnect_mid_meeting_keeps_pipeline_rx_alive() {
        // Two sequential "/audio" clients forward into the same
        // slot; the meeting's rx (taken once at start) sees both
        // streams of frames as if from a single producer.
        let src = RemoteAudioSource::new();
        let mut rx = src.start().await;

        // First "client".
        let tx1 = src.current_sender().await.unwrap();
        tx1.send(b"a".to_vec()).await.unwrap();
        drop(tx1); // simulate /audio disconnect

        // Stored sender in the slot is *the channel's* sender,
        // distinct from tx1 (which was a clone). It survives.
        // Second "client" reconnects, picks up the live sender,
        // forwards more frames.
        let tx2 = src.current_sender().await.unwrap();
        tx2.send(b"b".to_vec()).await.unwrap();

        assert_eq!(rx.recv().await.unwrap(), b"a");
        assert_eq!(rx.recv().await.unwrap(), b"b");
    }
}
