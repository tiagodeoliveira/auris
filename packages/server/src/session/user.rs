//! `UserSession` — per-user runtime state (meeting, items, devices).

use super::{default_modes, MeetingRuntime, RecoveredMeeting, DEFAULT_MODE_ID};
use crate::protocol::{Item, MeetingState, ModeOption, UpdateStrategy};
use crate::stt::TranscriptChunk;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

pub struct UserSession {
    /// Exposed `pub(crate)` so `ws/mod.rs`, `ws/control.rs`, and
    /// `agent/tools/mod.rs` can read the current meeting state without
    /// a round-trip through an accessor method.
    ///
    /// Invariant: `meeting.is_some() ⟺ meeting_state == Active`.
    /// Kept as a separate field for the snapshot wire surface — the
    /// JSON shape doesn't change.
    pub(crate) meeting_state: MeetingState,
    /// Exposed `pub(crate)` so `storage/persistence_loop.rs` can look
    /// up the UpdateStrategy for a mode without a separate accessor.
    pub(crate) available_modes: Vec<ModeOption>,
    pub(super) current_mode: String,
    pub(super) items_per_mode: HashMap<String, Vec<Item>>,
    /// Exposed `pub(crate)` so `agent/bootstrap.rs` can clone it for
    /// the bootstrap section without an extra accessor.
    pub(crate) metadata: HashMap<String, String>,
    /// User-supplied freeform meeting description. Distinct from
    /// `metadata` — this is the verbatim prose the user typed at
    /// compose time, kept around so the agent's bootstrap can seed
    /// the conversation with the user's full framing (relationships,
    /// intent, expected outcomes) rather than just the LLM-extracted
    /// structured fields.
    /// Exposed `pub(crate)` so `agent/bootstrap.rs` can read it.
    pub(crate) description: Option<String>,
    /// Registered devices keyed by `connection_id` (a UUID minted per
    /// WS connection). Phase 2b is in-memory only — disconnect removes
    /// the entry; Phase 4 adds a persistent `devices` table that
    /// preserves entries across reconnects.
    pub(crate) devices_by_connection: HashMap<String, crate::protocol::Device>,
    /// Active meeting runtime. `Some` exactly while `meeting_state ==
    /// Active`. Dropping this is sufficient to release all meeting-only
    /// state (transcript buffer, recalled context, audio binding) in
    /// one move.
    pub(crate) meeting: Option<MeetingRuntime>,
    /// Cancellation token for any in-flight metadata extraction for
    /// this user. `None` when no extraction is running. Replaced with
    /// a fresh token each time extraction fires (previous is cancelled
    /// first); cleared on stop_meeting so a late LLM result doesn't
    /// land in idle metadata.
    pub(crate) extraction_cancel: Option<CancellationToken>,
}

impl UserSession {
    pub fn new() -> Self {
        let modes = default_modes();
        let items_per_mode: HashMap<String, Vec<Item>> =
            modes.iter().map(|m| (m.id.clone(), Vec::new())).collect();
        let s = Self {
            meeting_state: MeetingState::Idle,
            available_modes: modes,
            current_mode: DEFAULT_MODE_ID.to_string(),
            items_per_mode,
            metadata: HashMap::new(),
            description: None,
            devices_by_connection: HashMap::new(),
            meeting: None,
            extraction_cancel: None,
        };
        s.assert_invariants();
        s
    }

    /// Rehydrate from a meeting that was active when the server last
    /// stopped. Called once at boot, before any clients connect.
    /// Wipes any in-memory state to avoid mixing fresh and recovered
    /// fields, then re-installs the recovered ones.
    ///
    /// Note: `started_at_wall` is set to `Utc::now()` rather than
    /// the original wall-clock start — the recovered wall-clock start
    /// lives in the DB row (`meetings.started_at`), not in the runtime.
    /// See spec §4 for the rationale.
    pub fn rehydrate_from_recovered_meeting(&mut self, r: &RecoveredMeeting) {
        self.meeting_state = MeetingState::Active;
        let mut rt = MeetingRuntime::new(r.id.clone(), chrono::Utc::now());
        // Restore the persisted assist sensitivity so the recovered
        // meeting picks up the user's last choice (instead of
        // silently reverting to Moderate on every server restart).
        rt.assist_sensitivity = r.assist_sensitivity;
        self.meeting = Some(rt);
        self.metadata = r.metadata.clone();
        self.description = r.description.clone();
        self.current_mode = DEFAULT_MODE_ID.to_string();
        // Replay transcript items into transcript-mode. Other modes
        // start empty — their summarizers will re-derive items as
        // new transcript chunks come in.
        self.items_per_mode
            .insert(DEFAULT_MODE_ID.to_string(), r.transcript_items.clone());
        self.assert_invariants();
    }

    /// Locate an item by id across all modes. Returns
    /// `Some((mode_id, text))` on hit, `None` if no mode contains
    /// an item with that id. Used by the WS layer to build the
    /// `ExpandItem` agent-kick payload — the agent needs both the
    /// mode (to render the right prompt: "expand on this {mode}
    /// item") and the text itself.
    pub fn find_item_by_id(&self, item_id: &str) -> Option<(String, String)> {
        for (mode, items) in &self.items_per_mode {
            if let Some(it) = items.iter().find(|i| i.id == item_id) {
                return Some((mode.clone(), it.text.clone()));
            }
        }
        None
    }

