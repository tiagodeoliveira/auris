//! Per-user JWT cache + deferred-push queue.
//!
//! Auris and mnemo share one Auth0 audience (kleos), so the JWT the
//! UI presents at WS handshake works for mnemo too. We cache it here
//! keyed by the local `users.id` so the mnemo client can pull it on
//! every push. Two write paths populate the store:
//!
//! - WS handshake: validated token cached immediately on connect.
//! - `Intent::SetAuthToken`: UI refreshes the cached token mid-session
//!   (typically on Auth0 silent-refresh) without dropping the WS.
//!
//! When a push fails for a recoverable reason — no cached token, 401
//! on the cached token, a network error, a 5xx/429 from mnemo, or an
//! open circuit breaker — the event is queued here instead of
//! dropped. The next handshake / SetAuthToken for that user and a
//! periodic 30s retry task both drain the queue FIFO (order matters:
//! mnemo stamps created_at at ingest), so neither an auth gap nor a
//! mnemo outage/restart loses data.
//!
//! In-memory only — an auris restart clears tokens AND any queued
//! events (each WS reconnect re-populates tokens from the handshake;
//! events queued across a restart are lost — accepted residual gap,
//! recoverable via the offline recover-meeting binary).

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, RwLock};

use tracing::warn;

use super::payload::IngestEvent;

/// Per-user queue cap. A long meeting produces ~50-200 transcript
/// items; 1000 covers ~5 long meetings of backlog before we start
/// dropping the oldest. Hardcoded — it's a memory ceiling, not a
/// behavioral knob.
const QUEUE_CAP_PER_USER: usize = 1000;

#[derive(Debug, Default)]
pub struct MnemoTokenStore {
    // RwLock: reads (every push) heavily outnumber writes (only on
    // SetMnemoToken intent or 401 eviction).
    tokens: RwLock<HashMap<String, String>>,
    // Mutex: every enqueue and drain writes. No contention benefit
    // from RwLock here.
    pending: Mutex<HashMap<String, VecDeque<IngestEvent>>>,
}

