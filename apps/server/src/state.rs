//! ServerState — owns all meeting state. See `docs/specs/server.md` §3.

use crate::contract::{Event, Item, MeetingState, ModeOption, Status, UpdateStrategy, PROTOCOL_VERSION};
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
}
