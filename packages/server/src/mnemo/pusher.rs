//! Mnemo push handling — `handle_event` + `PusherState` are driven by
//! the durable-writer task (`storage::persistence_loop`), which feeds
//! every durable event in FIFO order. There is no subscription loop
//! here anymore: riding the lossy client broadcast meant a Lagged ring
//! silently skipped transcript pushes and `MeetingFinalized` resets.
//! Nothing else changed — the per-meeting lifecycle below still holds.
//!
//! Lifecycle, per meeting:
//!   - `MeetingStateChanged { Active }`: start a new mnemo session (uuid),
//!     reset transcript-pushed count. Metadata cache is intentionally
//!     preserved so a `MetadataChanged` event that arrives slightly after
//!     `start_meeting` still gets attached to the same mnemo session.
//!   - `MetadataChanged`: refresh the metadata cache. Applied even outside
//!     active meetings (server-side auto-extract on start_meeting populates
//!     this asynchronously).
//!   - `ItemsUpdate { mode: "transcript", items }`: push each new item
//!     (since the last push) as a `user`-role turn. Each push is a
//!     spawned tokio task so a slow HTTP call doesn't stall the loop.
//!   - `MeetingStateChanged { Idle }`: NO reset — the detached finalize
//!     task is still draining the STT; the session must stay open so the
//!     drained tail (delivered as `TranscriptTail`) can push to it.
//!   - `TranscriptTail { meeting_id, items }` (server-internal, from
//!     finalize): the drained post-stop transcript tail. Pushed like
//!     live transcript items, but only when `meeting_id` matches the
//!     tracked session — a back-to-back meeting started during the
//!     drain is tracked instead, and the old tail is skipped (logged).
//!   - `MeetingFinalized { meeting_id }`: reset state (session_id, meeting,
//!     metadata) — but only when `meeting_id` matches the tracked session.
//!
//! All other event kinds (chat fires, highlight/action/open-question tool
//! emissions, moment summaries, etc.) are deliberately ignored. Pushing
//! them was muddying meeting recall with agent-summarized content; the
//! transcript stream is the only ground-truth signal mnemo gets.
//!
//! All HTTP calls are best-effort. Failure logs at warn but never aborts
//! the loop.

use std::collections::HashMap;

use tracing::{debug, info};
use uuid::Uuid;

use crate::protocol::{Event, Item, MeetingState};

use super::client::MnemoClient;
use super::payload::{build_meeting_ended_event, build_sentence_event};

#[derive(Debug, Default)]
pub(crate) struct PusherState {
    session_id: Option<String>,
    /// Server-assigned meeting id from `MeetingStateChanged{Active}`.
    /// Carried into every push as `attributes.meeting_id` so mnemo
    /// recall can scope by meeting.
    meeting_id: Option<String>,
    /// Latest metadata snapshot. Survives across the gap between
    /// `start_meeting` and the (async) `MetadataChanged` that lands
    /// when the LLM auto-extraction finishes.
    metadata: HashMap<String, String>,
    /// Count of transcript items pushed this session — live
    /// `ItemsUpdate{transcript}` items plus the finalize
    /// `TranscriptTail`. Observability + test hook; reset with the
    /// rest of the state on `MeetingFinalized`.
    transcript_items_pushed: usize,
}

