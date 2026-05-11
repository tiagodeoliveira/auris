//! Background task that subscribes to the WS event broadcast and pushes
//! relevant events to mnemo.
//!
//! Lifecycle, per meeting:
//!   - `MeetingStateChanged { Active }`: start a new mnemo session (uuid),
//!     reset transcript-pushed count and per-mode item cache. Metadata
//!     cache is intentionally preserved so a pre-meeting `ExtractMetadata`
//!     run carries through.
//!   - `MetadataChanged`: refresh the metadata cache. Applied even outside
//!     active meetings (Extract-before-Start populates this).
//!   - `ItemsUpdate { mode: "transcript", items }`: push each new item
//!     (since the last push) as a `user`-role turn. Each push is a
//!     spawned tokio task so a slow HTTP call doesn't stall the loop.
//!   - `ItemsUpdate { mode: <other>, items }`: cache for the end-of-meeting
//!     summary push. Replace strategy mirrors how mnemo will see the
//!     final state.
//!   - `MeetingStateChanged { Idle }`: push one `assistant`-role event
//!     bundling actions / highlights / open-questions, then reset.
//!   - `MeetingStateChanged { Paused }`: no-op; keep accumulating.
//!
//! All HTTP calls are best-effort. Failure logs at warn but never aborts
//! the loop.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::contract::{Event, Item, MeetingState, UserEvent};

use super::client::MnemoClient;
use super::payload::{
    build_chat_event, build_moment_event, build_sentence_event, build_summary_event,
};

#[derive(Debug, Default)]
struct PusherState {
    session_id: Option<String>,
    started_at: Option<DateTime<Utc>>,
    /// Server-assigned meeting id from `MeetingStateChanged{Active}`.
    /// Carried into every push as `attributes.meeting_id` so mnemo
    /// recall can scope by meeting.
    meeting_id: Option<String>,
    /// Number of transcript items already pushed in this session.
    transcript_pushed: usize,
    /// Per-mode item cache (excluding transcript). Populated as
    /// `ItemsUpdate` events fire so the end-of-meeting summary can be
    /// built without re-querying state.
    items_by_mode: HashMap<String, Vec<Item>>,
    /// Latest metadata snapshot. Survives across the
    /// `ExtractMetadata` → `start_meeting` boundary.
    metadata: HashMap<String, String>,
}

pub fn spawn(client: MnemoClient, events_rx: broadcast::Receiver<UserEvent>) {
    if !client.is_enabled() {
        info!("mnemo pusher not spawning — client disabled");
        return;
    }
    tokio::spawn(async move { pusher_loop(client, events_rx).await });
}

async fn pusher_loop(client: MnemoClient, mut rx: broadcast::Receiver<UserEvent>) {
    // Per-user pusher state: each user's session_id, transcript
    // counter, item cache, and metadata snapshot are tracked
    // independently so concurrent meetings don't co-mingle.
    let mut per_user: HashMap<String, PusherState> = HashMap::new();
    info!("mnemo pusher started");
    loop {
        match rx.recv().await {
            Ok(envelope) => {
                let entry = per_user.entry(envelope.user_id.clone()).or_default();
                handle_event(&client, entry, envelope.event).await
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(lagged = n, "mnemo pusher fell behind broadcast channel");
            }
            Err(broadcast::error::RecvError::Closed) => {
                debug!("mnemo pusher: broadcast closed, exiting");
                return;
            }
        }
    }
}

