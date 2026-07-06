//! Snapshot serialisation for `UserSession`.

use super::LIVE_MODE_EXCLUSIONS;
use crate::protocol::{Event, MeetingState, Status, PROTOCOL_VERSION};
use crate::session::UserSession;

impl UserSession {
    pub fn snapshot(&self) -> Event {
        Event::Snapshot {
            protocol_version: PROTOCOL_VERSION,
            meeting_state: self.meeting_state,
            meeting_id: self.meeting.as_ref().map(|m| m.meeting_id.clone()),
            // Hide wrap-up-only modes from the live picker. The full
            // catalog is still on `self.available_modes` so internal
            // `push_item_for_mode` calls keep working for actions /
            // open_questions, and past-meeting views (which read items
            // directly from the DB) are unaffected.
            available_modes: self
                .available_modes
                .iter()
                .filter(|m| !LIVE_MODE_EXCLUSIONS.contains(&m.id.as_str()))
                .cloned()
                .collect(),
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
                error: None,
            },
            prior_context: self
                .meeting
                .as_ref()
                .and_then(|m| m.recalled_context.as_ref())
                .map(|c| c.summary()),
            devices: self.devices_by_connection.values().cloned().collect(),
            audio_source_device_id: self
                .meeting
                .as_ref()
                .and_then(|m| m.audio_source_device_id.clone()),
            // Attached past meetings live in the meeting_attachments
            // table; the snapshot ships an empty list and the REST
            // attach/detach endpoints broadcast
            // `Event::AttachedMeetingsChanged` with the canonical
            // set when the wire state needs to update. On reconnect,
            // the server fires one synthetic AttachedMeetingsChanged
            // immediately after the snapshot if any attachments
            // exist (see ws.rs).
            attached_meeting_ids: Vec::new(),
            // Active-meeting sensitivity if there is one; otherwise
            // the default so the compose-screen picker has a well-
            // defined initial value across reloads.
            assist_sensitivity: self
                .meeting
                .as_ref()
                .map(|m| m.assist_sensitivity)
                .unwrap_or_default(),
        }
    }
}
