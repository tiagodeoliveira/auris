//! ServerState — owns all meeting state. See `docs/specs/server.md` §3.

use crate::contract::{
    Event, Intent, Item, MeetingState, ModeOption, Status, UpdateStrategy, PROTOCOL_VERSION,
};
use std::collections::HashMap;
use std::time::Instant;

pub fn default_modes() -> Vec<ModeOption> {
    vec![
        ModeOption {
            id: "highlights".into(),
            label: "Highlights".into(),
            update_strategy: UpdateStrategy::Replace,
        },
        ModeOption {
            id: "transcript".into(),
            label: "Transcript".into(),
            update_strategy: UpdateStrategy::Append,
        },
        ModeOption {
            id: "actions".into(),
            label: "Actions".into(),
            update_strategy: UpdateStrategy::Append,
        },
    ]
}

pub const DEFAULT_MODE_ID: &str = "highlights";

fn synthesize_detail(text: &str) -> String {
    format!(
        "Detail for '{}': lorem ipsum dolor sit amet, consectetur adipiscing elit. \
         Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. \
         Ut enim ad minim veniam.",
        text
    )
}

/// Result of applying an intent. `events` are broadcast in order.
/// `error` is sent only to the originating client (None unless protocol error).
#[derive(Debug, Default)]
pub struct IntentOutcome {
    pub events: Vec<Event>,
    pub error: Option<Event>,
    pub start_extraction_for: Option<String>,
    pub started_meeting: bool,
    pub stopped_meeting: bool,
    pub paused_meeting: bool,
    pub resumed_meeting: bool,
}

pub struct ServerState {
    pub(crate) meeting_state: MeetingState,
    pub(crate) available_modes: Vec<ModeOption>,
    pub(crate) current_mode: String,
    pub(crate) items_per_mode: HashMap<String, Vec<Item>>,
    pub(crate) metadata: HashMap<String, String>,
    pub(crate) meeting_started_at: Option<Instant>,
}

