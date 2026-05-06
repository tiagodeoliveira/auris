//! UserState — owns all meeting state.

use crate::contract::{
    Event, Intent, Item, MeetingState, ModeOption, Status, UpdateStrategy, PROTOCOL_VERSION,
};
use crate::stt::TranscriptChunk;
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
        ModeOption {
            id: "open_questions".into(),
            label: "Open Questions".into(),
            update_strategy: UpdateStrategy::Append,
        },
    ]
}

pub const DEFAULT_MODE_ID: &str = "transcript";

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
    /// Set by `start_meeting`; carries everything the `ws` layer
    /// needs to insert the meetings row in SQLite.
    pub created_meeting: Option<NewMeetingRecord>,
    /// Set by `stop_meeting`; carries the closing id + timestamp
    /// so the `ws` layer can update `ended_at`.
    pub closed_meeting: Option<ClosedMeetingRecord>,
    /// Pending moment to persist (set by `mark_moment` when a
    /// meeting is active). Carries everything the `ws` layer needs
    /// to write a moments row without re-grabbing the state lock.
    pub mark_moment: Option<MomentRequest>,
}

/// Snapshot of a freshly-started meeting that the persistence layer
/// will insert. Built in `handle_start_meeting` and consumed by the
/// `ws` persistence path. Description is cloned out of the intent
/// before being moved to extraction; metadata is the post-merge map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMeetingRecord {
    pub id: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub description: Option<String>,
    pub metadata: HashMap<String, String>,
}

/// Snapshot of a freshly-closed meeting. The `ws` layer flips
/// `ended_at` on the matching `meetings.id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedMeetingRecord {
    pub id: String,
    pub ended_at: chrono::DateTime<chrono::Utc>,
}

/// One row's worth of `moments` insert data. Built in
/// `handle_mark_moment` and consumed by the `ws` persistence path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MomentRequest {
    pub meeting_id: String,
    pub t: u64,
    pub note: Option<String>,
}

/// In-memory representation of an unfinished meeting recovered from
/// disk on server boot. Built by `db::find_active_meeting` +
/// `persistence::read_transcription` and consumed by
/// `UserState::rehydrate_from_recovered_meeting`.
#[derive(Debug, Clone)]
pub struct RecoveredMeeting {
    pub id: String,
    pub description: Option<String>,
    pub metadata: HashMap<String, String>,
    /// Wall-clock start time. Stored only for display / log; the
    /// in-memory `meeting_started_at: Instant` gets stamped fresh
    /// at recovery time since `Instant` can't be reconstructed.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// All previously-committed transcript items, replayed from the
    /// per-meeting JSONL blob. Empty when the meeting had no
    /// committed transcript before the crash.
    pub transcript_items: Vec<Item>,
}

pub struct UserState {
    pub(crate) meeting_state: MeetingState,
    pub(crate) available_modes: Vec<ModeOption>,
    pub(crate) current_mode: String,
    pub(crate) items_per_mode: HashMap<String, Vec<Item>>,
    pub(crate) metadata: HashMap<String, String>,
    pub(crate) meeting_started_at: Option<Instant>,
    pub(crate) rolling_transcript: Vec<TranscriptChunk>,
    /// Memories recalled from mnemo at the start of the current meeting,
    /// shared with the LLM summarizers as a "Prior context" preamble.
    /// `None` until recall completes (or if mnemo is disabled / failed).
    pub(crate) recalled_context: Option<crate::mnemo::RecalledContext>,
    /// Registered devices keyed by `connection_id` (a UUID minted per
    /// WS connection). Phase 2b is in-memory only — disconnect removes
    /// the entry; Phase 4 adds a persistent `devices` table that
    /// preserves entries across reconnects.
    pub(crate) devices_by_connection: HashMap<String, crate::contract::Device>,
    /// Device that's currently feeding audio into the meeting.
    /// `None` until a meeting starts and a `/audio` client is bound.
    pub(crate) audio_source_device_id: Option<String>,
    /// UUID of the active meeting in the persistence layer. Set in
    /// `handle_start_meeting`, cleared in `handle_stop_meeting`. The
    /// in-memory equivalent of the `meetings.id` row in SQLite —
    /// kept here so `mark_moment` (and future intents that need to
    /// reference the meeting) can attach to it without touching I/O.
    pub(crate) current_meeting_id: Option<String>,
}