    /// Set the `detail` field of one item identified by `(mode,
    /// item_id)`. Returns the updated `Item` (clone) on hit so the
    /// caller can broadcast `Event::ItemUpdated`. `None` if the
    /// mode's items list doesn't contain that id (race with stop,
    /// or stale id from an old meeting).
    pub fn set_item_detail(&mut self, mode: &str, item_id: &str, detail: &str) -> Option<Item> {
        let items = self.items_per_mode.get_mut(mode)?;
        let it = items.iter_mut().find(|i| i.id == item_id)?;
        it.detail = Some(detail.to_string());
        Some(it.clone())
    }

    /// Append a transcript chunk to the rolling buffer. Silently no-ops
    /// if no meeting is active.
    pub fn append_transcript_chunk(&mut self, chunk: TranscriptChunk) {
        if let Some(m) = self.meeting.as_mut() {
            m.rolling_transcript.push(chunk);
        } else {
            tracing::warn!("append_transcript_chunk called with no active meeting; dropping");
        }
    }

    /// Return the rolling transcript joined as a single string with
    /// newlines between chunks. Empty string if no meeting or no chunks.
    pub fn rolling_transcript_text(&self) -> String {
        self.meeting
            .as_ref()
            .map(|m| {
                m.rolling_transcript
                    .iter()
                    .map(|c| c.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    }

    /// Bulk replace the items for a mode. Used by `quick_asks` where
    /// the items_per_mode entry mirrors the user's DB-backed library,
    /// and any CRUD broadcast the whole new set. No 10-item cap (the
    /// application enforces a 50-quick-ask limit elsewhere).
    pub fn set_items_for_mode(&mut self, mode: &str, items: Vec<Item>) {
        if !self.available_modes.iter().any(|m| m.id == mode) {
            return;
        }
        self.items_per_mode.insert(mode.to_string(), items);
    }

    /// Append an item to the named mode's list using its declared UpdateStrategy.
    /// Returns the broadcast payload (full list for Replace, single-item Vec for Append).
    /// Empty Vec if mode is unknown or no item buffer exists.
    /// Caps Replace-strategy lists at 10 items (FIFO).
    pub fn push_item_for_mode(&mut self, mode: &str, item: Item) -> Vec<Item> {
        let strategy = match self.available_modes.iter().find(|m| m.id == mode) {
            Some(m) => m.update_strategy,
            None => return Vec::new(),
        };
        let items = match self.items_per_mode.get_mut(mode) {
            Some(v) => v,
            None => return Vec::new(),
        };
        items.push(item.clone());
        let payload = match strategy {
            UpdateStrategy::Replace => {
                while items.len() > 10 {
                    items.remove(0);
                }
                items.clone()
            }
            UpdateStrategy::Append => vec![item],
        };
        self.assert_invariants();
        payload
    }

    /// Upsert items into the named mode by id: existing items with
    /// matching ids are replaced in place; new ids are appended at
    /// the end. Mirrors the client's `applyItemsUpdate` merge-by-id
    /// behavior for Append-strategy modes. Used by the chat
    /// optimistic-pending → final-reply transition where the agent
    /// re-emits items under the ids the WS handler already published.
    pub fn merge_items_in_mode(&mut self, mode: &str, incoming: &[Item]) {
        let Some(items) = self.items_per_mode.get_mut(mode) else {
            return;
        };
        for item in incoming {
            if let Some(existing) = items.iter_mut().find(|i| i.id == item.id) {
                *existing = item.clone();
            } else {
                items.push(item.clone());
            }
        }
        self.assert_invariants();
    }

    /// Case-insensitive trim-compare check for whether a mode's
    /// bucket already contains an item with matching text. Used by
    /// `PushAssistSuggestion` as a defense-in-depth dedup guard on
    /// top of the LLM's history-awareness. Cheap: assist buckets
    /// are bounded by what the LLM has emitted this meeting.
    pub fn mode_contains_text(&self, mode: &str, text: &str) -> bool {
        let needle = text.trim().to_lowercase();
        if needle.is_empty() {
            return false;
        }
        self.items_per_mode
            .get(mode)
            .map(|items| items.iter().any(|i| i.text.trim().to_lowercase() == needle))
            .unwrap_or(false)
    }

    /// Number of items currently in the named mode's bucket; 0 for
    /// unknown modes. Used by `PushAssistSuggestion` as a per-meeting
    /// count backstop — assist is Append-strategy, so unlike the
    /// Replace modes it gets no 10-item FIFO cap in
    /// `push_item_for_mode`.
    pub fn mode_len(&self, mode: &str) -> usize {
        self.items_per_mode.get(mode).map(|v| v.len()).unwrap_or(0)
    }

    /// Replace the entire item list for the named mode. Used by Replace-strategy
    /// summarizers that re-derive the full list each cycle (highlights).
    /// Caps at 10 items (FIFO from the front of the supplied list).
    /// Empty Vec if mode is unknown.
    pub fn replace_items_for_mode(&mut self, mode: &str, new_items: Vec<Item>) -> Vec<Item> {
        if !self.available_modes.iter().any(|m| m.id == mode) {
            return Vec::new();
        }
        let mut capped = new_items;
        while capped.len() > 10 {
            capped.remove(0);
        }
        self.items_per_mode.insert(mode.to_string(), capped.clone());
        self.assert_invariants();
        capped
    }

    pub fn current_mode_id(&self) -> &str {
        &self.current_mode
    }

    /// Monotonic start time of the active meeting, or `None` if idle.
    pub fn meeting_started_at(&self) -> Option<std::time::Instant> {
        self.meeting.as_ref().map(|m| m.started_at_instant)
    }

    pub fn snapshot_meeting_state(&self) -> MeetingState {
        self.meeting_state
    }

    pub fn metadata_clone(&self) -> HashMap<String, String> {
        self.metadata.clone()
    }

    /// Snapshot the recalled context (cheap clone — `RecalledContext`
    /// holds owned strings/vecs already).
    pub fn recalled_context_clone(&self) -> Option<crate::mnemo::RecalledContext> {
        self.meeting
            .as_ref()
            .and_then(|m| m.recalled_context.clone())
    }

    pub fn set_recalled_context(&mut self, ctx: Option<crate::mnemo::RecalledContext>) {
        if let Some(m) = self.meeting.as_mut() {
            m.recalled_context = ctx;
        } else {
            tracing::warn!("set_recalled_context called with no active meeting; ignoring");
        }
    }

    pub fn set_metadata_full(&mut self, metadata: HashMap<String, String>) {
        // Idle is a valid state for metadata: extraction is spawned at
        // start_meeting time but the result may land slightly after
        // (e.g., a fast stop_meeting before the LLM returns). The
        // cancellation token in spawn_extraction handles the
        // "abandon if stopped" case explicitly, so no guard here.
        self.metadata = metadata;
    }

    pub(super) fn assert_invariants(&self) {
        debug_assert!(
            self.available_modes
                .iter()
                .any(|m| m.id == self.current_mode),
            "current_mode not in available_modes"
        );
        debug_assert_eq!(
            self.items_per_mode.len(),
            self.available_modes.len(),
            "items_per_mode must have an entry per mode"
        );
        for m in &self.available_modes {
            debug_assert!(
                self.items_per_mode.contains_key(&m.id),
                "items_per_mode missing entry for mode {}",
                m.id
            );
        }
        for (mode_id, items) in &self.items_per_mode {
            let mut seen = std::collections::HashSet::new();
            for item in items {
                debug_assert!(
                    seen.insert(&item.id),
                    "duplicate item.id '{}' in mode '{}'",
                    item.id,
                    mode_id
                );
            }
        }
        // Cross-field consistency: meeting runtime presence mirrors state enum.
        debug_assert_eq!(
            self.meeting.is_some(),
            matches!(self.meeting_state, MeetingState::Active),
            "meeting.is_some() must match meeting_state == Active"
        );
        match self.meeting_state {
            MeetingState::Idle => {
                debug_assert!(
                    self.items_per_mode.values().all(|v| v.is_empty()),
                    "items must be empty when idle"
                );
            }
            MeetingState::Active => {
                debug_assert!(
                    self.meeting.is_some(),
                    "meeting runtime must be Some when active"
                );
            }
        }
    }
}

impl Default for UserSession {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Event, Intent, Item, MeetingState, PROTOCOL_VERSION};
    use crate::session::{RecoveredMeeting, DEFAULT_MODE_ID};

    fn push_item(s: &mut UserSession, mode: &str, id: &str, text: &str) {
        s.items_per_mode.get_mut(mode).unwrap().push(Item {
            id: id.into(),
            text: text.into(),
            detail: None,
            t: 0,
            meta: None,
        });
    }

    #[test]
    fn new_has_idle_state() {
        let s = UserSession::new();
        assert!(matches!(s.meeting_state, MeetingState::Idle));
    }

    #[test]
    fn mode_contains_text_matches_case_insensitive_trim() {
        let mut s = UserSession::new();
        push_item(&mut s, "assist", "as-x", "What is RAG?");
        assert!(s.mode_contains_text("assist", "What is RAG?"));
        assert!(s.mode_contains_text("assist", "  what IS rag?  "));
        assert!(!s.mode_contains_text("assist", "What is HNSW?"));
        assert!(!s.mode_contains_text("assist", ""));
        // Unknown mode is a clean false (not a panic).
        assert!(!s.mode_contains_text("nonexistent", "anything"));
    }

    #[test]
    fn mode_len_counts_items_and_is_zero_for_unknown_mode() {
        let mut s = UserSession::new();
        assert_eq!(s.mode_len("assist"), 0);
        push_item(&mut s, "assist", "as-1", "one");
        push_item(&mut s, "assist", "as-2", "two");
        assert_eq!(s.mode_len("assist"), 2);
        // Unknown mode is a clean 0 (not a panic).
        assert_eq!(s.mode_len("nonexistent"), 0);
    }

    #[test]
    fn rehydrate_from_recovered_meeting_installs_state() {
        let mut s = UserSession::new();
        let recovered = RecoveredMeeting {
            id: "rec-1".to_string(),
            description: Some("recovered standup".to_string()),
            metadata: HashMap::from([("project".to_string(), "helix".to_string())]),
            started_at: chrono::Utc::now(),
            transcript_items: vec![Item {
                id: "i1".to_string(),
                text: "hello world".to_string(),
                detail: None,
                t: 100,
                meta: None,
            }],
            assist_sensitivity: crate::protocol::AssistSensitivity::Moderate,
        };
        s.rehydrate_from_recovered_meeting(&recovered);

        assert!(matches!(s.meeting_state, MeetingState::Active));
        assert_eq!(
            s.meeting.as_ref().map(|m| m.meeting_id.as_str()),
            Some("rec-1")
        );
        assert_eq!(s.metadata.get("project"), Some(&"helix".to_string()));
        assert!(s.meeting.as_ref().map(|m| m.started_at_instant).is_some());
        assert_eq!(s.current_mode, DEFAULT_MODE_ID);
        let items = s.items_per_mode.get(DEFAULT_MODE_ID).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "hello world");
    }

    #[test]
    fn new_has_default_modes() {
        // Asserts the *internal* catalog — not the live-snapshot
        // subset. actions / open_questions stay here so the agent's
        // push_item_for_mode calls still land, even though they're
        // filtered out of the wire snapshot for live UI pickers.
        let s = UserSession::new();
        assert_eq!(s.available_modes.len(), 8);
        assert_eq!(s.available_modes[0].id, "assist");
        assert_eq!(s.available_modes[1].id, "highlights");
        assert_eq!(s.available_modes[2].id, "transcript");
        assert_eq!(s.available_modes[3].id, "actions");
        assert_eq!(s.available_modes[4].id, "open_questions");
        assert_eq!(s.available_modes[5].id, "summary");
        assert_eq!(s.available_modes[6].id, "chat");
        assert_eq!(s.available_modes[7].id, "quick_asks");
    }

    #[test]
    fn new_has_empty_items_per_mode() {
        let s = UserSession::new();
        for mode in &s.available_modes {
            assert_eq!(s.items_per_mode[&mode.id].len(), 0);
        }
    }

    #[test]
    fn new_default_current_mode_is_transcript() {
        let s = UserSession::new();
        assert_eq!(s.current_mode, "transcript");
    }

    #[test]
    fn snapshot_initial_state() {
        let s = UserSession::new();
        match s.snapshot() {
            Event::Snapshot {
                protocol_version,
                meeting_state,
                meeting_id,
                available_modes,
                mode,
                display_tag,
                metadata,
                items,
                status,
                prior_context,
                devices,
                audio_source_device_id,
                attached_meeting_ids: _,
                assist_sensitivity: _,
            } => {
                assert_eq!(protocol_version, PROTOCOL_VERSION);
                assert!(matches!(meeting_state, MeetingState::Idle));
                assert!(
                    meeting_id.is_none(),
                    "idle snapshot should not carry a meeting_id"
                );
                // The snapshot filters out `actions` and `open_questions`
                // (wrap-up-only modes), so the wire list is the 8
                // internal modes minus those 2 = 6: assist, highlights,
                // transcript, summary, chat, quick_asks.
                assert_eq!(available_modes.len(), 6);
                assert!(available_modes.iter().all(|m| m.id != "actions"));
                assert!(available_modes.iter().all(|m| m.id != "open_questions"));
                assert!(available_modes.iter().any(|m| m.id == "assist"));
                assert_eq!(mode, "transcript");
                assert!(display_tag.is_none());
                assert!(metadata.is_empty());
                assert!(items.is_empty());
                assert!(prior_context.is_none());
                assert!(devices.is_empty());
                assert!(audio_source_device_id.is_none());
                assert!(!status.listening);
                assert!(status.error.is_none());
            }
            e => panic!("expected snapshot, got {:?}", e),
        }
    }

    #[test]
    fn start_meeting_from_idle() {
        let mut s = UserSession::new();
        let out = s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: Some(HashMap::from([("project".into(), "helix".into())])),
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        assert!(matches!(s.meeting_state, MeetingState::Active));
        assert_eq!(s.metadata.get("project"), Some(&"helix".into()));
        // 4 events: state change, metadata, mode, assist sensitivity.
        // The AssistSensitivityChanged emit lets cross-device clients
        // (who get MeetingStateChanged but not a fresh snapshot)
        // learn the new meeting's sensitivity in one round-trip.
        assert_eq!(out.events.len(), 4);
        assert!(matches!(
            out.events[0],
            Event::MeetingStateChanged {
                meeting_state: MeetingState::Active,
                meeting_id: Some(_),
            }
        ));
        assert!(matches!(out.events[1], Event::MetadataChanged { .. }));
        assert!(matches!(out.events[2], Event::ModeChanged { .. }));
        assert!(matches!(
            out.events[3],
            Event::AssistSensitivityChanged { .. }
        ));
        assert!(out.started_meeting);
        assert!(out.start_extraction_for.is_none());
    }

    #[test]
    fn start_meeting_with_description_signals_extraction() {
        let mut s = UserSession::new();
        let out = s.apply_intent(Intent::StartMeeting {
            description: Some("Q1 budget review".into()),
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        assert_eq!(
            out.start_extraction_for.as_deref(),
            Some("Q1 budget review")
        );
    }

    #[test]
    fn start_meeting_stores_description_in_state() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: Some("Quarterly review with Acme. Susan + 2 engineers.".into()),
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        assert_eq!(
            s.description.as_deref(),
            Some("Quarterly review with Acme. Susan + 2 engineers.")
        );
    }

    #[test]
    fn start_meeting_with_empty_description_stores_none() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: Some(String::new()),
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        assert!(s.description.is_none());
    }

    #[test]
    fn stop_meeting_clears_description() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: Some("hello".into()),
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        assert!(s.description.is_some());
        s.apply_intent(Intent::StopMeeting);
        assert!(s.description.is_none());
    }