pub(crate) async fn handle_event(
    client: &MnemoClient,
    user_id: &str,
    state: &mut PusherState,
    event: Event,
) {
    match event {
        Event::MeetingStateChanged {
            meeting_state: MeetingState::Active,
            meeting_id,
        } => {
            // Open a fresh session when there's none yet OR this is a
            // DIFFERENT meeting than the one we're tracking. The second
            // case matters now that Idle no longer resets: a back-to-back
            // meeting started before the prior meeting's `MeetingFinalized`
            // arrives must NOT inherit the stale session_id (that would
            // misroute its transcript and then get wiped when the old
            // finalize lands). A repeated Active for the SAME meeting stays
            // a no-op so we don't churn the session mid-meeting.
            let new_meeting = state.meeting_id != meeting_id;
            if state.session_id.is_none() || new_meeting {
                let id = Uuid::new_v4().to_string();
                info!(session_id = %id, ?meeting_id, "mnemo: new meeting session");
                state.session_id = Some(id);
                state.meeting_id = meeting_id;
                // Keep metadata cache — a MetadataChanged event from
                // server-side auto-extraction may have pre-populated it for
                // THIS meeting just before Active.
            }
        }
        Event::MeetingStateChanged {
            meeting_state: MeetingState::Idle,
            ..
        } => {
            // Do NOT reset here. The meeting flips to Idle instantly on
            // stop, but the detached finalize task is still draining the
            // STT — it broadcasts the drained tail as `TranscriptTail`
            // BEFORE `MeetingFinalized`, and that tail must still push
            // to this session. Reset happens on `MeetingFinalized`.
            debug!(
                user_id,
                session_id = ?state.session_id,
                "mnemo: meeting idle (keeping session alive for drain)"
            );
        }
        Event::MeetingFinalized { meeting_id } => {
            // Finalize finished the offline pass; the drained tail has
            // been pushed. Reset — but ONLY if this finalize is for the
            // session we're currently tracking, so a late finalize for an
            // old meeting can't wipe a newer session started during the
            // drain.
            if state.meeting_id.as_deref() == Some(meeting_id.as_str()) {
                info!(
                    user_id,
                    %meeting_id,
                    prev_session_id = ?state.session_id,
                    "mnemo: meeting finalized, resetting pusher state"
                );
                // Signal mnemo the meeting ended so it enqueues finalize_meeting
                // (writes the summary into the `meeting` dimension). Best-effort,
                // like every other push. Must happen BEFORE the reset below, which
                // clears session_id/meeting_id/metadata.
                if let Some(session_id) = state.session_id.clone() {
                    let payload = build_meeting_ended_event(
                        &session_id,
                        client.workstation(),
                        &state.metadata,
                        &meeting_id,
                        chrono::Utc::now(),
                    );
                    let client = client.clone();
                    let user_id = user_id.to_string();
                    tokio::spawn(async move {
                        client.push_event_or_queue(&user_id, &payload).await;
                    });
                }
                *state = PusherState::default();
            } else {
                debug!(
                    user_id,
                    %meeting_id,
                    current = ?state.meeting_id,
                    "mnemo: finalize for non-current meeting, ignoring"
                );
            }
        }
        Event::MetadataChanged { metadata } => {
            state.metadata = metadata;
        }
        Event::ItemsUpdate { mode, items } => {
            // Only transcript items push to mnemo. Chat, highlights,
            // actions, open_questions, summary — all derived/agent
            // content — are intentionally ignored so recall stays
            // anchored to ground-truth speech.
            if mode == "transcript" {
                info!(
                    user_id,
                    items_len = items.len(),
                    session_id = ?state.session_id,
                    "mnemo: received transcript ItemsUpdate"
                );
                push_new_transcript(client, user_id, state, &items);
            }
        }
        Event::TranscriptTail { meeting_id, items } => {
            // Server-internal: finalize's drained post-stop tail,
            // addressed by meeting id. Push only when it belongs to
            // the session we're tracking — if a back-to-back meeting
            // started during the drain we now track THAT meeting, and
            // pushing the old tail here would misroute it. (Its JSONL
            // persistence is unaffected; see storage::persistence_loop.)
            if state.meeting_id.as_deref() == Some(meeting_id.as_str()) {
                info!(
                    user_id,
                    items_len = items.len(),
                    session_id = ?state.session_id,
                    %meeting_id,
                    "mnemo: pushing drained transcript tail"
                );
                push_new_transcript(client, user_id, state, &items);
            } else {
                debug!(
                    user_id,
                    %meeting_id,
                    current = ?state.meeting_id,
                    "mnemo: transcript tail for non-current meeting, ignoring"
                );
            }
        }
        // Everything else: intentional no-op for memory purposes.
        Event::MomentSummarized { .. }
        | Event::Snapshot { .. }
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
        | Event::AttachedMeetingsChanged { .. }
        | Event::ItemUpdated { .. }
        | Event::PairCodeMinted { .. }
        | Event::PairedDevicesChanged
        | Event::AssistSensitivityChanged { .. } => {}
    }
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

fn push_new_transcript(
    client: &MnemoClient,
    user_id: &str,
    state: &mut PusherState,
    items: &[Item],
) {
    let Some(session_id) = state.session_id.as_deref() else {
        // Transcript items can arrive before MeetingStateChanged{Active}
        // in theory; ignore them rather than push without a session.
        info!(
            user_id,
            items_len = items.len(),
            "mnemo: skipping transcript push — no session_id (Active not seen yet?)"
        );
        return;
    };
    // `items` is the per-fire slice from the transcript summarizer
    // (one fresh chunk's worth, typically length 1), NOT a cumulative
    // running list. Push every entry — no high-water-mark math.
    let workstation = client.workstation().to_string();
    let metadata = state.metadata.clone();
    let session_id = session_id.to_string();
    let meeting_id = state.meeting_id.clone();
    for item in items {
        let content = format_transcript_content(item);
        let payload = build_sentence_event(
            &session_id,
            &workstation,
            &metadata,
            meeting_id.as_deref(),
            &content,
        );
        let client = client.clone();
        let user_id = user_id.to_string();
        tokio::spawn(async move {
            client.push_event_or_queue(&user_id, &payload).await;
        });
    }
    state.transcript_items_pushed += items.len();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Item;

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
                "u-test",
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
                "u-test",
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Active,
                    meeting_id: Some("m-1".into()),
                },
            )
            .await;
            assert!(state.session_id.is_some());
            // Metadata survived.
            assert_eq!(state.metadata.get("project"), Some(&"helix".to_string()));

            // Server confirms metadata after start.
            handle_event(
                &client,
                "u-test",
                &mut state,
                Event::MetadataChanged {
                    metadata: meta(&[("project", "helix"), ("title", "demo")]),
                },
            )
            .await;
            assert_eq!(state.metadata.len(), 2);

            // First per-fire chunk (transcript_summarizer emits one
            // chunk per fire, not a cumulative list).
            handle_event(
                &client,
                "u-test",
                &mut state,
                Event::ItemsUpdate {
                    mode: "transcript".into(),
                    items: vec![item("t1", "first.")],
                },
            )
            .await;

            // Next chunk arrives as its own per-fire batch.
            handle_event(
                &client,
                "u-test",
                &mut state,
                Event::ItemsUpdate {
                    mode: "transcript".into(),
                    items: vec![item("t2", "second.")],
                },
            )
            .await;

            // Other modes are ignored — only transcript turns push.
            handle_event(
                &client,
                "u-test",
                &mut state,
                Event::ItemsUpdate {
                    mode: "actions".into(),
                    items: vec![item("a1", "Send recap")],
                },
            )
            .await;

            // Stop flips to Idle but keeps the session alive for the drain.
            handle_event(
                &client,
                "u-test",
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Idle,
                    meeting_id: None,
                },
            )
            .await;
            assert!(
                state.session_id.is_some(),
                "idle keeps session for the drain"
            );
            // Finalize (matching meeting) resets.
            handle_event(
                &client,
                "u-test",
                &mut state,
                Event::MeetingFinalized {
                    meeting_id: "m-1".into(),
                },
            )
            .await;
            assert!(state.session_id.is_none());
            assert!(state.metadata.is_empty());
        });
    }

    #[test]
    fn transcript_before_meeting_active_is_ignored() {
        run(async {
            let client = MnemoClient::Disabled;
            let mut state = PusherState::default();
            handle_event(
                &client,
                "u-test",
                &mut state,
                Event::ItemsUpdate {
                    mode: "transcript".into(),
                    items: vec![item("t1", "stray.")],
                },
            )
            .await;
            // No session was ever opened, so nothing should have been
            // pushed. The pusher logs a skip warning but the state
            // itself stays untouched.
            assert!(state.session_id.is_none());
        });
    }

    #[test]
    fn idle_no_longer_resets_session() {
        run(async {
            let client = MnemoClient::Disabled;
            let mut state = PusherState::default();
            handle_event(
                &client,
                "u-test",
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Active,
                    meeting_id: Some("m-1".into()),
                },
            )
            .await;
            let sid = state.session_id.clone();
            assert!(sid.is_some());

            // Idle must NOT reset anymore — the drain tail still needs to push.
            handle_event(
                &client,
                "u-test",
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Idle,
                    meeting_id: None,
                },
            )
            .await;
            assert_eq!(state.session_id, sid, "Idle must keep the session alive");
        });
    }

    #[test]
    fn meeting_finalized_resets_only_matching_meeting() {
        run(async {
            let client = MnemoClient::Disabled;
            let mut state = PusherState::default();
            handle_event(
                &client,
                "u-test",
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Active,
                    meeting_id: Some("m-1".into()),
                },
            )
            .await;
            assert!(state.session_id.is_some());

            // A finalize for a DIFFERENT meeting must not wipe this session.
            handle_event(
                &client,
                "u-test",
                &mut state,
                Event::MeetingFinalized {
                    meeting_id: "m-OTHER".into(),
                },
            )
            .await;
            assert!(
                state.session_id.is_some(),
                "mismatched finalize must not reset"
            );

            // The matching finalize resets.
            handle_event(
                &client,
                "u-test",
                &mut state,
                Event::MeetingFinalized {
                    meeting_id: "m-1".into(),
                },
            )
            .await;
            assert!(state.session_id.is_none(), "matching finalize resets");
            assert!(state.metadata.is_empty());
        });
    }

    #[test]
    fn second_active_after_active_is_noop() {
        // Server should never emit Active twice without an Idle in
        // between, but if it does we must not generate a fresh
        // session_id mid-meeting (every subsequent push would land
        // under a new mnemo session and confuse recall).
        run(async {
            let client = MnemoClient::Disabled;
            let mut state = PusherState::default();
            handle_event(
                &client,
                "u-test",
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Active,
                    meeting_id: Some("m-1".into()),
                },
            )
            .await;
            let first_id = state.session_id.clone();
            handle_event(
                &client,
                "u-test",
                &mut state,
                Event::ItemsUpdate {
                    mode: "transcript".into(),
                    items: vec![item("t1", "x.")],
                },
            )
            .await;
            handle_event(
                &client,
                "u-test",
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Active,
                    meeting_id: Some("m-1".into()),
                },
            )
            .await;
            assert_eq!(state.session_id, first_id);
        });
    }

    #[test]
    fn back_to_back_meeting_gets_fresh_session() {
        // Now that Idle no longer resets, a meeting started before the
        // PRIOR meeting's MeetingFinalized arrives must still get its own
        // session — NOT inherit the stale one — and the late finalize for
        // the old meeting must not wipe the new session.
        run(async {
            let client = MnemoClient::Disabled;
            let mut state = PusherState::default();
            // Meeting A.
            handle_event(
                &client,
                "u",
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Active,
                    meeting_id: Some("m-A".into()),
                },
            )
            .await;
            let sid_a = state.session_id.clone();
            assert!(sid_a.is_some());

            // A stops (Idle keeps the session for the drain).
            handle_event(
                &client,
                "u",
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Idle,
                    meeting_id: None,
                },
            )
            .await;
            assert_eq!(state.session_id, sid_a);

            // B starts BEFORE A's finalize → fresh session, not A's.
            handle_event(
                &client,
                "u",
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Active,
                    meeting_id: Some("m-B".into()),
                },
            )
            .await;
            let sid_b = state.session_id.clone();
            assert!(sid_b.is_some());
            assert_ne!(sid_b, sid_a, "B must get a fresh session, not inherit A's");
            assert_eq!(state.meeting_id.as_deref(), Some("m-B"));

            // A's late finalize must NOT touch B.
            handle_event(
                &client,
                "u",
                &mut state,
                Event::MeetingFinalized {
                    meeting_id: "m-A".into(),
                },
            )
            .await;
            assert_eq!(state.session_id, sid_b, "A's late finalize must not wipe B");

            // B's own finalize resets.
            handle_event(
                &client,
                "u",
                &mut state,
                Event::MeetingFinalized {
                    meeting_id: "m-B".into(),
                },
            )
            .await;
            assert!(state.session_id.is_none());
        });
    }

    /// Regression (improvement #19): finalize broadcasts the drained
    /// post-stop tail as `TranscriptTail` BEFORE `MeetingFinalized`.
    /// The pusher must push those items to the still-open session —
    /// this is the whole reason Idle does not reset the session.
    #[test]
    fn transcript_tail_pushes_to_matching_session_before_finalize_reset() {
        run(async {
            let client = MnemoClient::Disabled;
            let mut state = PusherState::default();
            handle_event(
                &client,
                "u",
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Active,
                    meeting_id: Some("m-1".into()),
                },
            )
            .await;
            // One live transcript item during the meeting.
            handle_event(
                &client,
                "u",
                &mut state,
                Event::ItemsUpdate {
                    mode: "transcript".into(),
                    items: vec![item("t1", "first.")],
                },
            )
            .await;
            assert_eq!(state.transcript_items_pushed, 1);

            // Stop → Idle keeps the session open for the drain.
            handle_event(
                &client,
                "u",
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Idle,
                    meeting_id: None,
                },
            )
            .await;

            // Finalize delivers the drained tail for the SAME meeting.
            handle_event(
                &client,
                "u",
                &mut state,
                Event::TranscriptTail {
                    meeting_id: "m-1".into(),
                    items: vec![item("t2", "second."), item("t3", "third.")],
                },
            )
            .await;
            assert_eq!(
                state.transcript_items_pushed, 3,
                "tail items must push to the still-open session"
            );

            // Finalize then resets everything, counter included.
            handle_event(
                &client,
                "u",
                &mut state,
                Event::MeetingFinalized {
                    meeting_id: "m-1".into(),
                },
            )
            .await;
            assert!(state.session_id.is_none());
            assert_eq!(state.transcript_items_pushed, 0);
        });
    }

    /// Back-to-back: meeting B started during A's drain means the
    /// pusher is already tracking B when A's tail arrives. A's tail
    /// must be skipped — pushing it would misroute A's closing words
    /// into B's mnemo session. (The JSONL is still correct: the
    /// persistence loop addresses by the event's meeting_id.)
    #[test]
    fn transcript_tail_for_non_current_meeting_is_ignored() {
        run(async {
            let client = MnemoClient::Disabled;
            let mut state = PusherState::default();
            // Meeting A runs and stops.
            handle_event(
                &client,
                "u",
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Active,
                    meeting_id: Some("m-A".into()),
                },
            )
            .await;
            handle_event(
                &client,
                "u",
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Idle,
                    meeting_id: None,
                },
            )
            .await;
            // Meeting B starts before A's finalize lands.
            handle_event(
                &client,
                "u",
                &mut state,
                Event::MeetingStateChanged {
                    meeting_state: MeetingState::Active,
                    meeting_id: Some("m-B".into()),
                },
            )
            .await;
            // A's late drained tail must NOT push into B's session.
            handle_event(
                &client,
                "u",
                &mut state,
                Event::TranscriptTail {
                    meeting_id: "m-A".into(),
                    items: vec![item("t9", "tail of A")],
                },
            )
            .await;
            assert_eq!(
                state.transcript_items_pushed, 0,
                "A's tail must not push into B's session"
            );
            assert_eq!(state.meeting_id.as_deref(), Some("m-B"));
        });
    }
}
