//! `EventBus` — the server's two-lane event plumbing.
//!
//! **Fan-out lane** (`tokio::broadcast`, lossy by design): per-
//! connection WS forward loops, the STT status channel and the mnemo
//! recaller subscribe here. A subscriber that falls more than
//! [`FANOUT_CAPACITY`] events behind gets `RecvError::Lagged` — for a
//! client connection that means "disconnect and resnapshot", which is
//! the correct UX cost. Nothing durable rides this lane anymore.
//!
//! **Durable lane** (bounded `mpsc`, loss-proof): consumed by the
//! single durable-writer task (`storage::persistence_loop::
//! spawn_durable_writer`) which owns the transcript JSONL, the items
//! table, and the mnemo pusher. `emit` awaits the mpsc send, so when
//! the writer can't keep up producers slow down (backpressure)
//! instead of silently losing the system of record — the failure mode
//! the old 64-slot broadcast ring had.
//!
//! Routing is decided centrally by [`crate::protocol::Event::is_durable`];
//! the `durable` sender is private so no call site can bypass it.

use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, warn};

use crate::protocol::{Event, UserEvent};

/// Capacity of the client fan-out broadcast ring. Matches the
/// historical channel size; slow clients get disconnected by their
/// forward loop's `Lagged` arm — never data loss.
pub const FANOUT_CAPACITY: usize = 64;

/// Capacity of the durable mpsc queue. Committed transcript speech is
/// ~1-2 events/s, so 256 slots is minutes of backlog; if Postgres is
/// hard-down longer than that, producers backpressure in `emit`
/// rather than dropping (and the writer's retry-then-drop keeps the
/// queue moving — see `retry_durable_write`).
pub const DURABLE_QUEUE_CAPACITY: usize = 256;

#[derive(Clone)]
pub struct EventBus {
    /// Lossy client fan-out. Public: forward loops subscribe via
    /// `bus.fanout.subscribe()`, and the STT provider trait keeps its
    /// `broadcast::Sender` signature (`bus.fanout.clone()`).
    pub fanout: broadcast::Sender<UserEvent>,
    /// Durable lane into the single writer task. Private so the
    /// `is_durable` routing in `emit` cannot be bypassed.
    durable: mpsc::Sender<UserEvent>,
}

impl EventBus {
    pub fn new(fanout: broadcast::Sender<UserEvent>, durable: mpsc::Sender<UserEvent>) -> Self {
        Self { fanout, durable }
    }

    /// Emit an event for `user_id`. Durable events (see
    /// [`Event::is_durable`]) are sent — awaited, backpressured,
    /// never dropped — to the durable writer BEFORE the lossy
    /// fan-out send.
    pub async fn emit(&self, user_id: impl Into<String>, event: Event) {
        self.send(UserEvent::new(user_id.into(), event)).await;
    }

    /// `emit`, with the producer's known meeting id stamped on the
    /// envelope so the durable writer skips the registry lookup —
    /// see `UserEvent::meeting_id`.
    pub async fn emit_for_meeting(
        &self,
        user_id: impl Into<String>,
        meeting_id: impl Into<String>,
        event: Event,
    ) {
        self.send(UserEvent::with_meeting(user_id, meeting_id, event))
            .await;
    }

    /// Fan-out-only emit for high-frequency or purely-cosmetic
    /// traffic (heartbeat `Status`, chat streaming partials). Sync —
    /// no await — so heartbeat-style call sites stay simple. Skips
    /// the durable queue even for events `is_durable()` would route.
    pub fn emit_fanout_only(&self, user_id: impl Into<String>, event: Event) {
        if let Err(err) = self.fanout.send(UserEvent::new(user_id.into(), event)) {
            debug!(error = %err, "broadcast: no live subscribers");
        }
    }

