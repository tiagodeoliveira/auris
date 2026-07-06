//! mnemo memory integration.
//!
//! mnemo is a Postgres-backed memory service. We push exactly one
//! kind of event: `user`-role turns carrying consolidated transcript
//! sentences, streamed as the meeting progresses. Agent-summarized
//! content (chat exchanges, highlights, actions, open questions, moment
//! summaries) is intentionally not pushed — recall stays anchored to
//! ground-truth speech rather than progressively-rephrased derivatives.
//!
//! Per-user JWT pass-through model: auris and mnemo share one Auth0
//! API audience (kleos), so the same JWT the UI uses to authenticate
//! against auris also authenticates against mnemo. Auris caches the
//! validated token at WS handshake; the UI can refresh it mid-session
//! via `Intent::SetAuthToken`. Mnemo extracts the actor from the
//! JWT's sub claim — attribution is end-to-end attested by Auth0 and
//! a compromised auris can only impersonate users whose tokens it
//! currently holds. If `AURIS_MNEMO_URL` is unset the integration is
//! silently disabled (good for dev and tests).

pub mod client;
pub mod payload;
pub mod pusher;
pub mod recall;
pub mod recaller;
pub mod token_store;

pub use client::MnemoClient;
pub use payload::{build_sentence_event, IngestEvent, IngestRequest, Turn, TurnRole};
pub use recall::{RecallParams, RecalledContext, RecalledDimension, RecalledItem};
pub use token_store::MnemoTokenStore;

use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;

use crate::protocol::UserEvent;
use crate::session::SessionRegistry;

/// How often the background drain retries queued pushes. 30s balances
/// recovery latency against hammering a mnemo that is still down, and
/// matches the push breaker's cooldown so the first tick after an
/// outage is eligible to be the breaker's half-open probe.
const PENDING_DRAIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Spin up the start-of-meeting recaller (on the client fan-out lane)
/// and the periodic pending-queue drain. The mnemo pusher no longer
/// runs here — it moved into the durable writer
/// (`storage::persistence_loop::spawn_durable_writer`), which feeds it
/// every durable event in FIFO order. The recaller stays on the lossy
/// fan-out: a missed recall degrades one prompt's prior context, it is
/// not data loss. No-op when the client is disabled.
pub fn spawn_recaller_and_drain(
    client: MnemoClient,
    state: Arc<Mutex<SessionRegistry>>,
    fanout: &broadcast::Sender<UserEvent>,
    shutdown: CancellationToken,
) {
    if !client.is_enabled() {
        tracing::info!("mnemo recaller/drain not spawning — client disabled");
        return;
    }
    recaller::spawn(client.clone(), state, fanout.clone(), fanout.subscribe());
    spawn_pending_drain(client, shutdown);
}

/// One pass over every user with queued events. Pulled out of the
/// spawn loop so tests drive it directly instead of waiting on the
/// interval. Deliberately does NOT pre-check the circuit breaker —
/// `CircuitBreaker::allow()` is side-effecting (it claims the
/// half-open probe slot), so each user's first real `push_event`
/// inside `drain_pending` acts as the probe; a `CircuitOpen` result
/// stops that user's drain at zero HTTP cost.
pub async fn drain_all_pending(client: &MnemoClient) {
    let Some(tokens) = client.tokens() else {
        return;
    };
    for user_id in tokens.users_with_pending() {
        client.drain_pending(&user_id).await;
    }
}

/// Periodic retry loop for the transient-failure queue. Same
/// interval+shutdown shape as `ws::control::spawn_liveness_reaper`.
fn spawn_pending_drain(client: MnemoClient, shutdown: CancellationToken) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(PENDING_DRAIN_INTERVAL);
        interval.tick().await; // skip the immediate first tick
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = interval.tick() => {}
            }
            drain_all_pending(&client).await;
        }
    });
}
