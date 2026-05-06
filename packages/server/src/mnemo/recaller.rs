//! Background task that fires one mnemo `/recall` call when a meeting
//! goes Active, and stores the result on `ServerState` so the LLM
//! summarizers can include it as prior context in their prompts.
//!
//! Lifecycle:
//!   - On `MeetingStateChanged { Active }`: read current metadata under
//!     the state lock, build params (facts + preferences + episodes,
//!     plus `project` if metadata has one), spawn the recall, and on
//!     success write the result back under the lock.
//!   - On `MeetingStateChanged { Idle }`: clear the recalled context so
//!     the next meeting starts fresh.
//!
//! Failures log at warn and leave `recalled_context = None`; summarizers
//! degrade to "no prior context" prompts unchanged.

use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};
use tracing::{debug, info, warn};

use crate::contract::{Event, MeetingState, PriorContextSummary, UserEvent};
use crate::state::ServerState;

use super::client::MnemoClient;
use super::recall::RecallParams;

pub fn spawn(
    client: MnemoClient,
    state: Arc<Mutex<ServerState>>,
    events_tx: broadcast::Sender<UserEvent>,
    events_rx: broadcast::Receiver<UserEvent>,
) {
    if !client.is_enabled() {
        return;
    }
    tokio::spawn(async move { recaller_loop(client, state, events_tx, events_rx).await });
}

/// Per-user local state. The recaller is one global task but it
/// keeps a small map keyed by user_id so each user's "did I already
/// recall for this project?" check stays independent.
#[derive(Default)]
struct RecallerLocal {
    meeting_active: bool,
    /// Project we last recalled with. `None` = recalled with no project.
    last_recalled_project: Option<String>,
}

async fn recaller_loop(
    client: MnemoClient,
    state: Arc<Mutex<ServerState>>,
    events_tx: broadcast::Sender<UserEvent>,
    mut rx: broadcast::Receiver<UserEvent>,
) {
    info!("mnemo recaller started");
    let mut per_user: std::collections::HashMap<String, RecallerLocal> =
        std::collections::HashMap::new();
    loop {
        match rx.recv().await {
            Ok(envelope) => {
                let uid = envelope.user_id.clone();
                let local = per_user.entry(uid.clone()).or_default();
                match envelope.event {
                    Event::MeetingStateChanged {
                        meeting_state: MeetingState::Active,
                        ..
                    } => {
                        local.meeting_active = true;
                        let project = read_project(&state, &uid).await;
                        local.last_recalled_project = project.clone();
                        trigger_recall(&client, &state, &events_tx, &uid, project);
                    }
                    Event::MeetingStateChanged {
                        meeting_state: MeetingState::Idle,
                        ..
                    } => {
                        per_user.insert(uid.clone(), RecallerLocal::default());
                        state.lock().await.user_mut(&uid).set_recalled_context(None);
                        let _ = events_tx.send(UserEvent::new(
                            uid,
                            Event::PriorContextChanged {
                                summary: PriorContextSummary::default(),
                            },
                        ));
                    }
                    Event::MetadataChanged { .. } => {
                        if !local.meeting_active {
                            continue;
                        }
                        let project = read_project(&state, &uid).await;
                        if project == local.last_recalled_project {
                            continue;
                        }
                        debug!(
                            user_id = %uid,
                            old = ?local.last_recalled_project,
                            new = ?project,
                            "mnemo: project changed mid-meeting, re-recalling"
                        );
                        local.last_recalled_project = project.clone();
                        trigger_recall(&client, &state, &events_tx, &uid, project);
                    }
                    _ => {}
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(lagged = n, "mnemo recaller fell behind broadcast channel");
            }
            Err(broadcast::error::RecvError::Closed) => {
                debug!("mnemo recaller: broadcast closed, exiting");
                return;
            }
        }
    }
}

async fn read_project(state: &Arc<Mutex<ServerState>>, user_id: &str) -> Option<String> {
    state
        .lock()
        .await
        .user(user_id)
        .map(|u| u.metadata_clone())
        .unwrap_or_default()
        .get("project")
        .cloned()
        .filter(|s| !s.is_empty())
}

fn trigger_recall(
    client: &MnemoClient,
    state: &Arc<Mutex<ServerState>>,
    events_tx: &broadcast::Sender<UserEvent>,
    user_id: &str,
    project: Option<String>,
) {
    let params = RecallParams::for_meeting(project);
    let client = client.clone();
    let state = state.clone();
    let events_tx = events_tx.clone();
    let user_id = user_id.to_string();
    tokio::spawn(async move {
        match client.recall(&params).await {
            Ok(ctx) => {
                if ctx.is_empty() {
                    debug!(user_id = %user_id, "mnemo recall returned no records");
                } else {
                    info!(
                        user_id = %user_id,
                        preferences = ctx.preferences.len(),
                        facts = ctx.facts.len(),
                        episodes = ctx.episodes.len(),
                        has_project = ctx.project.is_some(),
                        "mnemo recall populated prior context"
                    );
                }
                let summary = ctx.summary();
                state
                    .lock()
                    .await
                    .user_mut(&user_id)
                    .set_recalled_context(Some(ctx));
                let _ = events_tx.send(UserEvent::new(
                    user_id,
                    Event::PriorContextChanged { summary },
                ));
            }
            Err(e) => {
                warn!(error = %e, "mnemo recall failed; summarizers run without prior context");
            }
        }
    });
}
