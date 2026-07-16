//! Session registry and shared session types.
//!
//! `SessionRegistry` owns the per-user `UserSession` map.
//! Shared value types (`IntentOutcome`, `NewMeetingRecord`, etc.) live here
//! so every sub-module can import them from `super::`.

pub mod devices;
pub mod intents;
pub mod meeting;
pub mod snapshot;
pub mod user;

pub use meeting::MeetingRuntime;
pub use user::UserSession;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::audio::RemoteAudioSource;
use crate::protocol::{Intent, Item, MeetingState, ModeOption, UpdateStrategy};
use crate::stt::TranscriptChunk;

pub fn default_modes() -> Vec<ModeOption> {
    vec![
        ModeOption {
            // `assist` — proactive contextual suggestions surfaced by
            // the summarizer agent during a live meeting (term
            // definitions, draft answers to questions, memory
            // recalls, talking-point hints). Append strategy: the
            // agent adds items over the course of the meeting and
            // they accumulate as a feed. Sits FIRST in the catalog
            // so it lands leftmost in companion mode pickers — this
            // is the "what does the agent want me to know right
            // now" surface, intentionally the prime spot.
            id: "assist".into(),
            label: "Assist".into(),
            update_strategy: UpdateStrategy::Append,
        },
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
        ModeOption {
            id: "summary".into(),
            label: "Summary".into(),
            update_strategy: UpdateStrategy::Replace,
        },
        ModeOption {
            id: "chat".into(),
            label: "Chat".into(),
            // Append so the live overlay shows a scrolling thread
            // and DB persistence keeps every Q+A turn (Replace
            // would clobber prior turns on each new exchange).
            // Auto-scroll-to-bottom on the live UI keeps the
            // request-response feel without dropping history.
            update_strategy: UpdateStrategy::Append,
        },
        ModeOption {
            id: "quick_asks".into(),
            label: "Quick Asks".into(),
            // Replace: the items in this mode are the *whole*
            // current state of the user's quick-ask library. Any
            // CRUD broadcasts the full set, so the receiver swaps
            // items wholesale rather than appending.
            update_strategy: UpdateStrategy::Replace,
        },
    ]
}

pub const DEFAULT_MODE_ID: &str = "transcript";

/// Modes that exist in the catalog (so items_per_mode buckets keep
/// working + the agent + extractors + persistence + past-meeting
/// views all see them) but are *hidden from the live mode picker*
/// on every client. These are wrap-up surfaces — useful when
/// reviewing a finished meeting, noise during one.
///
/// Filtered out of the snapshot's `available_modes` field; clients
/// build their mode-cycle / tab pickers from that field, so they
/// effectively disappear from glasses double-tap cycling, PWA tabs,
/// mobile tabs, and the Mac overlay simultaneously. Past-meeting
/// detail views use their own hardcoded mode lists and are
/// unaffected.
pub(super) const LIVE_MODE_EXCLUSIONS: &[&str] = &["actions", "open_questions"];