async fn handle_event(client: &MnemoClient, state: &mut PusherState, event: Event) {
    match event {
        Event::MeetingStateChanged {
            meeting_state: MeetingState::Active,
            meeting_id,
        } => {
            if state.session_id.is_none() {
                let id = Uuid::new_v4().to_string();
                info!(session_id = %id, "mnemo: new meeting session");
                state.session_id = Some(id);
                state.started_at = Some(Utc::now());
                state.meeting_id = meeting_id;
                state.transcript_pushed = 0;
                state.items_by_mode.clear();
                // Keep metadata cache — ExtractMetadata may have populated it.
            }
        }
        Event::MeetingStateChanged {
            meeting_state: MeetingState::Idle,
            ..
        } => {
            flush_summary(client, state).await;
            *state = PusherState::default();
        }
        Event::MeetingStateChanged {
            meeting_state: MeetingState::Paused,
            ..
        } => {
            // pause is a transient signal; nothing to do.
        }
        Event::MetadataChanged { metadata } => {
            state.metadata = metadata;
        }
        Event::ItemsUpdate { mode, items } => {
            if mode == "transcript" {
                push_new_transcript(client, state, &items);
            } else if mode == "chat" {
                push_chat_pair(client, state, &items);
                // Don't cache chat in items_by_mode — chat exchanges
                // stream to mnemo per fire (above), and the
                // end-of-meeting summary bundle intentionally
                // omits chat (covered by the per-fire pushes).
            } else if mode == "summary" {
                // Summary is the running 3-5 sentence overview that
                // refreshes on a heartbeat; pushing every refresh
                // would spam mnemo with progressively-larger versions
                // of the same content, and the actions/highlights/
                // open_questions bundle at meeting stop already
                // covers the same ground. Skip entirely.
            } else {
                state.items_by_mode.insert(mode, items);
            }
        }
        Event::MomentSummarized {
            t_ms,
            summary,
            note,
            ..
        } => {
            push_moment(client, state, t_ms, &summary, note.as_deref());
        }
        // No-ops for memory purposes.
        Event::Snapshot { .. }
        | Event::ModeChanged { .. }
        | Event::DisplayTagChanged { .. }
        | Event::PriorContextChanged { .. }
        | Event::TranscriptInterim { .. }
        | Event::Status { .. }
        | Event::Error { .. }
        | Event::DeviceRegistered { .. }
        | Event::DevicesChanged { .. }
        | Event::AudioSourceDeviceChanged { .. }
        | Event::CaptureMomentScreenshot { .. }
        | Event::ArtifactsChanged { .. }
        | Event::ItemUpdated { .. } => {}
    }
}

/// Chat exchange → one mnemo event with two turns. Fires per chat
/// round-trip. The Q+A pair arrives in `items` as
/// `[user_item, assistant_item]` (the chat fire's broadcast shape).
fn push_chat_pair(client: &MnemoClient, state: &PusherState, items: &[Item]) {
    let (Some(session_id), Some(started_at)) = (state.session_id.as_deref(), state.started_at)
    else {
        return;
    };
    if items.len() < 2 {
        return;
    }
    let question = &items[0].text;
    let answer = &items[1].text;
    let payload = build_chat_event(
        session_id,
        client.workstation(),
        &state.metadata,
        started_at,
        state.meeting_id.as_deref(),
        question,
        answer,
    );
    let client = client.clone();
    tokio::spawn(async move {
        if let Err(e) = client.push_event(&payload).await {
            warn!(error = %e, "mnemo: chat push failed");
        }
    });
}

/// Moment summary → one assistant-role turn carrying the LLM's
/// transcript-window + screenshot synthesis. Screenshot itself is
/// not sent (mnemo is text-only).
fn push_moment(
    client: &MnemoClient,
    state: &PusherState,
    t_ms: i64,
    summary: &str,
    note: Option<&str>,
) {
    let (Some(session_id), Some(started_at)) = (state.session_id.as_deref(), state.started_at)
    else {
        return;
    };
    let payload = build_moment_event(
        session_id,
        client.workstation(),
        &state.metadata,
        started_at,
        state.meeting_id.as_deref(),
        t_ms,
        summary,
        note,
    );
    let client = client.clone();
    tokio::spawn(async move {
        if let Err(e) = client.push_event(&payload).await {
            warn!(error = %e, "mnemo: moment push failed");
        }
    });
}