impl MnemoTokenStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current cached JWT for `user_id`, if any.
    pub fn get(&self, user_id: &str) -> Option<String> {
        self.tokens.read().unwrap().get(user_id).cloned()
    }

    /// Replace the cached JWT for `user_id`. Queued events are NOT
    /// drained here — after depositing a token, call
    /// `MnemoClient::drain_pending`, which replays FIFO and re-queues
    /// on transient failure instead of discarding the backlog.
    pub fn store(&self, user_id: &str, token: String) {
        self.tokens
            .write()
            .unwrap()
            .insert(user_id.to_string(), token);
    }

    /// Evict the cached JWT for `user_id`. Called on 401 from mnemo,
    /// where the cached token is either expired or revoked — future
    /// pushes will queue until the UI sends a fresh token.
    pub fn clear(&self, user_id: &str) {
        self.tokens.write().unwrap().remove(user_id);
    }

    /// Append an event to the user's pending queue. Bounded: once
    /// the queue reaches `QUEUE_CAP_PER_USER`, the oldest event is
    /// dropped with a warn. Better to lose the oldest than to grow
    /// unbounded if a user never returns.
    pub fn enqueue(&self, user_id: &str, event: IngestEvent) {
        let mut pending = self.pending.lock().unwrap();
        let queue = pending.entry(user_id.to_string()).or_default();
        if queue.len() >= QUEUE_CAP_PER_USER {
            queue.pop_front();
            warn!(
                user_id,
                cap = QUEUE_CAP_PER_USER,
                "mnemo: queue full, dropped oldest event"
            );
        }
        queue.push_back(event);
    }

    /// Remove and return the user's entire pending queue, FIFO order.
    /// The drain half of what `store()` used to do — callers replay
    /// each event and `requeue_front` whatever fails transiently.
    pub fn take_pending(&self, user_id: &str) -> Vec<IngestEvent> {
        self.pending
            .lock()
            .unwrap()
            .remove(user_id)
            .map(|q| q.into_iter().collect())
            .unwrap_or_default()
    }

    /// Put a failed drain batch back at the FRONT of the queue so a
    /// retry replays it before anything enqueued while the drain ran —
    /// mnemo stamps `created_at` at ingest, so per-user FIFO is the
    /// only thing keeping the stored transcript in order. Respects
    /// `QUEUE_CAP_PER_USER` by dropping oldest (front) with a warn,
    /// matching `enqueue`'s drop policy.
    pub fn requeue_front(&self, user_id: &str, events: Vec<IngestEvent>) {
        if events.is_empty() {
            return;
        }
        let mut pending = self.pending.lock().unwrap();
        let queue = pending.entry(user_id.to_string()).or_default();
        for event in events.into_iter().rev() {
            queue.push_front(event);
        }
        let mut dropped = 0usize;
        while queue.len() > QUEUE_CAP_PER_USER {
            queue.pop_front();
            dropped += 1;
        }
        if dropped > 0 {
            warn!(
                user_id,
                dropped,
                cap = QUEUE_CAP_PER_USER,
                "mnemo: queue full after requeue, dropped oldest events"
            );
        }
    }

    /// Users that currently have at least one queued event. Drives
    /// the periodic drain task's per-tick sweep.
    pub fn users_with_pending(&self) -> Vec<String> {
        self.pending
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, q)| !q.is_empty())
            .map(|(user_id, _)| user_id.clone())
            .collect()
    }

    /// Total queued events for `user_id`. Used by the periodic drain
    /// task (skip users with nothing pending) and by tests.
    pub fn pending_len(&self, user_id: &str) -> usize {
        self.pending
            .lock()
            .unwrap()
            .get(user_id)
            .map(|q| q.len())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::mnemo::payload::{Turn, TurnRole};

    fn event(session_id: &str) -> IngestEvent {
        IngestEvent {
            session_id: session_id.into(),
            source: "auris".into(),
            workstation: "h".into(),
            workdir: "/auris".into(),
            project: None,
            turns: vec![Turn {
                role: TurnRole::User,
                content: "x".into(),
            }],
            attributes: HashMap::new(),
        }
    }

    #[test]
    fn get_returns_none_for_unknown_user() {
        let s = MnemoTokenStore::new();
        assert!(s.get("nobody").is_none());
    }

    #[test]
    fn store_then_get_roundtrip() {
        let s = MnemoTokenStore::new();
        s.store("u1", "jwt-1".into());
        assert_eq!(s.get("u1").as_deref(), Some("jwt-1"));
    }

    #[test]
    fn store_does_not_drain_pending() {
        let s = MnemoTokenStore::new();
        s.enqueue("u1", event("a"));
        s.store("u1", "jwt-1".into());
        assert_eq!(
            s.pending_len("u1"),
            1,
            "store() must leave the queue intact; drain_pending owns replay + requeue"
        );
    }

    #[test]
    fn clear_evicts_token() {
        let s = MnemoTokenStore::new();
        s.store("u1", "jwt-1".into());
        s.clear("u1");
        assert!(s.get("u1").is_none());
    }

    #[test]
    fn enqueue_overflow_drops_oldest() {
        let s = MnemoTokenStore::new();
        for i in 0..(QUEUE_CAP_PER_USER + 5) {
            s.enqueue("u1", event(&format!("s-{i}")));
        }
        assert_eq!(s.pending_len("u1"), QUEUE_CAP_PER_USER);
        let drained = s.take_pending("u1");
        // Oldest 5 dropped; surviving range is s-5 .. s-1004.
        assert_eq!(drained[0].session_id, "s-5");
        assert_eq!(
            drained.last().unwrap().session_id,
            format!("s-{}", QUEUE_CAP_PER_USER + 4)
        );
    }

    #[test]
    fn take_pending_is_per_user() {
        let s = MnemoTokenStore::new();
        s.enqueue("u1", event("a"));
        s.enqueue("u2", event("b"));
        let drained = s.take_pending("u1");
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].session_id, "a");
        // u2's queue is untouched.
        assert_eq!(s.pending_len("u2"), 1);
    }

    #[test]
    fn take_pending_removes_and_preserves_order() {
        let s = MnemoTokenStore::new();
        s.enqueue("u1", event("a"));
        s.enqueue("u1", event("b"));
        let taken = s.take_pending("u1");
        assert_eq!(taken.len(), 2);
        assert_eq!(taken[0].session_id, "a");
        assert_eq!(taken[1].session_id, "b");
        assert_eq!(s.pending_len("u1"), 0, "take_pending must empty the queue");
    }

    #[test]
    fn take_pending_empty_for_unknown_user() {
        let s = MnemoTokenStore::new();
        assert!(s.take_pending("nobody").is_empty());
    }

    #[test]
    fn requeue_front_preserves_relative_order() {
        let s = MnemoTokenStore::new();
        // "c" was enqueued fresh while a drain held [a, b]; the drain
        // failed and puts its batch BACK AT THE FRONT — replay order
        // must come out a, b, c.
        s.enqueue("u1", event("c"));
        s.requeue_front("u1", vec![event("a"), event("b")]);
        let taken = s.take_pending("u1");
        let ids: Vec<&str> = taken.iter().map(|e| e.session_id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn requeue_front_over_cap_drops_oldest() {
        let s = MnemoTokenStore::new();
        for i in 0..QUEUE_CAP_PER_USER {
            s.enqueue("u1", event(&format!("q-{i}")));
        }
        // Requeueing 2 events at the front overflows the cap by 2.
        // Drop policy is oldest-first (same as enqueue's), and the
        // requeued events sit at the front, so they are sacrificed.
        s.requeue_front("u1", vec![event("r-0"), event("r-1")]);
        assert_eq!(s.pending_len("u1"), QUEUE_CAP_PER_USER);
        let taken = s.take_pending("u1");
        assert_eq!(taken[0].session_id, "q-0");
        assert_eq!(
            taken.last().unwrap().session_id,
            format!("q-{}", QUEUE_CAP_PER_USER - 1)
        );
    }

    #[test]
    fn users_with_pending_lists_only_nonempty() {
        let s = MnemoTokenStore::new();
        s.enqueue("u1", event("a"));
        s.enqueue("u2", event("b"));
        let _ = s.take_pending("u2"); // u2's queue is now empty
        let users = s.users_with_pending();
        assert_eq!(users, vec!["u1".to_string()]);
    }
}