/// Result of applying an intent. `events` are broadcast in order.
/// `originator_only` is sent only to the originating client — used
/// for protocol errors (`Event::Error`) and also for defense-in-depth
/// state re-broadcasts (e.g., a duplicate `start_meeting` re-echoes
/// the current `MeetingStateChanged { Active }` to the racing client
/// so its UI lands on the live meeting view).
#[derive(Default)]
pub struct IntentOutcome {
    pub events: Vec<crate::protocol::Event>,
    pub originator_only: Option<crate::protocol::Event>,
    pub start_extraction_for: Option<String>,
    pub started_meeting: bool,
    pub stopped_meeting: bool,
    /// Set by `start_meeting`; carries everything the `ws` layer
    /// needs to insert the meetings row in SQLite.
    pub created_meeting: Option<NewMeetingRecord>,
    /// Set by `stop_meeting`; carries the closing id + timestamp
    /// so the `ws` layer can update `ended_at`.
    pub closed_meeting: Option<ClosedMeetingRecord>,
    /// Set by `stop_meeting` when the meeting had transcript content;
    /// kicks off the post-meeting wrap-up extractor that generates
    /// actions + open_questions from the full transcript. None when
    /// there's nothing to extract (empty transcript / canceled-early
    /// meeting).
    pub start_wrap_up: Option<WrapUpRequest>,
    /// Pending moment to persist (set by `mark_moment` when a
    /// meeting is active). Carries everything the `ws` layer needs
    /// to write a moments row without re-grabbing the state lock.
    pub mark_moment: Option<MomentRequest>,
    /// Set by `stop_meeting`; the runtime is carried out so the ws
    /// layer can hand it to a detached `workers::finalize::run` task.
    /// That task drains the STT pipeline, runs wrap-up on the complete
    /// transcript, then calls `shutdown()` — all outside the registry
    /// lock so it never deadlocks against tasks that need the lock.
    pub stopped_runtime: Option<MeetingRuntime>,
    /// Set by `set_assist_sensitivity` when the value actually
    /// changed. The ws layer reads this and writes the new value
    /// into `meetings.assist_sensitivity` so a future reconnect
    /// observes the latest user choice.
    pub assist_sensitivity_persist: Option<AssistSensitivityPersist>,
}

/// Mid-meeting sensitivity change that needs to land in the DB.
/// Built in `handle_set_assist_sensitivity`; consumed by the ws
/// layer alongside the broadcast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistSensitivityPersist {
    pub meeting_id: String,
    pub value: crate::protocol::AssistSensitivity,
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
    /// Active sensitivity at meeting-create time. The DB column is
    /// nullable; we still write the explicit canonical string so a
    /// later mid-meeting flip via `SetAssistSensitivity` overwrites
    /// a concrete value rather than racing against NULL semantics.
    pub assist_sensitivity: crate::protocol::AssistSensitivity,
}

/// Snapshot of a freshly-closed meeting. The `ws` layer flips
/// `ended_at` on the matching `meetings.id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedMeetingRecord {
    pub id: String,
    pub ended_at: chrono::DateTime<chrono::Utc>,
}

/// Asks the ws layer to spawn the post-meeting actions +
/// open_questions extractor after applying a `stop_meeting`. The
/// transcript is captured here (BEFORE the in-memory items_per_mode
/// is cleared in `handle_stop_meeting`) so the extractor has the
/// full text to work with without re-reading from the DB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapUpRequest {
    pub meeting_id: String,
    /// Joined transcript text — one item per line, in the order the
    /// items were captured. Speaker labels / timestamps are NOT
    /// included here (the LLM extracts intent, not provenance).
    pub transcript_text: String,
}

/// One row's worth of `moments` insert data. Built in
/// `handle_mark_moment` and consumed by the `ws` persistence path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MomentRequest {
    pub meeting_id: String,
    pub t: u64,
    pub note: Option<String>,
    /// Client-supplied moment id (validated by the caller). `None` =>
    /// the server generates one.
    pub id: Option<String>,
    /// Marking client will upload its own image; skip the screen_capture
    /// delegation.
    pub self_capture: bool,
}

/// In-memory representation of an unfinished meeting recovered from
/// disk on server boot. Built by `db::find_active_meeting` +
/// `persistence::read_transcription` and consumed by
/// `UserSession::rehydrate_from_recovered_meeting`.
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
    /// Assist sensitivity recovered from the meeting row. Defaults
    /// applied at the load site (NULL / unknown → `Moderate`).
    pub assist_sensitivity: crate::protocol::AssistSensitivity,
}

// ============================================================================
// SessionRegistry — per-user UserSession map.
//
// Each authenticated user gets their own `UserSession` (meeting, items,
// devices, etc.), keyed by the local `users.id`. Lazy-creates an entry on
// first touch.
//
// External callers (ws.rs, summarizers, mnemo) keep working against
// `SessionRegistry` but must now pass a `user_id` on every method that
// previously operated on the global state.
// ============================================================================