impl UserState {
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
            rolling_transcript: Vec::new(),
            recalled_context: None,
            devices_by_connection: HashMap::new(),
            audio_source_device_id: None,
            current_meeting_id: None,
        };
        s.assert_invariants();
        s
    }

    /// Rehydrate from a meeting that was active when the server last
    /// stopped. Called once at boot, before any clients connect.
    /// Wipes any in-memory state to avoid mixing fresh and recovered
    /// fields, then re-installs the recovered ones.
    ///
    /// Note: `meeting_started_at` is stamped with `Instant::now()`
    /// rather than the wall-clock `recovered.started_at` — `Instant`
    /// is monotonic and process-local so we can't reconstruct it.
    /// Anywhere that wants a true wall-clock start consults the DB
    /// row directly (`db::find_active_meeting`), not this field.
    pub fn rehydrate_from_recovered_meeting(&mut self, r: &RecoveredMeeting) {
        self.meeting_state = MeetingState::Active;
        self.current_meeting_id = Some(r.id.clone());
        self.metadata = r.metadata.clone();
        self.meeting_started_at = Some(Instant::now());
        self.current_mode = DEFAULT_MODE_ID.to_string();
        // Replay transcript items into transcript-mode. Other modes
        // start empty — their summarizers will re-derive items as
        // new transcript chunks come in.
        self.items_per_mode
            .insert(DEFAULT_MODE_ID.to_string(), r.transcript_items.clone());
        self.rolling_transcript.clear();
        self.recalled_context = None;
        self.assert_invariants();
    }

    pub fn snapshot(&self) -> Event {
        Event::Snapshot {
            protocol_version: PROTOCOL_VERSION,
            meeting_state: self.meeting_state,
            meeting_id: self.current_meeting_id.clone(),
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
            prior_context: self.recalled_context.as_ref().map(|c| c.summary()),
            devices: self.devices_by_connection.values().cloned().collect(),
            audio_source_device_id: self.audio_source_device_id.clone(),
        }
    }

    /// Register (or re-register) a device under the given WS
    /// connection. Returns the assigned device.
    pub fn register_device(
        &mut self,
        connection_id: String,
        hostname: String,
        capabilities: Vec<crate::contract::Capability>,
    ) -> crate::contract::Device {
        let device = crate::contract::Device {
            id: uuid::Uuid::new_v4().to_string(),
            hostname,
            capabilities,
            online: true,
        };
        self.devices_by_connection
            .insert(connection_id, device.clone());
        device
    }

    /// Remove a device when its WS connection closes. Returns the
    /// removed device (for diagnostics) if there was one.
    pub fn unregister_device(&mut self, connection_id: &str) -> Option<crate::contract::Device> {
        let removed = self.devices_by_connection.remove(connection_id);
        // If this device was bound as the audio source, drop the binding.
        if let Some(d) = &removed {
            if self.audio_source_device_id.as_deref() == Some(&d.id) {
                self.audio_source_device_id = None;
            }
        }
        removed
    }

    /// Snapshot of all currently-registered devices.
    pub fn devices_clone(&self) -> Vec<crate::contract::Device> {
        self.devices_by_connection.values().cloned().collect()
    }

    pub fn apply_intent(&mut self, intent: Intent) -> IntentOutcome {
        let mut outcome = IntentOutcome::default();
        match intent {
            Intent::StartMeeting {
                description,
                metadata,
                audio_source_device_id,
            } => {
                self.handle_start_meeting(
                    description,
                    metadata,
                    audio_source_device_id,
                    &mut outcome,
                );
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
            Intent::ExtractMetadata { description } => {
                self.handle_extract_metadata(description, &mut outcome)
            }
            Intent::MarkMoment { t, note } => self.handle_mark_moment(t, note, &mut outcome),
            Intent::ExpandItem { item_id } => self.handle_expand_item(item_id, &mut outcome),
            Intent::RegisterDevice { .. } => {
                // Handled in ws.rs because it needs the per-connection
                // identity (only ws.rs has it). This arm exists so the
                // match stays exhaustive without us adding a fake outcome.
                tracing::warn!("RegisterDevice reached apply_intent — should be handled in ws.rs");
            }
        }
        self.assert_invariants();
        outcome
    }

    fn handle_start_meeting(
        &mut self,
        description: Option<String>,
        metadata: Option<HashMap<String, String>>,
        audio_source_device_id: Option<String>,
        outcome: &mut IntentOutcome,
    ) {
        if !matches!(self.meeting_state, MeetingState::Idle) {
            tracing::warn!(state = ?self.meeting_state, "start_meeting in invalid state");
            return;
        }
        self.meeting_state = MeetingState::Active;
        self.meeting_started_at = Some(Instant::now());
        // Mint the meeting id up front. Held in state so `mark_moment`
        // can reference it without I/O; surfaced through the outcome
        // so the `ws` layer can persist the meetings row.
        let meeting_id = uuid::Uuid::new_v4().to_string();
        self.current_meeting_id = Some(meeting_id.clone());
        // Preserve any metadata extracted while idle (via ExtractMetadata) when
        // the intent doesn't supply its own. If the intent supplies metadata,
        // it wins (treating the client as the source of truth at start time).
        if let Some(m) = metadata {
            self.metadata = m;
        }
        self.current_mode = DEFAULT_MODE_ID.to_string();

        // Bind the audio source if the caller provided one. We don't
        // validate the device exists or has audio_capture today —
        // worst case the meeting runs silent. (Future: 400-equivalent
        // error when the device id is unknown.)
        self.audio_source_device_id = audio_source_device_id.clone();

        outcome.events.push(Event::MeetingStateChanged {
            meeting_state: MeetingState::Active,
            meeting_id: self.current_meeting_id.clone(),
        });
        outcome.events.push(Event::MetadataChanged {
            metadata: self.metadata.clone(),
        });
        outcome.events.push(Event::ModeChanged {
            mode: self.current_mode.clone(),
            display_tag: None,
            items: self.items_per_mode[&self.current_mode].clone(),
        });
        // Tell the chosen device (and any clients tracking the
        // binding) to start streaming. Only emit if a source was
        // actually bound — silent meetings with no source skip
        // this event so clients don't see redundant `None` chatter.
        if audio_source_device_id.is_some() {
            outcome.events.push(Event::AudioSourceDeviceChanged {
                device_id: audio_source_device_id,
            });
        }
        outcome.started_meeting = true;
        // Snapshot the description before it's potentially moved to
        // `start_extraction_for` below — we want both paths (DB insert
        // *and* extraction) to see the same value.
        let description_for_record = description.as_ref().filter(|s| !s.is_empty()).cloned();
        outcome.created_meeting = Some(NewMeetingRecord {
            id: meeting_id,
            started_at: chrono::Utc::now(),
            description: description_for_record,
            metadata: self.metadata.clone(),
        });
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
        self.rolling_transcript.clear();
        self.recalled_context = None;
        // Surface the closing id to the persistence layer before
        // clearing it locally — the `ws` handler reads
        // `outcome.closed_meeting_id` to set `ended_at` in SQLite.
        let closing_id = self.current_meeting_id.take();
        let had_audio_source = self.audio_source_device_id.take().is_some();

        outcome.events.push(Event::MeetingStateChanged {
            meeting_state: MeetingState::Idle,
            meeting_id: None,
        });
        // Tell the chosen device to stop streaming. Only emit if
        // there *was* a binding — otherwise we'd send a redundant
        // None on every plain stop.
        if had_audio_source {
            outcome
                .events
                .push(Event::AudioSourceDeviceChanged { device_id: None });
        }
        outcome.stopped_meeting = true;
        if let Some(id) = closing_id {
            outcome.closed_meeting = Some(ClosedMeetingRecord {
                id,
                ended_at: chrono::Utc::now(),
            });
        }
    }

    fn handle_pause(&mut self, outcome: &mut IntentOutcome) {
        if !matches!(self.meeting_state, MeetingState::Active) {
            tracing::warn!(state = ?self.meeting_state, "pause in invalid state");
            return;
        }
        self.meeting_state = MeetingState::Paused;
        outcome.events.push(Event::MeetingStateChanged {
            meeting_state: MeetingState::Paused,
            meeting_id: self.current_meeting_id.clone(),
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
            meeting_id: self.current_meeting_id.clone(),
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

    /// Trigger metadata extraction without changing meeting state. The
    /// extraction runs asynchronously (ws.rs spawns it via
    /// `outcome.start_extraction_for`); the resulting metadata is merged
    /// with any manual edits and pushed back via `MetadataChanged`.
    fn handle_extract_metadata(&mut self, description: String, outcome: &mut IntentOutcome) {
        if description.is_empty() {
            return;
        }
        outcome.start_extraction_for = Some(description);
    }

    fn handle_mark_moment(&mut self, t: u64, note: Option<String>, outcome: &mut IntentOutcome) {
        if !matches!(self.meeting_state, MeetingState::Active) {
            tracing::warn!(state = ?self.meeting_state, "mark_moment in invalid state");
            return;
        }
        // `meeting_state == Active` guarantees `current_meeting_id`
        // is Some — both are written atomically in `handle_start_meeting`
        // and cleared atomically in `handle_stop_meeting`.
        let Some(meeting_id) = self.current_meeting_id.clone() else {
            tracing::error!("invariant violation: Active meeting with no current_meeting_id");
            return;
        };
        tracing::info!(t, ?note, meeting_id = %meeting_id, "mark_moment");
        outcome.events.push(Event::Status {
            status: Status {
                listening: true,
                paused: false,
                error: None,
            },
        });
        outcome.mark_moment = Some(MomentRequest {
            meeting_id,
            t,
            note,
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
        outcome.events.push(Event::ItemsUpdate {
            mode: self.current_mode.clone(),
            items: payload,
        });
    }

    /// Append a transcript chunk to the rolling buffer. Silently no-ops
    /// if the meeting is not Active.
    pub fn append_transcript_chunk(&mut self, chunk: TranscriptChunk) {
        if !matches!(self.meeting_state, MeetingState::Active) {
            return;
        }
        self.rolling_transcript.push(chunk);
    }

    /// Return the rolling transcript joined as a single string with
    /// newlines between chunks. Empty string if no chunks accumulated.
    pub fn rolling_transcript_text(&self) -> String {
        self.rolling_transcript
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
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

    pub fn meeting_started_at(&self) -> Option<Instant> {
        self.meeting_started_at
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
        self.recalled_context.clone()
    }

    pub fn set_recalled_context(&mut self, ctx: Option<crate::mnemo::RecalledContext>) {
        self.recalled_context = ctx;
    }

    pub fn set_metadata_full(&mut self, metadata: HashMap<String, String>) {
        // Idle is a valid state for metadata: extraction can run before
        // start_meeting via Intent::ExtractMetadata. The previous
        // "abandon if idle" guard was meant to drop in-flight results when
        // the meeting was stopped mid-extraction; that case is now covered
        // by the cancellation token in spawn_extraction.
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

impl Default for UserState {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// ServerState — per-user UserState map.
//
// Phase B of the OAuth migration. Each authenticated user gets their
// own `UserState` (meeting, items, devices, etc.), keyed by the local
// `users.id`. Lazy-creates an entry on first touch.
//
// External callers (ws.rs, summarizers, mnemo) keep working against
// `ServerState` but must now pass a `user_id` on every method that
// previously operated on the global state.
// ============================================================================

pub struct ServerState {
    /// User-keyed map; `users.id` (UUID) is the key.
    users: HashMap<String, UserState>,
}

impl ServerState {
    pub fn new() -> Self {
        Self {
            users: HashMap::new(),
        }
    }

    /// Borrow a user's state if it exists. Returns `None` for users
    /// who haven't connected yet.
    pub fn user(&self, uid: &str) -> Option<&UserState> {
        self.users.get(uid)
    }

    /// Get-or-create a user's state. Lazy creation means we only
    /// hold state for users who've ever interacted; the map doesn't
    /// grow with every Auth0 user that *could* log in.
    pub fn user_mut(&mut self, uid: &str) -> &mut UserState {
        self.users.entry(uid.to_string()).or_default()
    }

    /// Apply an intent on behalf of `uid`. Lazy-creates the user's
    /// state on first call.
    pub fn apply_intent(&mut self, uid: &str, intent: Intent) -> IntentOutcome {
        self.user_mut(uid).apply_intent(intent)
    }

    /// Render a fresh-connection snapshot for the named user. Users
    /// who've never interacted get the same shape as a brand-new
    /// idle state — no leakage of other users' meetings.
    pub fn snapshot(&mut self, uid: &str) -> Event {
        self.user_mut(uid).snapshot()
    }

    pub fn register_device(
        &mut self,
        uid: &str,
        connection_id: String,
        hostname: String,
        capabilities: Vec<crate::contract::Capability>,
    ) -> crate::contract::Device {
        self.user_mut(uid)
            .register_device(connection_id, hostname, capabilities)
    }

    /// Removes a connection's device from whichever user owns it.
    /// Returns `(user_id, device)` if found — callers fan out a
    /// `DevicesChanged` to that user only.
    pub fn unregister_connection(
        &mut self,
        connection_id: &str,
    ) -> Option<(String, crate::contract::Device)> {
        for (uid, u) in self.users.iter_mut() {
            if let Some(d) = u.unregister_device(connection_id) {
                return Some((uid.clone(), d));
            }
        }
        None
    }

    /// Devices visible to the given user.
    pub fn devices_clone_for(&self, uid: &str) -> Vec<crate::contract::Device> {
        self.users
            .get(uid)
            .map(|u| u.devices_clone())
            .unwrap_or_default()
    }

    /// Currently-bound audio source device for the user (if any).
    pub fn audio_source_device_id_for(&self, uid: &str) -> Option<String> {
        self.users
            .get(uid)
            .and_then(|u| u.audio_source_device_id.clone())
    }

    /// Rolling transcript text for the user (mnemo pusher uses it).
    pub fn rolling_transcript_text_for(&self, uid: &str) -> Option<String> {
        self.users.get(uid).map(|u| u.rolling_transcript_text())
    }

    /// Append a transcript chunk to the named user's rolling buffer
    /// AND push the resulting Item into transcript-mode. Returns the
    /// payload the summarizer should broadcast (full list for
    /// Replace strategy, just the new item for Append).
    pub fn append_transcript_chunk_for(
        &mut self,
        uid: &str,
        chunk: TranscriptChunk,
        item: Item,
    ) -> Vec<Item> {
        let u = self.user_mut(uid);
        u.append_transcript_chunk(chunk);
        u.push_item_for_mode("transcript", item)
    }

    /// True if *any* user has an active meeting. Used by the heartbeat
    /// task that emits global Status broadcasts. Per-user Status
    /// events are emitted separately in the intent path.
    pub fn any_meeting_active(&self) -> bool {
        self.users
            .values()
            .any(|u| matches!(u.meeting_state, MeetingState::Active))
    }

    pub fn any_meeting_paused(&self) -> bool {
        self.users
            .values()
            .any(|u| matches!(u.meeting_state, MeetingState::Paused))
    }

    /// Iterate (uid, state) for boot recovery and bookkeeping.
    pub fn user_ids(&self) -> Vec<String> {
        self.users.keys().cloned().collect()
    }

    /// Rehydrate a user's state from a recovered meeting at boot.
    pub fn rehydrate_user_from_recovered(&mut self, uid: &str, recovered: &RecoveredMeeting) {
        self.user_mut(uid)
            .rehydrate_from_recovered_meeting(recovered);
    }

    /// Look up which user owns a given device id (for routing
    /// targeted events like CaptureMomentScreenshot). Returns None
    /// if the device isn't registered to any user.
    pub fn find_user_and_connection_for_device(&self, device_id: &str) -> Option<(String, String)> {
        for (uid, u) in self.users.iter() {
            for (conn_id, dev) in u.devices_by_connection.iter() {
                if dev.id == device_id {
                    return Some((uid.clone(), conn_id.clone()));
                }
            }
        }
        None
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

    fn push_item(s: &mut UserState, mode: &str, id: &str, text: &str) {
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
        let s = UserState::new();
        assert!(matches!(s.meeting_state, MeetingState::Idle));
    }

    #[test]
    fn rehydrate_from_recovered_meeting_installs_state() {
        let mut s = UserState::new();
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
        };
        s.rehydrate_from_recovered_meeting(&recovered);

        assert!(matches!(s.meeting_state, MeetingState::Active));
        assert_eq!(s.current_meeting_id.as_deref(), Some("rec-1"));
        assert_eq!(s.metadata.get("project"), Some(&"helix".to_string()));
        assert!(s.meeting_started_at.is_some());
        assert_eq!(s.current_mode, DEFAULT_MODE_ID);
        let items = s.items_per_mode.get(DEFAULT_MODE_ID).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "hello world");
    }

    #[test]
    fn new_has_four_default_modes() {
        let s = UserState::new();
        assert_eq!(s.available_modes.len(), 4);
        assert_eq!(s.available_modes[0].id, "highlights");
        assert_eq!(s.available_modes[1].id, "transcript");
        assert_eq!(s.available_modes[2].id, "actions");
        assert_eq!(s.available_modes[3].id, "open_questions");
    }

    #[test]
    fn new_has_empty_items_per_mode() {
        let s = UserState::new();
        for mode in &s.available_modes {
            assert_eq!(s.items_per_mode[&mode.id].len(), 0);
        }
    }

    #[test]
    fn new_default_current_mode_is_transcript() {
        let s = UserState::new();
        assert_eq!(s.current_mode, "transcript");
    }

    #[test]
    fn snapshot_initial_state() {
        let s = UserState::new();
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
            } => {
                assert_eq!(protocol_version, PROTOCOL_VERSION);
                assert!(matches!(meeting_state, MeetingState::Idle));
                assert!(
                    meeting_id.is_none(),
                    "idle snapshot should not carry a meeting_id"
                );
                assert_eq!(available_modes.len(), 4);
                assert_eq!(mode, "transcript");
                assert!(display_tag.is_none());
                assert!(metadata.is_empty());
                assert!(items.is_empty());
                assert!(prior_context.is_none());
                assert!(devices.is_empty());
                assert!(audio_source_device_id.is_none());
                assert!(!status.listening);
                assert!(!status.paused);
                assert!(status.error.is_none());
            }
            e => panic!("expected snapshot, got {:?}", e),
        }
    }

    #[test]
    fn start_meeting_from_idle() {
        let mut s = UserState::new();
        let out = s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: Some(HashMap::from([("project".into(), "helix".into())])),
            audio_source_device_id: None,
        });
        assert!(matches!(s.meeting_state, MeetingState::Active));
        assert_eq!(s.metadata.get("project"), Some(&"helix".into()));
        assert_eq!(out.events.len(), 3);
        assert!(matches!(
            out.events[0],
            Event::MeetingStateChanged {
                meeting_state: MeetingState::Active,
                meeting_id: Some(_),
            }
        ));
        assert!(matches!(out.events[1], Event::MetadataChanged { .. }));
        assert!(matches!(out.events[2], Event::ModeChanged { .. }));
        assert!(out.started_meeting);
        assert!(out.start_extraction_for.is_none());
    }

    #[test]
    fn start_meeting_with_description_signals_extraction() {
        let mut s = UserState::new();
        let out = s.apply_intent(Intent::StartMeeting {
            description: Some("Q1 budget review".into()),
            metadata: None,
            audio_source_device_id: None,
        });
        assert_eq!(
            out.start_extraction_for.as_deref(),
            Some("Q1 budget review")
        );
    }

    #[test]
    fn start_meeting_when_active_is_noop() {
        let mut s = UserState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
        });
        let out = s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
        });
        assert!(out.events.is_empty());
        assert!(!out.started_meeting);
        assert!(matches!(s.meeting_state, MeetingState::Active));
    }

    #[test]
    fn stop_meeting_from_active() {
        let mut s = UserState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: Some(HashMap::from([("k".into(), "v".into())])),
            audio_source_device_id: None,
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
        let mut s = UserState::new();
        let out = s.apply_intent(Intent::StopMeeting);
        assert!(out.events.is_empty());
        assert!(!out.stopped_meeting);
    }

    #[test]
    fn pause_from_active() {
        let mut s = UserState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
        });
        let out = s.apply_intent(Intent::Pause);
        assert!(matches!(s.meeting_state, MeetingState::Paused));
        assert_eq!(out.events.len(), 1);
        assert!(out.paused_meeting);
    }

    #[test]
    fn pause_when_idle_or_paused_is_noop() {
        let mut s = UserState::new();
        let out = s.apply_intent(Intent::Pause);
        assert!(out.events.is_empty());

        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
        });
        s.apply_intent(Intent::Pause);
        let out2 = s.apply_intent(Intent::Pause);
        assert!(out2.events.is_empty());
    }

    #[test]
    fn resume_from_paused() {
        let mut s = UserState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
        });
        s.apply_intent(Intent::Pause);
        let out = s.apply_intent(Intent::Resume);
        assert!(matches!(s.meeting_state, MeetingState::Active));
        assert!(out.resumed_meeting);
    }

    #[test]
    fn resume_when_idle_or_active_is_noop() {
        let mut s = UserState::new();
        let out = s.apply_intent(Intent::Resume);
        assert!(out.events.is_empty());

        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
        });
        let out2 = s.apply_intent(Intent::Resume);
        assert!(out2.events.is_empty());
    }

    #[test]
    fn set_mode_valid() {
        let mut s = UserState::new();
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
        let mut s = UserState::new();
        let out = s.apply_intent(Intent::SetMode {
            mode: "bogus".into(),
        });
        assert_eq!(s.current_mode, "transcript");
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
        let mut s = UserState::new();
        let out = s.apply_intent(Intent::SetMode {
            mode: "actions".into(),
        });
        assert_eq!(s.current_mode, "actions");
        assert_eq!(out.events.len(), 1);
    }

    #[test]
    fn set_metadata_insert() {
        let mut s = UserState::new();
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
        let mut s = UserState::new();
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
        let mut s = UserState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
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
        let mut s = UserState::new();
        let out = s.apply_intent(Intent::MarkMoment { t: 0, note: None });
        assert!(out.events.is_empty());
    }

    #[test]
    fn expand_item_append_strategy_returns_single_item() {
        let mut s = UserState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
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
            [Event::ItemsUpdate { mode, items }] => {
                assert_eq!(mode, "transcript");
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].id, "i2");
                assert!(items[0].detail.is_some());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn expand_item_replace_strategy_returns_full_list() {
        let mut s = UserState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
        });
        s.apply_intent(Intent::SetMode {
            mode: "highlights".into(),
        });
        push_item(&mut s, "highlights", "h1", "first");
        push_item(&mut s, "highlights", "h2", "second");

        let out = s.apply_intent(Intent::ExpandItem {
            item_id: "h1".into(),
        });
        match &out.events[..] {
            [Event::ItemsUpdate { mode, items }] => {
                assert_eq!(mode, "highlights");
                assert_eq!(items.len(), 2);
                assert!(items[0].detail.is_some());
                assert!(items[1].detail.is_none());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn expand_item_unknown_emits_error() {
        let mut s = UserState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
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

    #[test]
    fn rolling_transcript_appends_chunks() {
        let mut s = UserState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
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
        let mut s = UserState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
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
        let mut s = UserState::new();
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
        let mut s = UserState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
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
        let mut s = UserState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
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
        let mut s = UserState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
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
        let mut s = UserState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
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
        let mut s = UserState::new();
        s.apply_intent(Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
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
        let mut s = UserState::new();
        let device = s.register_device(
            "conn-1".into(),
            "tiago-laptop".into(),
            vec![
                crate::contract::Capability::AudioCapture,
                crate::contract::Capability::SystemAudio,
            ],
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
        let mut s = UserState::new();
        let device = s.register_device(
            "conn-1".into(),
            "tiago-laptop".into(),
            vec![crate::contract::Capability::AudioCapture],
        );
        assert_eq!(s.devices_clone().len(), 1);

        let removed = s.unregister_device("conn-1");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id, device.id);
        assert!(s.devices_clone().is_empty());
    }

    #[test]
    fn unregister_device_clears_audio_binding_if_match() {
        let mut s = UserState::new();
        let device = s.register_device(
            "conn-1".into(),
            "tiago-laptop".into(),
            vec![crate::contract::Capability::AudioCapture],
        );
        s.audio_source_device_id = Some(device.id.clone());

        s.unregister_device("conn-1");
        assert!(
            s.audio_source_device_id.is_none(),
            "audio binding should clear when bound device unregisters"
        );
    }

    #[test]
    fn unregister_device_keeps_audio_binding_if_different() {
        let mut s = UserState::new();
        let other = s.register_device(
            "conn-other".into(),
            "other-mac".into(),
            vec![crate::contract::Capability::AudioCapture],
        );
        let _ = s.register_device(
            "conn-self".into(),
            "tiago-laptop".into(),
            vec![crate::contract::Capability::AudioCapture],
        );
        s.audio_source_device_id = Some(other.id.clone());

        s.unregister_device("conn-self");
        assert_eq!(
            s.audio_source_device_id.as_deref(),
            Some(other.id.as_str()),
            "audio binding should persist when an unrelated device disconnects"
        );
    }

    #[test]
    fn snapshot_includes_devices_and_audio_binding() {
        let mut s = UserState::new();
        let d = s.register_device(
            "conn-1".into(),
            "host".into(),
            vec![crate::contract::Capability::ControlSurface],
        );
        s.audio_source_device_id = Some(d.id.clone());

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
