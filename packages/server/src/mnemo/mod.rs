//! mnemo memory integration.
//!
//! mnemo is a Bedrock AgentCore-backed memory layer. We push:
//!   - one `user`-role turn per consolidated transcript sentence (streaming)
//!   - one final batch of `assistant`-role turns at meeting stop, holding
//!     the LLM-extracted summaries (action items, highlights, open questions)
//!
//! Recall is not yet implemented; for now we lean on mnemo's existing
//! `facts` / `preferences` / `project` dimensions, queried via its `/recall`
//! endpoint when needed.
//!
//! All HTTP calls are fire-and-forget: failure logs a warning but does not
//! block the meeting flow. If `MEETING_COMPANION_MNEMO_URL` or
//! `MEETING_COMPANION_MNEMO_API_KEY` is unset, the integration is silently
//! disabled (good for dev and tests).

pub mod client;
pub mod payload;
pub mod pusher;
pub mod recall;
pub mod recaller;

pub use client::MnemoClient;
pub use payload::{
    build_sentence_event, build_summary_event, IngestEvent, IngestRequest, Turn, TurnRole,
};
pub use recall::{RecallParams, RecalledContext, RecalledMemory};

use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};

use crate::contract::{Event, UserEvent};
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