pub struct SessionRegistry {
    /// User-keyed map; `users.id` (UUID) is the key.
    users: HashMap<String, UserSession>,
    /// Per-user `/audio` liveness, keyed by `users.id`. `lost_at` is
    /// armed when the user's audio source went away while their
    /// meeting was still active (and seeded at boot for recovered
    /// meetings); cleared on `/audio` (re)connect and on meeting
    /// start/stop. `generation` is bumped on every `/audio` accept so
    /// the LATE close of a replaced socket — which carries a stale
    /// generation — can never arm the timer (improvement #10, hole a).
    /// The liveness reaper ends a meeting whose audio has been gone
    /// past the grace window — see `stale_audio_meetings`.
    /// `lost_at == None` = audio is live (or nothing is armed).
    audio: HashMap<String, AudioLiveness>,
}

/// See the `audio` field doc on `SessionRegistry`.
#[derive(Default)]
struct AudioLiveness {
    generation: u64,
    lost_at: Option<Instant>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            users: HashMap::new(),
            audio: HashMap::new(),
        }
    }

    /// True if `uid` currently has an active meeting.
    pub fn is_meeting_active(&self, uid: &str) -> bool {
        self.users
            .get(uid)
            .map(|u| matches!(u.meeting_state, MeetingState::Active))
            .unwrap_or(false)
    }

    /// The user's `/audio` stream (re)connected — audio is flowing, so
    /// clear any pending loss timer and bump the socket generation.
    /// Returns the new generation; the socket task passes it back to
    /// `mark_audio_disconnected` at close so a replaced socket's late
    /// close can't arm the timer. Safe to call when none is set.
    pub fn mark_audio_connected(&mut self, uid: &str) -> u64 {
        let e = self.audio.entry(uid.to_string()).or_default();
        e.generation += 1;
        e.lost_at = None;
        e.generation
    }

    /// The user's `/audio` stream dropped. Only arms the loss timer if
    /// a meeting is actually active (a drop with no meeting is a no-op
    /// — nothing to reap) AND `gen` is still the latest generation (a
    /// stale close of an already-replaced socket is a no-op).
    /// Idempotent: keeps the FIRST loss time so a flapping socket
    /// doesn't keep resetting the grace window.
    pub fn mark_audio_disconnected(&mut self, uid: &str, gen: u64) {
        if !self.is_meeting_active(uid) {
            return;
        }
        if let Some(e) = self.audio.get_mut(uid) {
            if e.generation == gen && e.lost_at.is_none() {
                e.lost_at = Some(Instant::now());
            }
        }
    }

    /// Boot-recovery seeding: a recovered meeting has no `/audio`
    /// client in this process, so arm the loss timer regardless of
    /// generation history. No-op without an active meeting. A
    /// returning client's `/audio` accept (`mark_audio_connected`)
    /// clears it.
    pub fn seed_audio_loss(&mut self, uid: &str) {
        if !self.is_meeting_active(uid) {
            return;
        }
        let e = self.audio.entry(uid.to_string()).or_default();
        if e.lost_at.is_none() {
            e.lost_at = Some(Instant::now());
        }
    }

    /// Drop any loss timer for `uid` (e.g. after the meeting ends).
    /// Keeps the generation counter — only the armed timer is cleared.
    pub fn clear_audio_loss(&mut self, uid: &str) {
        if let Some(e) = self.audio.get_mut(uid) {
            e.lost_at = None;
        }
    }

    /// True if `uid`'s meeting is active AND its audio source has been
    /// gone at least `timeout`. The reaper re-checks this under the
    /// lock right before ending a meeting, so a client that reconnects
    /// `/audio` (clearing the timer) in the gap between the sweep and
    /// the reap is never wrongly ended.
    pub fn is_audio_stale(&self, uid: &str, timeout: Duration) -> bool {
        self.is_meeting_active(uid)
            && self
                .audio
                .get(uid)
                .and_then(|e| e.lost_at)
                .map(|since| since.elapsed() >= timeout)
                .unwrap_or(false)
    }

    /// Users whose active meeting's audio source has been gone at least
    /// `timeout`. These are the abandoned meetings the reaper ends.
    /// Filters on `is_meeting_active` so a stale timer for a meeting
    /// that already ended some other way is ignored.
    pub fn stale_audio_meetings(&self, timeout: Duration) -> Vec<String> {
        self.audio
            .iter()
            .filter_map(|(uid, e)| {
                let since = e.lost_at?;
                (since.elapsed() >= timeout && self.is_meeting_active(uid)).then(|| uid.clone())
            })
            .collect()
    }

    /// Borrow a user's session if it exists. Returns `None` for users
    /// who haven't connected yet.
    pub fn user(&self, uid: &str) -> Option<&UserSession> {
        self.users.get(uid)
    }

    /// Get-or-create a user's session. Lazy creation means we only
    /// hold state for users who've ever interacted; the map doesn't
    /// grow with every Auth0 user that *could* log in.
    pub fn user_mut(&mut self, uid: &str) -> &mut UserSession {
        self.users.entry(uid.to_string()).or_default()
    }

    /// Apply an intent on behalf of `uid`. Lazy-creates the user's
    /// session on first call. Meeting lifecycle edges invalidate any
    /// armed audio-loss timer here, at the single choke point both the
    /// client dispatch and the reaper go through: a fresh start must
    /// not inherit a residual timer from a previous meeting, and after
    /// a stop there is nothing left to reap (improvement #10, hole b).
    pub fn apply_intent(&mut self, uid: &str, intent: Intent) -> IntentOutcome {
        let outcome = self.user_mut(uid).apply_intent(intent);
        if outcome.started_meeting || outcome.stopped_meeting {
            self.clear_audio_loss(uid);
        }
        outcome
    }

    /// Render a fresh-connection snapshot for the named user. Users
    /// who've never interacted get the same shape as a brand-new
    /// idle state — no leakage of other users' meetings.
    pub fn snapshot(&mut self, uid: &str) -> crate::protocol::Event {
        self.user_mut(uid).snapshot()
    }

    pub fn register_device(
        &mut self,
        uid: &str,
        connection_id: String,
        hostname: String,
        capabilities: Vec<crate::protocol::Capability>,
        device_id: Option<String>,
    ) -> crate::protocol::Device {
        self.user_mut(uid)
            .register_device(connection_id, hostname, capabilities, device_id)
    }

    /// Removes a connection's device from whichever user owns it.
    /// Returns `(user_id, device)` if found — callers fan out a
    /// `DevicesChanged` to that user only.
    pub fn unregister_connection(
        &mut self,
        connection_id: &str,
    ) -> Option<(String, crate::protocol::Device)> {
        for (uid, u) in self.users.iter_mut() {
            if let Some(d) = u.unregister_device(connection_id) {
                return Some((uid.clone(), d));
            }
        }
        None
    }

    /// Devices visible to the given user.
    pub fn devices_clone_for(&self, uid: &str) -> Vec<crate::protocol::Device> {
        self.users
            .get(uid)
            .map(|u| u.devices_clone())
            .unwrap_or_default()
    }

    /// Currently-bound audio source device for the user (if any).
    pub fn audio_source_device_id_for(&self, uid: &str) -> Option<String> {
        self.users
            .get(uid)
            .and_then(|u| u.meeting.as_ref())
            .and_then(|m| m.audio_source_device_id.clone())
    }

    /// Rolling transcript text for the user (mnemo pusher uses it).
    pub fn rolling_transcript_text_for(&self, uid: &str) -> Option<String> {
        self.users.get(uid).map(|u| u.rolling_transcript_text())
    }

    /// The user's currently-active meeting id, if any. `None` when the
    /// user is Idle (between meetings).
    pub fn active_meeting_id_for(&self, uid: &str) -> Option<String> {
        self.users
            .get(uid)
            .and_then(|u| u.meeting.as_ref())
            .map(|m| m.meeting_id.clone())
    }

    /// Append a transcript chunk to the named user's rolling buffer AND
    /// push the resulting Item into transcript-mode — but ONLY while
    /// `meeting_id` is still that user's active meeting. Returns the
    /// payload the summarizer should broadcast (full list for Replace
    /// strategy, just the new item for Append), or `None` when the
    /// meeting has since stopped or a *different* meeting is now active.
    ///
    /// The scope check is what keeps a stopped meeting's STT drain tail
    /// from bleeding into the next meeting. `finalize` keeps a meeting-1
    /// summarizer alive for several seconds after stop to flush Soniox's
    /// last buffered utterance; without this guard those late chunks
    /// land in whatever meeting is current — polluting meeting 2's
    /// rolling buffer, its `transcription.jsonl` (the persistence loop
    /// resolves the file by *current* active meeting), and its live UI.
    /// The wrap-up still receives the tail via finalize's own chunk
    /// subscription, so nothing meeting-1 needs is lost.
    pub fn append_transcript_chunk_if_active(
        &mut self,
        uid: &str,
        meeting_id: &str,
        chunk: TranscriptChunk,
        item: Item,
    ) -> Option<Vec<Item>> {
        let is_active = self
            .users
            .get(uid)
            .and_then(|u| u.meeting.as_ref())
            .map(|m| m.meeting_id == meeting_id)
            .unwrap_or(false);
        if !is_active {
            return None;
        }
        let u = self.user_mut(uid);
        u.append_transcript_chunk(chunk);
        Some(u.push_item_for_mode("transcript", item))
    }

    /// Run `f` against the user's session ONLY while `meeting_id` is
    /// still that user's active meeting. The check and the mutation
    /// share one `&mut self` borrow — i.e. one acquisition of the
    /// single registry mutex — so check+act is atomic and a late
    /// agent result (LLM fire that outlived a `stop_meeting`, or a
    /// stop+start race) can never land in idle state or in the NEXT
    /// meeting's buckets. The items-write twin of
    /// `append_transcript_chunk_if_active` above, which solves the
    /// same bug class for the STT drain tail.
    ///
    /// Returns `Some(f(..))` when `meeting_id` matches the user's
    /// active meeting, `None` otherwise (caller should skip any
    /// broadcast / persistence / follow-up kicks).
    pub fn with_session_if_active<T>(
        &mut self,
        uid: &str,
        meeting_id: &str,
        f: impl FnOnce(&mut UserSession) -> T,
    ) -> Option<T> {
        let is_active = self
            .users
            .get(uid)
            .and_then(|u| u.meeting.as_ref())
            .map(|m| m.meeting_id == meeting_id)
            .unwrap_or(false);
        if !is_active {
            return None;
        }
        Some(f(self.user_mut(uid)))
    }

    /// True if *any* user has an active meeting. Used by the heartbeat
    /// task that emits global Status broadcasts. Per-user Status
    /// events are emitted separately in the intent path.
    pub fn any_meeting_active(&self) -> bool {
        self.users
            .values()
            .any(|u| matches!(u.meeting_state, MeetingState::Active))
    }

    /// Iterate (uid, state) for boot recovery and bookkeeping.
    pub fn user_ids(&self) -> Vec<String> {
        self.users.keys().cloned().collect()
    }

    /// Rehydrate a user's session from a recovered meeting at boot.
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

    /// Cancel the active-meeting cancel token for `uid` (if any).
    pub fn cancel_meeting_for(&mut self, uid: &str) {
        if let Some(meeting) = self.users.get_mut(uid).and_then(|u| u.meeting.as_mut()) {
            meeting.cancel.cancel();
        }
    }

    /// Replace this user's extraction-cancel token with a fresh one,
    /// cancelling the previous (if any) so an in-flight extraction
    /// is dropped before the new one fires. Returns a clone of the new
    /// token for the caller to register with the spawned future.
    pub fn extraction_cancel_for(&mut self, uid: &str) -> CancellationToken {
        let user = self.user_mut(uid);
        if let Some(prev) = user.extraction_cancel.take() {
            prev.cancel();
        }
        let t = CancellationToken::new();
        user.extraction_cancel = Some(t.clone());
        t
    }

    /// Cancel and clear any in-flight extraction token for `uid`.
    pub fn cancel_extraction_for(&mut self, uid: &str) {
        if let Some(prev) = self
            .users
            .get_mut(uid)
            .and_then(|u| u.extraction_cancel.take())
        {
            prev.cancel();
        }
    }

    /// Register a JoinHandle with the user's active MeetingRuntime,
    /// so MeetingRuntime::shutdown can await it. No-op if the user has
    /// no active meeting (caller bug, logged).
    pub fn register_meeting_task(&mut self, uid: &str, handle: tokio::task::JoinHandle<()>) {
        match self.users.get_mut(uid).and_then(|u| u.meeting.as_mut()) {
            Some(rt) => rt.register_task(handle),
            None => {
                tracing::error!(
                    user_id = %uid,
                    "register_meeting_task called with no active meeting"
                );
                handle.abort();
            }
        }
    }

    /// Currently-bound audio source for the user's active meeting, if any.
    pub fn audio_source_for_active_meeting(&self, uid: &str) -> Option<Arc<RemoteAudioSource>> {
        self.users
            .get(uid)
            .and_then(|u| u.meeting.as_ref())
            .map(|m| m.audio_source.clone())
    }

    /// Clone of the active-meeting cancel token for `uid`, if any.
    /// Used by `spawn_*` callers that need to register the token with
    /// a spawned task.
    pub fn meeting_cancel_token(&self, uid: &str) -> Option<CancellationToken> {
        self.users
            .get(uid)
            .and_then(|u| u.meeting.as_ref())
            .map(|m| m.cancel.clone())
    }

    /// Clone of the active-meeting graceful-drain token for `uid`, if
    /// any. Handed to the STT spawn site so the provider can drain on
    /// signal (see `workers::finalize`).
    pub fn meeting_drain_token(&self, uid: &str) -> Option<CancellationToken> {
        self.users
            .get(uid)
            .and_then(|u| u.meeting.as_ref())
            .map(|m| m.drain_token())
    }

    /// Clone of the active-meeting reactive-agent cancel token for
    /// `uid`, if any. Handed to the chat + active spawn sites so finalize
    /// can tear those two down without touching STT/summarizer.
    pub fn meeting_reactive_token(&self, uid: &str) -> Option<CancellationToken> {
        self.users
            .get(uid)
            .and_then(|u| u.meeting.as_ref())
            .map(|m| m.reactive_token())
    }

    /// Clone of the active-meeting transcript-chunk sender for `uid`,
    /// if any. The live pipeline uses this instead of allocating its
    /// own channel so the finalize task (which holds the runtime) can
    /// subscribe to the same stream.
    pub fn meeting_chunk_sender(
        &self,
        uid: &str,
    ) -> Option<tokio::sync::broadcast::Sender<TranscriptChunk>> {
        self.users
            .get(uid)
            .and_then(|u| u.meeting.as_ref())
            .map(|m| m.chunk_sender())
    }

    /// Register the STT JoinHandle with the user's active runtime,
    /// kept apart from the general task list so finalize can await it
    /// alone. No-op (logged) if no active meeting.
    pub fn register_meeting_stt_task(&mut self, uid: &str, handle: tokio::task::JoinHandle<()>) {
        match self.users.get_mut(uid).and_then(|u| u.meeting.as_mut()) {
            Some(rt) => rt.set_stt_task(handle),
            None => {
                tracing::error!(
                    user_id = %uid,
                    "register_meeting_stt_task called with no active meeting"
                );
                handle.abort();
            }
        }
    }
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod liveness_tests {
    use super::*;
    use crate::protocol::Intent;

    fn start_meeting(reg: &mut SessionRegistry, uid: &str) {
        reg.apply_intent(
            uid,
            Intent::StartMeeting {
                description: None,
                metadata: None,
                audio_source_device_id: None,
                assist_sensitivity: None,
            },
        );
    }

    #[test]
    fn disconnect_only_tracks_an_active_meeting() {
        let mut reg = SessionRegistry::new();
        // No meeting → a disconnect is a no-op (nothing to reap).
        let g = reg.mark_audio_connected("u1");
        reg.mark_audio_disconnected("u1", g);
        assert!(reg.stale_audio_meetings(Duration::ZERO).is_empty());
        // With an active meeting → tracked, and immediately "stale" at
        // a zero grace window.
        start_meeting(&mut reg, "u1");
        let g = reg.mark_audio_connected("u1");
        reg.mark_audio_disconnected("u1", g);
        assert_eq!(
            reg.stale_audio_meetings(Duration::ZERO),
            vec!["u1".to_string()]
        );
    }

    #[test]
    fn reconnect_clears_the_loss_timer() {
        let mut reg = SessionRegistry::new();
        start_meeting(&mut reg, "u1");
        let g = reg.mark_audio_connected("u1");
        reg.mark_audio_disconnected("u1", g);
        reg.mark_audio_connected("u1");
        assert!(reg.stale_audio_meetings(Duration::ZERO).is_empty());
    }

    #[test]
    fn not_stale_before_the_grace_window() {
        let mut reg = SessionRegistry::new();
        start_meeting(&mut reg, "u1");
        let g = reg.mark_audio_connected("u1");
        reg.mark_audio_disconnected("u1", g);
        // Just disconnected — a long grace window means not yet stale.
        assert!(reg
            .stale_audio_meetings(Duration::from_secs(3600))
            .is_empty());
    }

    #[test]
    fn is_audio_stale_requires_active_meeting_and_elapsed_window() {
        let mut reg = SessionRegistry::new();
        start_meeting(&mut reg, "u1");
        let g = reg.mark_audio_connected("u1");
        reg.mark_audio_disconnected("u1", g);
        // Disconnected + zero window → stale; huge window → not yet.
        assert!(reg.is_audio_stale("u1", Duration::ZERO));
        assert!(!reg.is_audio_stale("u1", Duration::from_secs(3600)));
        // Reconnect clears it → not stale at any window.
        reg.mark_audio_connected("u1");
        assert!(!reg.is_audio_stale("u1", Duration::ZERO));
        // Unknown user → not stale.
        assert!(!reg.is_audio_stale("nobody", Duration::ZERO));
    }

    #[test]
    fn clear_audio_loss_removes_the_timer() {
        let mut reg = SessionRegistry::new();
        start_meeting(&mut reg, "u1");
        let g = reg.mark_audio_connected("u1");
        reg.mark_audio_disconnected("u1", g);
        reg.clear_audio_loss("u1");
        assert!(reg.stale_audio_meetings(Duration::ZERO).is_empty());
    }

    // ── Improvement #10, hole (a): overlapping /audio sockets ──────────

    #[test]
    fn late_close_of_replaced_socket_does_not_arm_timer() {
        let mut reg = SessionRegistry::new();
        start_meeting(&mut reg, "u1");
        // Socket 1 accepted, then socket 2 accepted (replacement, e.g.
        // Wi-Fi→LTE handoff) BEFORE socket 1's close lands.
        let g1 = reg.mark_audio_connected("u1");
        let _g2 = reg.mark_audio_connected("u1");
        // Socket 1's late close carries a stale generation → no-op.
        reg.mark_audio_disconnected("u1", g1);
        assert!(!reg.is_audio_stale("u1", Duration::ZERO));
        assert!(reg.stale_audio_meetings(Duration::ZERO).is_empty());
    }

    #[test]
    fn close_of_latest_socket_arms_timer() {
        let mut reg = SessionRegistry::new();
        start_meeting(&mut reg, "u1");
        let _g1 = reg.mark_audio_connected("u1");
        let g2 = reg.mark_audio_connected("u1");
        // The CURRENT socket closing is a real audio loss → armed.
        reg.mark_audio_disconnected("u1", g2);
        assert!(reg.is_audio_stale("u1", Duration::ZERO));
    }

    // ── Boot-recovery seeding (must keep working) ──────────────────────

    #[test]
    fn boot_seed_arms_timer_and_reconnect_clears() {
        let mut reg = SessionRegistry::new();
        // Seeding a user with no active meeting is a no-op.
        reg.seed_audio_loss("u2");
        assert!(reg.stale_audio_meetings(Duration::ZERO).is_empty());
        // A recovered (active) meeting with no /audio client is armed
        // regardless of generation history...
        start_meeting(&mut reg, "u1");
        reg.seed_audio_loss("u1");
        assert!(reg.is_audio_stale("u1", Duration::ZERO));
        // ...and a returning client's /audio accept clears it.
        reg.mark_audio_connected("u1");
        assert!(!reg.is_audio_stale("u1", Duration::ZERO));
    }

    // ── Improvement #10, hole (b): timer leaking across meetings ───────

    #[test]
    fn stop_meeting_clears_loss_timer() {
        let mut reg = SessionRegistry::new();
        // Meeting 1: audio drops (timer armed with the LATEST gen),
        // then the user stops normally.
        start_meeting(&mut reg, "u1");
        let g = reg.mark_audio_connected("u1");
        reg.mark_audio_disconnected("u1", g);
        reg.apply_intent("u1", Intent::StopMeeting);
        // Meeting 2: silent / PWA-only — no /audio client ever
        // connects, so nothing else would clear a residual timer.
        start_meeting(&mut reg, "u1");
        assert!(!reg.is_audio_stale("u1", Duration::ZERO));
        assert!(reg.stale_audio_meetings(Duration::ZERO).is_empty());
    }

    #[test]
    fn start_meeting_clears_residual_timer() {
        let mut reg = SessionRegistry::new();
        // A timer armed via boot seeding while a meeting was active...
        start_meeting(&mut reg, "u1");
        reg.seed_audio_loss("u1");
        assert!(reg.is_audio_stale("u1", Duration::ZERO));
        // ...must not survive into the NEXT meeting.
        reg.apply_intent("u1", Intent::StopMeeting);
        start_meeting(&mut reg, "u1");
        assert!(!reg.is_audio_stale("u1", Duration::ZERO));
        assert!(reg.stale_audio_meetings(Duration::ZERO).is_empty());
    }
}

