//! Shared application context: the cheaply-cloneable `ServerHandle`
//! threaded through every WS handler, background worker, and API
//! handler; the two-lane `EventBus`; and the user-event broadcast
//! helper. Lives at the crate top level so subsystems (workers,
//! agent, stt, mnemo, api) depend on `context` — never on the `ws`
//! transport module.

pub mod bus;

pub use bus::EventBus;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::llm::LlmClient;

/// Per-connection event mailbox. Looked up by `connection_id` to
/// deliver targeted events without leaking them to every client.
pub type DirectMailbox = tokio::sync::mpsc::Sender<crate::protocol::Event>;

/// Registry of live control connections keyed by `connection_id`.
/// Populated on accept, removed on disconnect.
pub type DirectRegistry = Arc<StdMutex<HashMap<String, DirectMailbox>>>;

#[derive(Clone)]
pub struct ServerHandle {
    pub sessions: Arc<Mutex<crate::session::SessionRegistry>>,
    /// Two-lane event bus: `bus.emit(...)` for everything durable
    /// (awaited; backpressured into the durable-writer task),
    /// `bus.emit_fanout_only(...)` for heartbeats/partials,
    /// `bus.fanout.subscribe()` for per-connection forward loops.
    pub bus: EventBus,
    /// Per-connection senders for targeted events
    /// (currently `Event::CaptureMomentScreenshot`). The `bus`
    /// fan-out still handles everything-to-everyone traffic.
    pub direct_tx: DirectRegistry,
    /// Auth mode chosen at boot. `Arc` so it's cheap to clone into
    /// `ApiState` without paying for the `Disabled` enum variant.
    pub auth: Arc<crate::auth::AuthMode>,
    pub shutdown: CancellationToken,
    pub chat_llm: Arc<LlmClient>,
    pub background_llm: Arc<LlmClient>,
    /// PostgreSQL pool for meeting / moment persistence. See `db` module.
    /// Single connection pool is fine — the access pattern is
    /// "occasional small writes from intent handlers"; we're
    /// nowhere near needing read replicas or sharding.
    pub db: sqlx::PgPool,
    /// Internal broadcast: each moment created via the REST POST is
    /// published here. The async summary worker (spawned at boot)
    /// subscribes; nothing else does today. Held so api.rs can
    /// receive a clone via `ApiState`.
    pub moment_created_tx: broadcast::Sender<crate::api::MomentCreated>,
    /// Mirror channel for artifact uploads (`POST /artifacts`).
    /// Subscribed by the artifact-summary worker.
    pub artifact_created_tx: broadcast::Sender<crate::api::ArtifactCreated>,
    /// Mirror channel for wrap-up retries
    /// (`POST /meetings/:id/retry-wrap-up`). Subscribed by the
    /// wrap-up retry worker, which re-runs the extractor off the
    /// persisted transcript.
    pub wrap_up_retry_tx: broadcast::Sender<crate::api::WrapUpRetry>,
    /// Kick the agent loop into firing immediately for a specific
    /// user. Sent by API handlers when something happens that the
    /// agent should react to without waiting for the next
    /// transcript-driven trigger (today: artifact attach to a
    /// running meeting). Subscribed by the agent task.
    pub agent_kick_tx: broadcast::Sender<crate::agent::AgentKick>,
    /// Mnemo client (shared with the pusher + recaller). Held on the
    /// handle so the agent's per-meeting fetch tools can recall
    /// scoped to a specific attached meeting's `meeting_id`. Stays
    /// `Disabled` (no-op) when `AURIS_MNEMO_*` env vars
    /// are unset.
    pub mnemo: crate::mnemo::MnemoClient,
    /// Tracker for detached-but-FINITE background tasks — today the
    /// post-stop finalize (`workers::finalize::run`) and the per-retry
    /// wrap-up fan-outs. The shutdown sequence closes the tracker and
    /// waits (bounded by `AURIS_SHUTDOWN_GRACE_MS`) so a redeploy
    /// seconds after StopMeeting doesn't kill the summary mid-flight.
    /// Do NOT spawn infinite-loop workers (persistence loops, mnemo,
    /// heartbeat) on this — `wait()` would never return. Clones share
    /// the same underlying tracker.
    pub tasks: TaskTracker,
}

/// Send a `UserEvent` on the broadcast bus.
///
/// `broadcast::Sender::send` returns `Err(SendError)` only when no
/// receivers are alive — at shutdown, or after every subscriber has
/// dropped. Both are non-fatal: we lose the event but the channel
/// stays usable for the next send. Lagged subscribers handle their
/// own catch-up via `recv()` returning `RecvError::Lagged`.
///
/// Centralising the "fire and forget" here makes the failure
/// observable (debug-level so production logs aren't noisy) without
/// scattering `let _ =` across 25+ call sites.
pub fn broadcast_user_event(
    tx: &broadcast::Sender<crate::protocol::UserEvent>,
    user_id: impl Into<String>,
    event: crate::protocol::Event,
) {
    if let Err(err) = tx.send(crate::protocol::UserEvent::new(user_id.into(), event)) {
        tracing::debug!(error = %err, "broadcast: no live subscribers");
    }
}