    #[test]
    fn rehydrate_restores_description_from_recovered_meeting() {
        let mut s = UserSession::new();
        let recovered = RecoveredMeeting {
            id: "rec-2".to_string(),
            description: Some("recovered prose".to_string()),
            metadata: HashMap::new(),
            started_at: chrono::Utc::now(),
            transcript_items: vec![],
            assist_sensitivity: crate::protocol::AssistSensitivity::Moderate,
        };
        s.rehydrate_from_recovered_meeting(&recovered);
        assert_eq!(s.description.as_deref(), Some("recovered prose"));
    }

    #[test]
    fn start_meeting_when_active_is_noop() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        let active_id = s.meeting.as_ref().map(|m| m.meeting_id.clone());
        let out = s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        // No new meeting created — same id, same state.
        assert!(out.events.is_empty());
        assert!(!out.started_meeting);
        assert!(out.created_meeting.is_none());
        assert_eq!(s.meeting.as_ref().map(|m| m.meeting_id.clone()), active_id);
        assert!(matches!(s.meeting_state, MeetingState::Active));
        // Defense-in-depth: re-emit the current state to the
        // originating session so its UI lands on the live meeting.
        match out.originator_only {
            Some(Event::MeetingStateChanged {
                meeting_state,
                meeting_id,
            }) => {
                assert!(matches!(meeting_state, MeetingState::Active));
                assert_eq!(meeting_id, active_id);
            }
            other => panic!("expected MeetingStateChanged echo, got {:?}", other),
        }
    }

    #[test]
    fn stop_meeting_from_active() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: Some(HashMap::from([("k".into(), "v".into())])),
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        let out = s.apply_intent(Intent::StopMeeting);
        assert!(matches!(s.meeting_state, MeetingState::Idle));
        assert!(s.metadata.is_empty());
        assert!(s.items_per_mode.values().all(|v| v.is_empty()));
        assert_eq!(s.current_mode, "transcript");
        assert_eq!(out.events.len(), 1);
        assert!(out.stopped_meeting);
    }

    #[test]
    fn stop_meeting_when_idle_is_noop() {
        let mut s = UserSession::new();
        let out = s.apply_intent(Intent::StopMeeting);
        assert!(out.events.is_empty());
        assert!(!out.stopped_meeting);
    }

    #[test]
    fn start_meeting_clears_stale_items_except_quick_asks() {
        // A late agent write that slips past the stop-time clear
        // (release builds skip the items-empty-when-idle debug_assert)
        // leaves stray items in the idle map. Starting the next
        // meeting must not inherit them — but the quick_asks library
        // is persistent and must survive meeting boundaries.
        let mut s = UserSession::new();
        push_item(&mut s, "chat", "stale-chat", "meeting-1 leftover reply");
        push_item(&mut s, "assist", "stale-assist", "meeting-1 leftover hint");
        push_item(&mut s, "quick_asks", "qa-1", "my saved quick ask");

        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });

        assert!(
            s.items_per_mode["chat"].is_empty(),
            "stale chat items must be cleared at start"
        );
        assert!(
            s.items_per_mode["assist"].is_empty(),
            "stale assist items must be cleared at start"
        );
        assert_eq!(
            s.items_per_mode["quick_asks"].len(),
            1,
            "quick_asks library survives meeting boundaries"
        );
    }

    #[test]
    fn legacy_pause_resume_are_silent_noops() {
        // Pause/Resume are kept as no-op variants for old TestFlight
        // builds. They must not produce events, state changes, or
        // outcome flags — old clients should see nothing on click.
        let mut s = UserSession::new();
        let out = s.apply_intent(Intent::Pause);
        assert!(out.events.is_empty());
        assert!(!out.started_meeting);
        assert!(!out.stopped_meeting);
        assert!(matches!(s.meeting_state, MeetingState::Idle));

        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        let out = s.apply_intent(Intent::Pause);
        assert!(out.events.is_empty());
        assert!(matches!(s.meeting_state, MeetingState::Active));
        let out = s.apply_intent(Intent::Resume);
        assert!(out.events.is_empty());
        assert!(matches!(s.meeting_state, MeetingState::Active));
    }

    #[test]
    fn set_mode_is_legacy_noop() {
        // `currentMode` is now per-surface UI state; the SetMode
        // intent is a no-op kept for compatibility with deployed
        // clients still sending it. No events, no state change.
        let mut s = UserSession::new();
        let initial = s.current_mode.clone();
        let out = s.apply_intent(Intent::SetMode {
            mode: "highlights".into(),
        });
        assert_eq!(s.current_mode, initial);
        assert!(out.events.is_empty());
        assert!(out.originator_only.is_none());
    }

    #[test]
    fn set_metadata_insert() {
        let mut s = UserSession::new();
        let out = s.apply_intent(Intent::SetMetadata {
            key: "project".into(),
            value: Some("helix".into()),
        });
        assert_eq!(s.metadata.get("project"), Some(&"helix".into()));
        match &out.events[..] {
            [Event::MetadataChanged { metadata }] => {
                assert_eq!(metadata.len(), 1);
                assert_eq!(metadata["project"], "helix");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn set_metadata_delete() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::SetMetadata {
            key: "k".into(),
            value: Some("v".into()),
        });
        let out = s.apply_intent(Intent::SetMetadata {
            key: "k".into(),
            value: None,
        });
        assert!(s.metadata.is_empty());
        match &out.events[..] {
            [Event::MetadataChanged { metadata }] => assert!(metadata.is_empty()),
            _ => panic!(),
        }
    }

    #[test]
    fn mark_moment_active_emits_status() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        let out = s.apply_intent(Intent::MarkMoment {
            t: 1234,
            note: None,
        });
        match &out.events[..] {
            [Event::Status { status }] => {
                assert!(status.listening);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn mark_moment_idle_is_noop() {
        let mut s = UserSession::new();
        let out = s.apply_intent(Intent::MarkMoment { t: 0, note: None });
        assert!(out.events.is_empty());
    }

    #[test]
    fn mark_moment_t_zero_computes_server_offset() {
        use std::time::{Duration, Instant};
        // Glasses/mobile clients (and proto3's absent-scalar default)
        // send t:0. The server must substitute its own meeting-clock
        // offset rather than anchoring the moment at meeting start.
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        // Rewind the meeting clock 10s so a server-computed offset is
        // clearly distinguishable from a passed-through 0.
        s.meeting.as_mut().unwrap().started_at_instant = Instant::now() - Duration::from_secs(10);
        let out = s.apply_intent(Intent::MarkMoment { t: 0, note: None });
        let req = out.mark_moment.expect("mark_moment outcome set");
        assert!(
            req.t >= 10_000,
            "t==0 sentinel should yield the server-side elapsed offset, got {}",
            req.t
        );
        assert!(req.t < 11_000, "offset should be ~10s, got {}", req.t);
    }

    #[test]
    fn mark_moment_nonzero_t_passes_through() {
        // A client that knows the offset (phone CTA, Mac) is trusted
        // verbatim — across a server restart the client clock is the
        // more accurate one (recovery re-stamps started_at_instant).
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        let out = s.apply_intent(Intent::MarkMoment {
            t: 4242,
            note: None,
        });
        let req = out.mark_moment.expect("mark_moment outcome set");
        assert_eq!(req.t, 4242);
    }

    #[test]
    fn set_item_detail_writes_in_place() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        push_item(&mut s, "highlights", "h1", "first");
        push_item(&mut s, "highlights", "h2", "second");

        let updated = s.set_item_detail("highlights", "h2", "an expansion");
        let updated = updated.expect("hit");
        assert_eq!(updated.id, "h2");
        assert_eq!(updated.detail.as_deref(), Some("an expansion"));

        // Other item untouched.
        let items = s.items_per_mode.get("highlights").unwrap();
        assert!(items
            .iter()
            .find(|i| i.id == "h1")
            .unwrap()
            .detail
            .is_none());
    }

    #[test]
    fn set_item_detail_returns_none_for_unknown() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        assert!(s.set_item_detail("highlights", "nope", "x").is_none());
        assert!(s.set_item_detail("nonexistent_mode", "h1", "x").is_none());
    }

    #[test]
    fn find_item_by_id_searches_across_modes() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        push_item(&mut s, "highlights", "h1", "found me");
        push_item(&mut s, "actions", "a1", "do thing");

        let (mode, text) = s.find_item_by_id("a1").expect("hit");
        assert_eq!(mode, "actions");
        assert_eq!(text, "do thing");
        assert!(s.find_item_by_id("nope").is_none());
    }

    #[test]
    fn rolling_transcript_appends_chunks() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        let chunk = crate::stt::TranscriptChunk {
            id: "c1".into(),
            text: "hello world".into(),
            t_start_ms: 100,
            t_end_ms: 1100,
            speaker: None,
            user_id: "test-user".into(),
        };
        s.append_transcript_chunk(chunk);
        assert_eq!(s.rolling_transcript_text(), "hello world");
    }

    #[test]
    fn rolling_transcript_joins_with_newlines() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        s.append_transcript_chunk(crate::stt::TranscriptChunk {
            id: "c1".into(),
            text: "first line".into(),
            t_start_ms: 0,
            t_end_ms: 1000,
            speaker: None,
            user_id: "test-user".into(),
        });
        s.append_transcript_chunk(crate::stt::TranscriptChunk {
            id: "c2".into(),
            text: "second line".into(),
            t_start_ms: 1100,
            t_end_ms: 2000,
            speaker: None,
            user_id: "test-user".into(),
        });
        assert_eq!(s.rolling_transcript_text(), "first line\nsecond line");
    }

    #[test]
    fn append_transcript_chunk_noop_when_not_active() {
        let mut s = UserSession::new();
        // Meeting is Idle, not Active
        s.append_transcript_chunk(crate::stt::TranscriptChunk {
            id: "c1".into(),
            text: "should not be stored".into(),
            t_start_ms: 0,
            t_end_ms: 100,
            speaker: None,
            user_id: "test-user".into(),
        });
        assert_eq!(s.rolling_transcript_text(), "");
    }

    #[test]
    fn stop_meeting_clears_rolling_transcript() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        s.append_transcript_chunk(crate::stt::TranscriptChunk {
            id: "c1".into(),
            text: "stuff".into(),
            t_start_ms: 0,
            t_end_ms: 100,
            speaker: None,
            user_id: "test-user".into(),
        });
        s.apply_intent(Intent::StopMeeting);
        assert_eq!(s.rolling_transcript_text(), "");
    }

    #[test]
    fn push_item_for_mode_replace_caps_at_10() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        // current_mode = highlights = replace strategy
        for i in 0..15 {
            let item = Item {
                id: format!("h{}", i),
                text: format!("item {}", i),
                detail: None,
                t: i as u64,
                meta: None,
            };
            let payload = s.push_item_for_mode("highlights", item);
            assert!(payload.len() <= 10);
        }
        let final_items = &s.items_per_mode["highlights"];
        assert_eq!(final_items.len(), 10);
        assert_eq!(final_items[0].id, "h5"); // FIFO drop kept items 5..15
        assert_eq!(final_items[9].id, "h14");
    }

    #[test]
    fn push_item_for_mode_append_returns_single_item() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        let item = Item {
            id: "t1".into(),
            text: "hi".into(),
            detail: None,
            t: 0,
            meta: None,
        };
        let payload = s.push_item_for_mode("transcript", item.clone());
        assert_eq!(payload.len(), 1);
        assert_eq!(payload[0].id, "t1");
        // items_per_mode keeps growing
        for i in 0..5 {
            s.push_item_for_mode(
                "transcript",
                Item {
                    id: format!("t{}", i + 2),
                    text: format!("hi{}", i + 2),
                    detail: None,
                    t: i as u64,
                    meta: None,
                },
            );
        }
        assert_eq!(s.items_per_mode["transcript"].len(), 6);
    }

    #[test]
    fn push_item_for_unknown_mode_is_noop() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        let payload = s.push_item_for_mode(
            "nonexistent",
            Item {
                id: "x".into(),
                text: "y".into(),
                detail: None,
                t: 0,
                meta: None,
            },
        );
        assert!(payload.is_empty());
    }

    #[test]
    fn replace_items_for_mode_overwrites() {
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        let initial: Vec<Item> = (0..3)
            .map(|i| Item {
                id: format!("h{}", i),
                text: format!("first {}", i),
                detail: None,
                t: 0,
                meta: None,
            })
            .collect();
        s.replace_items_for_mode("highlights", initial);
        let replacement: Vec<Item> = (0..2)
            .map(|i| Item {
                id: format!("h{}", i),
                text: format!("second {}", i),
                detail: None,
                t: 0,
                meta: None,
            })
            .collect();
        let payload = s.replace_items_for_mode("highlights", replacement);
        assert_eq!(payload.len(), 2);
        assert_eq!(s.items_per_mode["highlights"].len(), 2);
        assert_eq!(s.items_per_mode["highlights"][0].text, "second 0");
    }

    // ── Device registry ──────────────────────────────────────────────

    #[test]
    fn register_device_assigns_unique_id_and_marks_online() {
        let mut s = UserSession::new();
        let device = s.register_device(
            "conn-1".into(),
            "tiago-laptop".into(),
            vec![
                crate::protocol::Capability::AudioCapture,
                crate::protocol::Capability::SystemAudio,
            ],
            None,
        );
        assert!(!device.id.is_empty());
        assert_eq!(device.hostname, "tiago-laptop");
        assert!(device.online);
        assert_eq!(device.capabilities.len(), 2);

        let all = s.devices_clone();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, device.id);
    }

    #[test]
    fn unregister_device_removes_entry() {
        let mut s = UserSession::new();
        let device = s.register_device(
            "conn-1".into(),
            "tiago-laptop".into(),
            vec![crate::protocol::Capability::AudioCapture],
            None,
        );
        assert_eq!(s.devices_clone().len(), 1);

        let removed = s.unregister_device("conn-1");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id, device.id);
        assert!(s.devices_clone().is_empty());
    }

    #[test]
    fn unregister_device_keeps_audio_binding_for_resume() {
        // The bound audio-source device disconnecting (crash / Ctrl-C /
        // force-quit) must NOT clear the binding — it has to survive so
        // the same device, reconnecting, can resume the live meeting.
        // (Abandonment is handled by the liveness reaper, not here.)
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        let device = s.register_device(
            "conn-1".into(),
            "tiago-laptop".into(),
            vec![crate::protocol::Capability::AudioCapture],
            None,
        );
        s.meeting.as_mut().unwrap().audio_source_device_id = Some(device.id.clone());

        s.unregister_device("conn-1");
        assert_eq!(
            s.meeting
                .as_ref()
                .unwrap()
                .audio_source_device_id
                .as_deref(),
            Some(device.id.as_str()),
            "audio binding must survive the bound device's disconnect so it can resume"
        );
    }

    #[test]
    fn register_device_reuses_client_supplied_stable_id() {
        // A client (PWA) that persists its device id across reconnects
        // sends it back on re-register; the server must reuse it rather
        // than mint a fresh UUID, so the device's identity — and any
        // audio-source binding keyed on it — survives the reconnect.
        let mut s = UserSession::new();
        let first = s.register_device(
            "conn-1".into(),
            "Browser (Glasses)".into(),
            vec![crate::protocol::Capability::AudioCapture],
            Some("stable-abc".into()),
        );
        assert_eq!(first.id, "stable-abc");

        // Reconnect under a new connection, same stable id.
        let second = s.register_device(
            "conn-2".into(),
            "Browser (Glasses)".into(),
            vec![crate::protocol::Capability::AudioCapture],
            Some("stable-abc".into()),
        );
        assert_eq!(second.id, "stable-abc");
        // Stale conn-1 entry evicted — exactly one logical device.
        let all = s.devices_clone();
        assert_eq!(all.len(), 1, "stale connection should be deduped");
        assert_eq!(all[0].id, "stable-abc");
    }

    #[test]
    fn audio_binding_survives_reconnect_of_same_stable_id() {
        // The core regression: wifi→5G drops the socket, the device
        // reconnects with the same stable id under a new connection,
        // and the *old* connection's disconnect fires late. The binding
        // must NOT be cleared, because the logical device is still here.
        let mut s = UserSession::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        s.register_device(
            "conn-old".into(),
            "Browser (Glasses)".into(),
            vec![crate::protocol::Capability::AudioCapture],
            Some("stable-abc".into()),
        );
        s.meeting.as_mut().unwrap().audio_source_device_id = Some("stable-abc".into());

        // Reconnect: new connection, same stable id (evicts conn-old).
        s.register_device(
            "conn-new".into(),
            "Browser (Glasses)".into(),
            vec![crate::protocol::Capability::AudioCapture],
            Some("stable-abc".into()),
        );

        // The late disconnect of the dead old socket must be a no-op
        // for the binding — conn-old was already evicted, and even if
        // it lingered, conn-new still carries the id.
        s.unregister_device("conn-old");
        assert_eq!(
            s.meeting
                .as_ref()
                .unwrap()
                .audio_source_device_id
                .as_deref(),
            Some("stable-abc"),
            "binding must survive a same-id reconnect"
        );

        // Even a true disconnect (no other connection holds the id)
        // now KEEPS the binding: the device may relaunch and resume the
        // meeting, and the snapshot must still report it as the source.
        // Genuine abandonment is ended by the liveness reaper, not by
        // unbinding here.
        s.unregister_device("conn-new");
        assert_eq!(
            s.meeting
                .as_ref()
                .unwrap()
                .audio_source_device_id
                .as_deref(),
            Some("stable-abc"),
            "binding must survive a full disconnect so the device can resume on relaunch"
        );
    }

    #[test]
    fn unregister_device_keeps_audio_binding_if_different() {
        let mut s = UserSession::new();
        // Need an active meeting for audio_source_device_id to be meaningful.
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        let other = s.register_device(
            "conn-other".into(),
            "other-mac".into(),
            vec![crate::protocol::Capability::AudioCapture],
            None,
        );
        let _ = s.register_device(
            "conn-self".into(),
            "tiago-laptop".into(),
            vec![crate::protocol::Capability::AudioCapture],
            None,
        );
        s.meeting.as_mut().unwrap().audio_source_device_id = Some(other.id.clone());

        s.unregister_device("conn-self");
        assert_eq!(
            s.meeting
                .as_ref()
                .unwrap()
                .audio_source_device_id
                .as_deref(),
            Some(other.id.as_str()),
            "audio binding should persist when an unrelated device disconnects"
        );
    }

    #[test]
    fn snapshot_includes_devices_and_audio_binding() {
        let mut s = UserSession::new();
        // Need an active meeting for audio_source_device_id to appear in snapshot.
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        });
        let d = s.register_device(
            "conn-1".into(),
            "host".into(),
            vec![crate::protocol::Capability::ControlSurface],
            None,
        );
        s.meeting.as_mut().unwrap().audio_source_device_id = Some(d.id.clone());

        match s.snapshot() {
            Event::Snapshot {
                devices,
                audio_source_device_id,
                ..
            } => {
                assert_eq!(devices.len(), 1);
                assert_eq!(devices[0].id, d.id);
                assert_eq!(audio_source_device_id.as_deref(), Some(d.id.as_str()));
            }
            e => panic!("expected snapshot, got {e:?}"),
        }
    }
}