impl ServerState {
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
            meeting_started_at: None,
        };
        s.assert_invariants();
        s
    }

    pub fn snapshot(&self) -> Event {
        Event::Snapshot {
            protocol_version: PROTOCOL_VERSION,
            meeting_state: self.meeting_state,
            available_modes: self.available_modes.clone(),
            mode: self.current_mode.clone(),
            display_tag: None,
            metadata: self.metadata.clone(),
            items: self
                .items_per_mode
                .get(&self.current_mode)
                .cloned()
                .unwrap_or_default(),
            status: Status {
                listening: matches!(self.meeting_state, MeetingState::Active),
                paused: matches!(self.meeting_state, MeetingState::Paused),
                error: None,
            },
        }
    }

    pub fn apply_intent(&mut self, intent: Intent) -> IntentOutcome {
        let mut outcome = IntentOutcome::default();
        match intent {
            Intent::StartMeeting {
                description,
                metadata,
            } => {
                self.handle_start_meeting(description, metadata, &mut outcome);
            }
            Intent::StopMeeting => {
                self.handle_stop_meeting(&mut outcome);
            }
            Intent::Pause => {
                self.handle_pause(&mut outcome);
            }
            Intent::Resume => {
                self.handle_resume(&mut outcome);
            }
            Intent::SetMode { mode } => self.handle_set_mode(mode, &mut outcome),
            Intent::SetMetadata { key, value } => {
                self.handle_set_metadata(key, value, &mut outcome)
            }
            Intent::MarkMoment { t, note } => self.handle_mark_moment(t, note, &mut outcome),
            Intent::ExpandItem { item_id } => self.handle_expand_item(item_id, &mut outcome),
        }
        self.assert_invariants();
        outcome
    }

    fn handle_start_meeting(
        &mut self,
        description: Option<String>,
        metadata: Option<HashMap<String, String>>,
        outcome: &mut IntentOutcome,
    ) {
        if !matches!(self.meeting_state, MeetingState::Idle) {
            tracing::warn!(state = ?self.meeting_state, "start_meeting in invalid state");
            return;
        }
        self.meeting_state = MeetingState::Active;
        self.meeting_started_at = Some(Instant::now());
        self.metadata = metadata.unwrap_or_default();
        self.current_mode = DEFAULT_MODE_ID.to_string();

        outcome.events.push(Event::MeetingStateChanged {
            meeting_state: MeetingState::Active,
        });
        outcome.events.push(Event::MetadataChanged {
            metadata: self.metadata.clone(),
        });
        outcome.events.push(Event::ModeChanged {
            mode: self.current_mode.clone(),
            display_tag: None,
            items: self.items_per_mode[&self.current_mode].clone(),
        });
        outcome.started_meeting = true;
        if let Some(d) = description.filter(|s| !s.is_empty()) {
            outcome.start_extraction_for = Some(d);
        }
    }

    fn handle_stop_meeting(&mut self, outcome: &mut IntentOutcome) {
        if matches!(self.meeting_state, MeetingState::Idle) {
            tracing::warn!("stop_meeting in idle state");
            return;
        }
        self.meeting_state = MeetingState::Idle;
        self.metadata.clear();
        for v in self.items_per_mode.values_mut() {
            v.clear();
        }
        self.meeting_started_at = None;
        self.current_mode = DEFAULT_MODE_ID.to_string();

        outcome.events.push(Event::MeetingStateChanged {
            meeting_state: MeetingState::Idle,
        });
        outcome.stopped_meeting = true;
    }

    fn handle_pause(&mut self, outcome: &mut IntentOutcome) {
        if !matches!(self.meeting_state, MeetingState::Active) {
            tracing::warn!(state = ?self.meeting_state, "pause in invalid state");
            return;
        }
        self.meeting_state = MeetingState::Paused;
        outcome.events.push(Event::MeetingStateChanged {
            meeting_state: MeetingState::Paused,
        });
        outcome.paused_meeting = true;
    }

    fn handle_resume(&mut self, outcome: &mut IntentOutcome) {
        if !matches!(self.meeting_state, MeetingState::Paused) {
            tracing::warn!(state = ?self.meeting_state, "resume in invalid state");
            return;
        }
        self.meeting_state = MeetingState::Active;
        outcome.events.push(Event::MeetingStateChanged {
            meeting_state: MeetingState::Active,
        });
        outcome.resumed_meeting = true;
    }

    fn handle_set_mode(&mut self, mode: String, outcome: &mut IntentOutcome) {
        if !self.available_modes.iter().any(|m| m.id == mode) {
            outcome.error = Some(Event::Error {
                code: "unknown_mode".into(),
                message: format!("mode '{}' not in catalog", mode),
                intent_ref: Some(mode),
            });
            return;
        }
        self.current_mode = mode.clone();
        outcome.events.push(Event::ModeChanged {
            mode,
            display_tag: None,
            items: self.items_per_mode[&self.current_mode].clone(),
        });
    }

    fn handle_set_metadata(
        &mut self,
        key: String,
        value: Option<String>,
        outcome: &mut IntentOutcome,
    ) {
        match value {
            Some(v) => {
                self.metadata.insert(key, v);
            }
            None => {
                self.metadata.remove(&key);
            }
        }
        outcome.events.push(Event::MetadataChanged {
            metadata: self.metadata.clone(),
        });
    }

    fn handle_mark_moment(&mut self, t: u64, note: Option<String>, outcome: &mut IntentOutcome) {
        if !matches!(self.meeting_state, MeetingState::Active) {
            tracing::warn!(state = ?self.meeting_state, "mark_moment in invalid state");
            return;
        }
        tracing::info!(t, ?note, "mark_moment");
        outcome.events.push(Event::Status {
            status: Status {
                listening: true,
                paused: false,
                error: None,
            },
        });
    }

    fn handle_expand_item(&mut self, item_id: String, outcome: &mut IntentOutcome) {
        let mode_id = self.current_mode.clone();
        let strategy = self
            .available_modes
            .iter()
            .find(|m| m.id == mode_id)
            .map(|m| m.update_strategy)
            .expect("invariant: current_mode in available_modes");

        let items = self
            .items_per_mode
            .get_mut(&mode_id)
            .expect("invariant: items_per_mode entry exists");
        let Some(idx) = items.iter().position(|i| i.id == item_id) else {
            outcome.error = Some(Event::Error {
                code: "unknown_item".into(),
                message: format!("item '{}' not found in current mode", item_id),
                intent_ref: Some(item_id),
            });
            return;
        };

        let detail = synthesize_detail(&items[idx].text);
        items[idx].detail = Some(detail);

        let payload = match strategy {
            UpdateStrategy::Replace => items.clone(),
            UpdateStrategy::Append => vec![items[idx].clone()],
        };
        outcome.events.push(Event::ItemsUpdate { items: payload });
    }

    pub fn current_mode_id(&self) -> &str {
        &self.current_mode
    }

    pub fn meeting_started_at(&self) -> Option<Instant> {
        self.meeting_started_at
    }

    pub fn snapshot_meeting_state(&self) -> MeetingState {
        self.meeting_state
    }

    pub fn metadata_clone(&self) -> HashMap<String, String> {
        self.metadata.clone()
    }

    pub fn set_metadata_full(&mut self, metadata: HashMap<String, String>) {
        if matches!(self.meeting_state, MeetingState::Idle) {
            // Don't apply extraction results to idle state — meeting was stopped mid-extraction.
            return;
        }
        self.metadata = metadata;
    }

    pub(crate) fn assert_invariants(&self) {
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
        match self.meeting_state {
            MeetingState::Idle => {
                debug_assert!(
                    self.items_per_mode.values().all(|v| v.is_empty()),
                    "items must be empty when idle"
                );
                debug_assert!(
                    self.meeting_started_at.is_none(),
                    "meeting_started_at must be None when idle"
                );
            }
            MeetingState::Active | MeetingState::Paused => {
                debug_assert!(
                    self.meeting_started_at.is_some(),
                    "meeting_started_at must be Some when not idle"
                );
            }
        }
    }
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_item(s: &mut ServerState, mode: &str, id: &str, text: &str) {
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
        let s = ServerState::new();
        assert!(matches!(s.meeting_state, MeetingState::Idle));
    }

    #[test]
    fn new_has_three_default_modes() {
        let s = ServerState::new();
        assert_eq!(s.available_modes.len(), 3);
        assert_eq!(s.available_modes[0].id, "highlights");
        assert_eq!(s.available_modes[1].id, "transcript");
        assert_eq!(s.available_modes[2].id, "actions");
    }

    #[test]
    fn new_has_empty_items_per_mode() {
        let s = ServerState::new();
        for mode in &s.available_modes {
            assert_eq!(s.items_per_mode[&mode.id].len(), 0);
        }
    }

    #[test]
    fn new_default_current_mode_is_highlights() {
        let s = ServerState::new();
        assert_eq!(s.current_mode, "highlights");
    }

    #[test]
    fn snapshot_initial_state() {
        let s = ServerState::new();
        match s.snapshot() {
            Event::Snapshot {
                protocol_version,
                meeting_state,
                available_modes,
                mode,
                display_tag,
                metadata,
                items,
                status,
            } => {
                assert_eq!(protocol_version, PROTOCOL_VERSION);
                assert!(matches!(meeting_state, MeetingState::Idle));
                assert_eq!(available_modes.len(), 3);
                assert_eq!(mode, "highlights");
                assert!(display_tag.is_none());
                assert!(metadata.is_empty());
                assert!(items.is_empty());
                assert!(!status.listening);
                assert!(!status.paused);
                assert!(status.error.is_none());
            }
            e => panic!("expected snapshot, got {:?}", e),
        }
    }

    #[test]
    fn start_meeting_from_idle() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: Some(HashMap::from([("project".into(), "helix".into())])),
        });
        assert!(matches!(s.meeting_state, MeetingState::Active));
        assert_eq!(s.metadata.get("project"), Some(&"helix".into()));
        assert_eq!(out.events.len(), 3);
        assert!(matches!(
            out.events[0],
            Event::MeetingStateChanged {
                meeting_state: MeetingState::Active
            }
        ));
        assert!(matches!(out.events[1], Event::MetadataChanged { .. }));
        assert!(matches!(out.events[2], Event::ModeChanged { .. }));
        assert!(out.started_meeting);
        assert!(out.start_extraction_for.is_none());
    }

    #[test]
    fn start_meeting_with_description_signals_extraction() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::StartMeeting {
            description: Some("Q1 budget review".into()),
            metadata: None,
        });
        assert_eq!(
            out.start_extraction_for.as_deref(),
            Some("Q1 budget review")
        );
    }

    #[test]
    fn start_meeting_when_active_is_noop() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
        });
        let out = s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
        });
        assert!(out.events.is_empty());
        assert!(!out.started_meeting);
        assert!(matches!(s.meeting_state, MeetingState::Active));
    }

    #[test]
    fn stop_meeting_from_active() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: Some(HashMap::from([("k".into(), "v".into())])),
        });
        let out = s.apply_intent(Intent::StopMeeting);
        assert!(matches!(s.meeting_state, MeetingState::Idle));
        assert!(s.metadata.is_empty());
        assert!(s.items_per_mode.values().all(|v| v.is_empty()));
        assert_eq!(s.current_mode, "highlights");
        assert_eq!(out.events.len(), 1);
        assert!(out.stopped_meeting);
    }

    #[test]
    fn stop_meeting_when_idle_is_noop() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::StopMeeting);
        assert!(out.events.is_empty());
        assert!(!out.stopped_meeting);
    }

    #[test]
    fn pause_from_active() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
        });
        let out = s.apply_intent(Intent::Pause);
        assert!(matches!(s.meeting_state, MeetingState::Paused));
        assert_eq!(out.events.len(), 1);
        assert!(out.paused_meeting);
    }

    #[test]
    fn pause_when_idle_or_paused_is_noop() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::Pause);
        assert!(out.events.is_empty());

        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
        });
        s.apply_intent(Intent::Pause);
        let out2 = s.apply_intent(Intent::Pause);
        assert!(out2.events.is_empty());
    }

    #[test]
    fn resume_from_paused() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
        });
        s.apply_intent(Intent::Pause);
        let out = s.apply_intent(Intent::Resume);
        assert!(matches!(s.meeting_state, MeetingState::Active));
        assert!(out.resumed_meeting);
    }

    #[test]
    fn resume_when_idle_or_active_is_noop() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::Resume);
        assert!(out.events.is_empty());

        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
        });
        let out2 = s.apply_intent(Intent::Resume);
        assert!(out2.events.is_empty());
    }

    #[test]
    fn set_mode_valid() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::SetMode {
            mode: "transcript".into(),
        });
        assert_eq!(s.current_mode, "transcript");
        assert!(out.error.is_none());
        match &out.events[..] {
            [Event::ModeChanged {
                mode,
                items,
                display_tag,
            }] => {
                assert_eq!(mode, "transcript");
                assert!(items.is_empty());
                assert!(display_tag.is_none());
            }
            other => panic!("unexpected events: {:?}", other),
        }
    }

    #[test]
    fn set_mode_unknown_emits_error() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::SetMode {
            mode: "bogus".into(),
        });
        assert_eq!(s.current_mode, "highlights");
        assert!(out.events.is_empty());
        match out.error {
            Some(Event::Error {
                code, intent_ref, ..
            }) => {
                assert_eq!(code, "unknown_mode");
                assert_eq!(intent_ref.as_deref(), Some("bogus"));
            }
            _ => panic!("expected unknown_mode error"),
        }
    }

    #[test]
    fn set_mode_in_idle_is_allowed() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::SetMode {
            mode: "actions".into(),
        });
        assert_eq!(s.current_mode, "actions");
        assert_eq!(out.events.len(), 1);
    }

    #[test]
    fn set_metadata_insert() {
        let mut s = ServerState::new();
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
        let mut s = ServerState::new();
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
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
        });
        let out = s.apply_intent(Intent::MarkMoment {
            t: 1234,
            note: None,
        });
        match &out.events[..] {
            [Event::Status { status }] => {
                assert!(status.listening);
                assert!(!status.paused);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn mark_moment_idle_is_noop() {
        let mut s = ServerState::new();
        let out = s.apply_intent(Intent::MarkMoment { t: 0, note: None });
        assert!(out.events.is_empty());
    }

    #[test]
    fn expand_item_append_strategy_returns_single_item() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
        });
        s.apply_intent(Intent::SetMode {
            mode: "transcript".into(),
        });
        push_item(&mut s, "transcript", "i1", "first");
        push_item(&mut s, "transcript", "i2", "second");

        let out = s.apply_intent(Intent::ExpandItem {
            item_id: "i2".into(),
        });
        match &out.events[..] {
            [Event::ItemsUpdate { items }] => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].id, "i2");
                assert!(items[0].detail.is_some());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn expand_item_replace_strategy_returns_full_list() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
        });
        push_item(&mut s, "highlights", "h1", "first");
        push_item(&mut s, "highlights", "h2", "second");

        let out = s.apply_intent(Intent::ExpandItem {
            item_id: "h1".into(),
        });
        match &out.events[..] {
            [Event::ItemsUpdate { items }] => {
                assert_eq!(items.len(), 2);
                assert!(items[0].detail.is_some());
                assert!(items[1].detail.is_none());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn expand_item_unknown_emits_error() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
        });
        let out = s.apply_intent(Intent::ExpandItem {
            item_id: "nope".into(),
        });
        assert!(out.events.is_empty());
        match out.error {
            Some(Event::Error {
                code, intent_ref, ..
            }) => {
                assert_eq!(code, "unknown_item");
                assert_eq!(intent_ref.as_deref(), Some("nope"));
            }
            _ => panic!(),
        }
    }
}