#[cfg(test)]
mod conditional_write_tests {
    use super::*;
    use crate::protocol::Intent;

    /// Start a meeting for `uid` and return the freshly-minted
    /// meeting id (handle_start_meeting mints a uuid per meeting).
    fn start_meeting(reg: &mut SessionRegistry, uid: &str) -> String {
        reg.apply_intent(
            uid,
            Intent::StartMeeting {
                description: None,
                metadata: None,
                audio_source_device_id: None,
                assist_sensitivity: None,
            },
        );
        reg.active_meeting_id_for(uid)
            .expect("meeting just started must have an id")
    }

    #[test]
    fn with_session_if_active_runs_for_matching_meeting() {
        let mut reg = SessionRegistry::new();
        let mid = start_meeting(&mut reg, "u1");
        let ran = reg.with_session_if_active("u1", &mid, |u| {
            assert!(matches!(u.meeting_state, MeetingState::Active));
            42
        });
        assert_eq!(ran, Some(42));
    }

    #[test]
    fn with_session_if_active_skips_after_stop() {
        let mut reg = SessionRegistry::new();
        let mid = start_meeting(&mut reg, "u1");
        reg.apply_intent("u1", Intent::StopMeeting);
        let ran = reg.with_session_if_active("u1", &mid, |_| 42);
        assert_eq!(ran, None, "a stopped meeting's late write must be refused");
    }

    #[test]
    fn with_session_if_active_skips_when_a_different_meeting_is_active() {
        let mut reg = SessionRegistry::new();
        let mid1 = start_meeting(&mut reg, "u1");
        reg.apply_intent("u1", Intent::StopMeeting);
        let mid2 = start_meeting(&mut reg, "u1");
        assert_ne!(mid1, mid2, "uuids must differ");
        // Meeting 1's late write must NOT land in meeting 2 ...
        assert_eq!(reg.with_session_if_active("u1", &mid1, |_| ()), None);
        // ... while meeting 2's own writes go through.
        assert_eq!(reg.with_session_if_active("u1", &mid2, |_| ()), Some(()));
    }

    #[test]
    fn with_session_if_active_skips_for_unknown_user() {
        let mut reg = SessionRegistry::new();
        assert_eq!(reg.with_session_if_active("ghost", "m1", |_| ()), None);
    }
}