    async fn send(&self, envelope: UserEvent) {
        if envelope.event.is_durable() {
            // Early-warning before backpressure bites: >50% full means
            // the writer is falling behind its producers.
            if self.durable.capacity() < self.durable.max_capacity() / 2 {
                warn!(
                    remaining = self.durable.capacity(),
                    max = self.durable.max_capacity(),
                    "durable queue more than half full — writer falling behind"
                );
            }
            if let Err(e) = self.durable.send(envelope.clone()).await {
                // Only possible when the writer task is gone (post-
                // shutdown). Loud: this is genuine data loss.
                error!(error = %e, "durable queue closed; durable event LOST");
            }
        }
        if let Err(err) = self.fanout.send(envelope) {
            debug!(error = %err, "broadcast: no live subscribers");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Event, Item};
    use std::time::Duration;
    use tokio::sync::{broadcast, mpsc};

    fn durable_event() -> Event {
        Event::ItemsUpdate {
            mode: "transcript".into(),
            items: vec![Item {
                id: "t-1".into(),
                text: "hello.".into(),
                detail: None,
                t: 0,
                meta: None,
            }],
        }
    }

    #[tokio::test]
    async fn emit_routes_durable_event_to_both_lanes() {
        let (fanout, mut fanout_rx) = broadcast::channel(8);
        let (durable_tx, mut durable_rx) = mpsc::channel(8);
        let bus = EventBus::new(fanout, durable_tx);

        bus.emit("u1", durable_event()).await;

        let d = durable_rx.try_recv().expect("durable lane got the event");
        assert_eq!(d.user_id, "u1");
        assert!(matches!(d.event, Event::ItemsUpdate { .. }));
        let f = fanout_rx.try_recv().expect("fanout lane got the event");
        assert_eq!(f.user_id, "u1");
    }

    #[tokio::test]
    async fn emit_skips_durable_lane_for_fanout_only_events() {
        let (fanout, mut fanout_rx) = broadcast::channel(8);
        let (durable_tx, mut durable_rx) = mpsc::channel(8);
        let bus = EventBus::new(fanout, durable_tx);

        bus.emit(
            "u1",
            Event::TranscriptInterim {
                text: "in flight".into(),
            },
        )
        .await;

        assert!(
            durable_rx.try_recv().is_err(),
            "interims must not consume durable-queue slots"
        );
        assert!(
            fanout_rx.try_recv().is_ok(),
            "clients still get the interim"
        );
    }

    #[tokio::test]
    async fn emit_fanout_only_never_touches_durable_lane() {
        // Even for an event that is_durable() would classify durable —
        // this is the escape hatch broadcast_chat_partial uses for the
        // terminal (streaming:false) chat partial.
        let (fanout, mut fanout_rx) = broadcast::channel(8);
        let (durable_tx, mut durable_rx) = mpsc::channel(8);
        let bus = EventBus::new(fanout, durable_tx);

        bus.emit_fanout_only("u1", durable_event());

        assert!(
            durable_rx.try_recv().is_err(),
            "emit_fanout_only must bypass the durable queue"
        );
        assert!(fanout_rx.try_recv().is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn emit_blocks_when_durable_queue_full_instead_of_dropping() {
        // The core regression this whole improvement exists for: when
        // the durable consumer stalls, producers must backpressure —
        // not drop like broadcast's Lagged ring did.
        let (fanout, _fanout_rx) = broadcast::channel(8);
        let (durable_tx, mut durable_rx) = mpsc::channel(1);
        let bus = EventBus::new(fanout, durable_tx);

        // Fill the capacity-1 queue; the consumer is paused (we just
        // don't recv).
        bus.emit("u1", durable_event()).await;

        // Second durable emit must stay pending, not complete-and-drop.
        let second = bus.emit("u1", durable_event());
        tokio::pin!(second);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut second)
                .await
                .is_err(),
            "emit must apply backpressure while the queue is full, not drop the event"
        );

        // Draining one slot unblocks the pending emit.
        let _ = durable_rx.recv().await;
        tokio::time::timeout(Duration::from_millis(50), second)
            .await
            .expect("emit completes once a slot frees");
        let drained = durable_rx
            .recv()
            .await
            .expect("second event arrived intact");
        assert!(matches!(drained.event, Event::ItemsUpdate { .. }));
    }
}
