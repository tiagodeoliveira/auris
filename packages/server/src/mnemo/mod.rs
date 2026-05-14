//! mnemo memory integration.
//!
//! mnemo is a Bedrock AgentCore-backed memory layer. We push exactly one
//! kind of event: `user`-role turns carrying consolidated transcript
//! sentences, streamed as the meeting progresses. Agent-summarized
//! content (chat exchanges, highlights, actions, open questions, moment
//! summaries) is intentionally not pushed — recall stays anchored to
//! ground-truth speech rather than progressively-rephrased derivatives.
//!
//! All HTTP calls are fire-and-forget: failure logs a warning but does not
//! block the meeting flow. If `AURIS_MNEMO_URL` or
//! `AURIS_MNEMO_API_KEY` is unset, the integration is silently
//! disabled (good for dev and tests).

pub mod client;
pub mod payload;
pub mod pusher;
pub mod recall;
pub mod recaller;

pub use client::MnemoClient;
pub use payload::{build_sentence_event, IngestEvent, IngestRequest, Turn, TurnRole};
pub use recall::{RecallParams, RecalledContext, RecalledDimension, RecalledItem};

use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};

use crate::contract::UserEvent;
use crate::state::ServerState;

/// Spin up both the ingestion pusher and the start-of-meeting recaller.
/// No-op when the client is disabled.
pub fn spawn_tasks(
    client: MnemoClient,
    state: Arc<Mutex<ServerState>>,
    events_tx: &broadcast::Sender<UserEvent>,
) {
    if !client.is_enabled() {
        tracing::info!("mnemo tasks not spawning — client disabled");
        return;
    }
    pusher::spawn(client.clone(), events_tx.subscribe());
    recaller::spawn(client, state, events_tx.clone(), events_tx.subscribe());
}
