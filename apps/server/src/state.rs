//! ServerState — owns all meeting state. See `docs/specs/server.md` §3.

use crate::contract::{Event, Intent, Item, MeetingState, ModeOption, Status, UpdateStrategy, PROTOCOL_VERSION};
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
            Intent::StartMeeting { description, metadata } => {
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
            // Other intents land in Tasks 7 and 8.
            _ => {
                tracing::warn!("intent not yet implemented");
            }
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

        outcome.events.push(Event::MeetingStateChanged { meeting_state: MeetingState::Active });
        outcome.events.push(Event::MetadataChanged { metadata: self.metadata.clone() });
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

        outcome.events.push(Event::MeetingStateChanged { meeting_state: MeetingState::Idle });
        outcome.stopped_meeting = true;
    }

    fn handle_pause(&mut self, outcome: &mut IntentOutcome) {
        if !matches!(self.meeting_state, MeetingState::Active) {
            tracing::warn!(state = ?self.meeting_state, "pause in invalid state");
            return;
        }
        self.meeting_state = MeetingState::Paused;
        outcome.events.push(Event::MeetingStateChanged { meeting_state: MeetingState::Paused });
        outcome.paused_meeting = true;
    }

    fn handle_resume(&mut self, outcome: &mut IntentOutcome) {
        if !matches!(self.meeting_state, MeetingState::Paused) {
            tracing::warn!(state = ?self.meeting_state, "resume in invalid state");
            return;
        }
        self.meeting_state = MeetingState::Active;
        outcome.events.push(Event::MeetingStateChanged { meeting_state: MeetingState::Active });
        outcome.resumed_meeting = true;
    }

    pub(crate) fn assert_invariants(&self) {
        debug_assert!(
            self.available_modes.iter().any(|m| m.id == self.current_mode),
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
                debug_assert!(self.metadata.is_empty(), "metadata must be empty when idle");
                debug_assert!(
                    self.items_per_mode.values().all(|v| v.is_empty()),
                    "items must be empty when idle"
                );
                debug_assert!(self.meeting_started_at.is_none(), "meeting_started_at must be None when idle");
            }
            MeetingState::Active | MeetingState::Paused => {
                debug_assert!(self.meeting_started_at.is_some(), "meeting_started_at must be Some when not idle");
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
        assert!(matches!(out.events[0], Event::MeetingStateChanged { meeting_state: MeetingState::Active }));
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
        assert_eq!(out.start_extraction_for.as_deref(), Some("Q1 budget review"));
    }

    #[test]
    fn start_meeting_when_active_is_noop() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
        let out = s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
        assert!(out.events.is_empty());
        assert!(!out.started_meeting);
        assert!(matches!(s.meeting_state, MeetingState::Active));
    }

    #[test]
    fn stop_meeting_from_active() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting { description: None, metadata: Some(HashMap::from([("k".into(), "v".into())])) });
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
        s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
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

        s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
        s.apply_intent(Intent::Pause);
        let out2 = s.apply_intent(Intent::Pause);
        assert!(out2.events.is_empty());
    }

    #[test]
    fn resume_from_paused() {
        let mut s = ServerState::new();
        s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
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

        s.apply_intent(Intent::StartMeeting { description: None, metadata: None });
        let out2 = s.apply_intent(Intent::Resume);
        assert!(out2.events.is_empty());
    }
}