/// Format a transcript Item's content for mnemo, prefixing the
/// speaker tag when Soniox identified one. mnemo only sees strings,
/// so we inline the speaker as `"[Speaker N] <text>"` rather than
/// pushing it as a separate attribute — that way recall composes
/// memories back into the agent's context with the speaker
/// attribution intact. Items without a `meta.speaker` field
/// pass through unchanged.
fn format_transcript_content(item: &Item) -> String {
    let speaker = item
        .meta
        .as_ref()
        .and_then(|m| m.get("speaker"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    match speaker {
        Some(s) => format!("[Speaker {s}] {}", item.text),
        None => item.text.clone(),
    }
}

fn push_new_transcript(client: &MnemoClient, state: &mut PusherState, items: &[Item]) {
    let (Some(session_id), Some(started_at)) = (state.session_id.as_deref(), state.started_at)
    else {
        // Transcript items can arrive before MeetingStateChanged{Active}
        // in theory; ignore them rather than push without a session.
        return;
    };
    if state.transcript_pushed >= items.len() {
        return;
    }
    let workstation = client.workstation().to_string();
    let metadata = state.metadata.clone();
    let session_id = session_id.to_string();
    let meeting_id = state.meeting_id.clone();
    for item in &items[state.transcript_pushed..] {
        let content = format_transcript_content(item);
        let payload = build_sentence_event(
            &session_id,
            &workstation,
            &metadata,
            started_at,
            meeting_id.as_deref(),
            &content,
        );
        let client = client.clone();
        tokio::spawn(async move {
            if let Err(e) = client.push_event(&payload).await {
                warn!(error = %e, "mnemo: sentence push failed");
            }
        });
    }
    state.transcript_pushed = items.len();
}

async fn flush_summary(client: &MnemoClient, state: &PusherState) {
    let (Some(session_id), Some(started_at)) = (state.session_id.as_deref(), state.started_at)
    else {
        return;
    };
    // `flush_summary` fires from the MeetingStateChanged → Idle
    // handler, so `now` IS the meeting end time. Captured here
    // rather than threaded through the broader state so it's
    // unambiguously the stop-time wall clock.
    let meeting_ended = Utc::now();
    let Some(payload) = build_summary_event(
        session_id,
        client.workstation(),
        &state.metadata,
        started_at,
        state.meeting_id.as_deref(),
        Some(meeting_ended),
        &state.items_by_mode,
    ) else {
        debug!(session_id = %session_id, "mnemo: nothing to summarize, skipping final push");
        return;
    };
    if let Err(e) = client.push_event(&payload).await {
        warn!(error = %e, "mnemo: final summary push failed");
    } else {
        info!(session_id = %session_id, "mnemo: final summary pushed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::Item;

    fn item(id: &str, text: &str) -> Item {
        Item {
            id: id.into(),
            text: text.into(),
            detail: None,
            t: 0,
            meta: None,
        }
    }

    #[test]
    fn format_transcript_keeps_text_when_no_speaker_meta() {
        let it = item("i1", "We talked about the demo.");
        assert_eq!(format_transcript_content(&it), "We talked about the demo.");
    }

    #[test]
    fn format_transcript_prefixes_speaker_when_present() {
        let mut it = item("i1", "We talked about the demo.");
        it.meta = Some(serde_json::json!({"speaker": "2"}));
        assert_eq!(
            format_transcript_content(&it),
            "[Speaker 2] We talked about the demo.",
        );
    }

    #[test]
    fn format_transcript_ignores_non_string_speaker() {
        // Defensive: Soniox emits strings, but if upstream ever
        // produces a number/bool/null, fall through to plain text
        // instead of crashing or stringifying with quotes around.
        let mut it = item("i1", "hello");
        it.meta = Some(serde_json::json!({"speaker": 7}));
        assert_eq!(format_transcript_content(&it), "hello");
    }

    #[test]
    fn format_transcript_ignores_empty_speaker_string() {
        let mut it = item("i1", "hello");
        it.meta = Some(serde_json::json!({"speaker": ""}));
        assert_eq!(format_transcript_content(&it), "hello");
    }

    fn meta(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn run<F: std::future::Future<Output = ()>>(f: F) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f);
    }

    /// Smoke test: a full meeting lifecycle drives the state correctly,
    /// even when the client is disabled (push_event is a no-op).
    #[test]
    fn lifecycle_drives_state_correctly() {
        run(async {
            let client = MnemoClient::Disabled;
            let mut state = PusherState::default();

            // Pre-meeting metadata extraction.
            handle_event(
                &client,
                &mut state,
                Event::MetadataChanged {
                    metadata: meta(&[("project", "helix")]),
                },
            )
            .await;
            assert_eq!(state.metadata.get("project"), Some(&"helix".to_string()));
            assert!(state.session_id.is_none());

            // Meeting starts.
            handle_event(
                &client,
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Active,
                    meeting_id: None,
                },
            )
            .await;
            assert!(state.session_id.is_some());
            assert!(state.started_at.is_some());
            // Metadata survived.
            assert_eq!(state.metadata.get("project"), Some(&"helix".to_string()));

            // Server confirms metadata after start.
            handle_event(
                &client,
                &mut state,
                Event::MetadataChanged {
                    metadata: meta(&[("project", "helix"), ("title", "demo")]),
                },
            )
            .await;
            assert_eq!(state.metadata.len(), 2);

            // First three transcript items.
            handle_event(
                &client,
                &mut state,
                Event::ItemsUpdate {
                    mode: "transcript".into(),
                    items: vec![item("t1", "first."), item("t2", "second.")],
                },
            )
            .await;
            assert_eq!(state.transcript_pushed, 2);

            // One more.
            handle_event(
                &client,
                &mut state,
                Event::ItemsUpdate {
                    mode: "transcript".into(),
                    items: vec![
                        item("t1", "first."),
                        item("t2", "second."),
                        item("t3", "third."),
                    ],
                },
            )
            .await;
            assert_eq!(state.transcript_pushed, 3);

            // Other modes get cached.
            handle_event(
                &client,
                &mut state,
                Event::ItemsUpdate {
                    mode: "actions".into(),
                    items: vec![item("a1", "Send recap")],
                },
            )
            .await;
            assert_eq!(state.items_by_mode.get("actions").unwrap().len(), 1);

            // Pause: state unchanged.
            let snap_before_pause = state.transcript_pushed;
            handle_event(
                &client,
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Paused,
                    meeting_id: None,
                },
            )
            .await;
            assert_eq!(state.transcript_pushed, snap_before_pause);
            assert!(state.session_id.is_some());

            // Stop: state resets.
            handle_event(
                &client,
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Idle,
                    meeting_id: None,
                },
            )
            .await;
            assert!(state.session_id.is_none());
            assert!(state.metadata.is_empty());
            assert!(state.items_by_mode.is_empty());
            assert_eq!(state.transcript_pushed, 0);
        });
    }

    #[test]
    fn transcript_before_meeting_active_is_ignored() {
        run(async {
            let client = MnemoClient::Disabled;
            let mut state = PusherState::default();
            handle_event(
                &client,
                &mut state,
                Event::ItemsUpdate {
                    mode: "transcript".into(),
                    items: vec![item("t1", "stray.")],
                },
            )
            .await;
            assert_eq!(state.transcript_pushed, 0);
        });
    }

    #[test]
    fn second_active_after_active_is_noop() {
        // Server should never emit Active twice without an Idle in between,
        // but if it does we must not generate a fresh session_id and lose
        // the in-flight transcript count.
        run(async {
            let client = MnemoClient::Disabled;
            let mut state = PusherState::default();
            handle_event(
                &client,
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Active,
                    meeting_id: None,
                },
            )
            .await;
            let first_id = state.session_id.clone();
            handle_event(
                &client,
                &mut state,
                Event::ItemsUpdate {
                    mode: "transcript".into(),
                    items: vec![item("t1", "x.")],
                },
            )
            .await;
            handle_event(
                &client,
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Active,
                    meeting_id: None,
                },
            )
            .await;
            assert_eq!(state.session_id, first_id);
            assert_eq!(state.transcript_pushed, 1);
        });
    }
}
