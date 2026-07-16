//! WebSocket message contract. Mirrors `packages/pwa/src/contract.ts`.
//! See `docs/PROTOCOL.md` for the wire reference.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingState {
    Idle,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStrategy {
    Replace,
    Append,
}

/// How aggressively the agent should surface proactive assist
/// suggestions. Per-meeting setting, persisted on the meeting row;
/// controls BOTH the server-side confidence threshold (see
/// `agent::tools::assist::assist_confidence_threshold`) and the
/// nudge inserted into the agent's system prompt at bootstrap
/// (see `agent::bootstrap`). Defaults to `Moderate` when the
/// meeting row's column is NULL or the client omits the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AssistSensitivity {
    /// Lowest threshold + prompt nudge to surface anything mildly
    /// useful. Fires a lot. Best when the user wants the assist
    /// surface to be conversational and reactive.
    Aggressive,
    /// Current/historical behavior. The agent self-rates per the
    /// stock calibration guidance and the server gates at the
    /// historical floor (coach ≥ 85, others ≥ 70).
    #[default]
    Moderate,
    /// Highest threshold + prompt nudge to fire ONLY when the
    /// signal is unmistakable. Few but important suggestions.
    Minimal,
}

impl AssistSensitivity {
    /// Canonical lowercase wire string ("aggressive" / "moderate"
    /// / "minimal"). Used by the DB column + the smoke harness's
    /// JSON inspection.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Aggressive => "aggressive",
            Self::Moderate => "moderate",
            Self::Minimal => "minimal",
        }
    }

    /// Parse from the canonical wire string. Anything we don't
    /// recognise (including NULL/missing) returns `None` so the
    /// caller can apply the default explicitly. Not implementing
    /// `std::str::FromStr` so the signature stays `-> Option<Self>`
    /// instead of `-> Result<Self, _>` — every caller wants the
    /// default-on-failure semantics, not error propagation.
    pub fn parse_wire(s: &str) -> Option<Self> {
        match s {
            "aggressive" => Some(Self::Aggressive),
            "moderate" => Some(Self::Moderate),
            "minimal" => Some(Self::Minimal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeOption {
    pub id: String,
    pub label: String,
    pub update_strategy: UpdateStrategy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub detail: Option<String>,
    pub t: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub meta: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub listening: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

/// Counts of memories recalled from mnemo for the current meeting.
/// Drives a PWA badge so the user can confirm the LLM extractors have
/// prior context available.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PriorContextSummary {
    pub preferences: usize,
    pub facts: usize,
    pub episodes: usize,
    pub project_memories: usize,
}

impl PriorContextSummary {
    pub fn is_empty(&self) -> bool {
        self.preferences == 0 && self.facts == 0 && self.episodes == 0 && self.project_memories == 0
    }
}

/// Capabilities a connected client can offer to the meeting. Drives
/// the audio-source picker (PWA filters by `audio_capture`), the
/// screenshot trigger (Phase 5: filters by `screen_capture`), etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Microphone or system audio. Mac app, future PWA-via-goggles.
    AudioCapture,
    /// Display capture. Mac app on macOS 14+.
    ScreenCapture,
    /// Renders meeting state (mode tabs, items, moments). Both PWA
    /// and Mac overlay (Phase 6) declare this.
    ControlSurface,
    /// Sub-capability of `audio_capture` indicating the device can
    /// grab system-wide audio output, not just a microphone.
    /// Distinguishes "Mac via SCKit" from "phone with mic permission."
    SystemAudio,
}

/// A registered device. The server tracks one entry per active WS
/// connection that has sent `RegisterDevice`. `online` flips to false
/// on disconnect; the entry is removed only when the user explicitly
/// unregisters (Phase 4 with the persistent `devices` table). For
/// Phase 2b, in-memory only — disconnect removes the entry entirely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub hostname: String,
    pub capabilities: Vec<Capability>,
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Intent {
    StartMeeting {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        metadata: Option<HashMap<String, String>>,
        /// Which registered device should provide audio for this
        /// meeting. The chosen device sees the resulting
        /// `AudioSourceDeviceChanged` event and starts streaming
        /// `/audio`. `None` means "no audio source bound" — the
        /// meeting runs silent until something is bound (or for
        /// PWA-only meetings without paired capture devices).
        #[serde(skip_serializing_if = "Option::is_none", default)]
        audio_source_device_id: Option<String>,
        /// Per-meeting assist surface sensitivity. Omitted /
        /// `None` defaults to `AssistSensitivity::Moderate`,
        /// preserving the historical behavior for clients that
        /// pre-date this field.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        assist_sensitivity: Option<AssistSensitivity>,
    },
    StopMeeting,
    /// Adjust the active meeting's assist sensitivity mid-meeting.
    /// The new value takes effect immediately for any subsequent
    /// `PushAssistSuggestion` tool call (threshold + prompt nudge
    /// re-read on each agent fire). Persisted to the meeting row
    /// so a reconnect / reload sees the latest value. No-op when
    /// no meeting is active.
    SetAssistSensitivity {
        value: AssistSensitivity,
    },
    /// Legacy no-op variants. Pause was removed in favor of client-
    /// local mute; old TestFlight builds may still send these. The
    /// dispatcher logs at debug level and returns an empty outcome so
    /// no `MeetingStateChanged` event fires and no error toast lands
    /// on the old client. Keep around until the v0.4.x mobile builds
    /// fall out of TestFlight (~30 days).
    Pause,
    Resume,
    SetMode {
        mode: String,
    },
    SetMetadata {
        key: String,
        value: Option<String>,
    },
    /// Capability-bearing client (Mac app, future glasses) declaring
    /// itself to the server. Server returns the assigned `device_id`
    /// via `Event::DeviceRegistered` (to the registering client) and
    /// broadcasts `Event::DevicesChanged` (to everyone).
    RegisterDevice {
        hostname: String,
        capabilities: Vec<Capability>,
        /// Client-supplied stable device id, persisted across
        /// reconnects (PWA stores it in bridge KV). When present the
        /// server reuses it instead of minting a fresh UUID, so a
        /// browser that drops and reconnects (e.g. wifi→5G) keeps the
        /// same identity and its audio-source binding survives. Absent
        /// for older clients (Mac/proto path) → server generates one.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        device_id: Option<String>,
    },
    /// Mark a "remember this" moment during an active meeting.
    /// `t` is the millisecond offset from meeting start. `t == 0`
    /// is a sentinel for "client doesn't know the offset" (glasses
    /// and mobile surfaces; also the proto3 default for an absent
    /// scalar) — the session layer substitutes the server-side
    /// meeting-clock elapsed time. Nonzero values are trusted
    /// verbatim. See `UserSession::handle_mark_moment`.
    MarkMoment {
        t: u64,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        note: Option<String>,
        /// Client-generated moment id (mobile: minted before sending
        /// so the client can immediately upload its own photo against
        /// it). Only a well-formed UUID is trusted; anything else is
        /// replaced server-side. Absent => the server mints one.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        id: Option<String>,
        /// The marking client will upload its own screenshot (mobile
        /// camera capture) rather than the Mac auto-screenshotting.
        /// When `true`, the server skips delegating
        /// `Event::CaptureMomentScreenshot` to the audio-source
        /// device. Absent/`false` => today's Mac-driven behavior.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        self_capture: Option<bool>,
    },
    ExpandItem {
        item_id: String,
    },
    /// Chat with the agent during an active meeting. The user's
    /// question is rendered as the user-side bubble in chat mode
    /// (Replace strategy, single Q+A pair); the agent's reply
    /// becomes the assistant-side bubble. Allowed only when a
    /// meeting is active — chat is per-meeting only,
    /// no persistence across meetings in v1.
    ///
    /// `attachment_ids` (added 2026-05-12) reference rows in
    /// `chat_attachments`, uploaded via
    /// `POST /meetings/:id/chat_attachments`. The server reads
    /// the bytes from disk and threads them as vision content
    /// blocks into the agent's LLM call. Empty list = today's
    /// text-only behavior. Mac is the only producer in v1.
    Chat {
        text: String,
        #[serde(default)]
        attachment_ids: Vec<String>,
    },
    /// Upsert a user-curated "quick ask". `id` is client-minted UUID
    /// (reused on edit). `position` orders the library; lower first.
    UpsertQuickAsk {
        id: String,
        label: String,
        text: String,
        position: i32,
    },
    /// Delete a user's quick ask by id. Unknown ids are a no-op.
    DeleteQuickAsk {
        id: String,
    },
    /// Deposit a fresh Auth0 access_token (kleos shared audience).
    /// The WS handshake already caches the user's token on connect;
    /// this intent lets the UI refresh the cached token mid-session
    /// without dropping and reopening the WS — typically fired by an
    /// Auth0 silent-refresh callback. See `mnemo/token_store.rs` for
    /// the storage + queue semantics.
    SetAuthToken {
        access_token: String,
    },
    /// Mint a single-use pairing code on behalf of the requesting
    /// user. The server replies with `Event::PairCodeMinted` on the
    /// same connection (not broadcast — the code is sensitive). The
    /// `/pair/code` REST endpoint stays alive as a fallback for old
    /// clients but the canonical path is this intent.
    MintPairCode {
        /// Optional human label baked into the resulting `paired_devices`
        /// row when a device redeems this code. Defaults to "G2 glasses"
        /// server-side.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        device_label: Option<String>,
    },
}

/// Internal envelope: `Event` + the local `users.id` of the user
/// the event belongs to. Server-side broadcast channels carry this;
/// per-connection forwarders filter by their own user_id so each
/// client only sees their user's events on the wire. Never seen by
/// clients (the `event` field is what gets serialized over WS).
#[derive(Debug, Clone)]
pub struct UserEvent {
    pub user_id: String,
    /// Meeting the event belongs to, when the PRODUCER knows it
    /// (today: the transcript summarizer). Lets the durable writer
    /// resolve the JSONL path without consulting the session registry
    /// at consume time — closing the race where a queued line
    /// straddling a stop/start boundary landed in the NEXT meeting's
    /// file. Server-internal; never serialized to clients.
    pub meeting_id: Option<String>,
    pub event: Event,
}

impl UserEvent {
    pub fn new(user_id: impl Into<String>, event: Event) -> Self {
        Self {
            user_id: user_id.into(),
            meeting_id: None,
            event,
        }
    }

    pub fn with_meeting(
        user_id: impl Into<String>,
        meeting_id: impl Into<String>,
        event: Event,
    ) -> Self {
        Self {
            user_id: user_id.into(),
            meeting_id: Some(meeting_id.into()),
            event,
        }
    }
}

/// True when `item` is a mid-stream chat partial (`meta.streaming ==
/// true`). `agent::chat::broadcast_chat_partial` emits these every
/// ~50 ms while a reply streams; they are pure UI sugar — the closing
/// `ItemsUpdate` from `surface_chat_reply` carries the canonical row.
/// They must never enter the durable persistence queue. The terminal
/// partial (`streaming: false`) is also emitted via the fanout-only
/// path at its call site; this predicate only needs to catch the
/// `true` flood.
fn item_is_streaming_partial(item: &Item) -> bool {
    item.meta
        .as_ref()
        .and_then(|m| m.get("streaming"))
        .and_then(|v| v.as_bool())
        == Some(true)
}

impl Event {
    /// True for events a durable consumer must never miss: the
    /// transcript JSONL writer, the items-table writer, and the
    /// mnemo pusher (which keys its session lifecycle off
    /// `MeetingStateChanged` / `MeetingFinalized` / `MetadataChanged`).
    /// `EventBus::emit` routes durable events onto the bounded mpsc
    /// queue (awaited — backpressure, never dropped); everything else
    /// is client fan-out only and may be dropped under load.
    pub fn is_durable(&self) -> bool {
        match self {
            Event::ItemsUpdate { .. }
            | Event::TranscriptTail { .. }
            | Event::MeetingStateChanged { .. }
            | Event::MeetingFinalized { .. }
            | Event::MetadataChanged { .. } => true,
            Event::ItemUpdated { item, .. } => !item_is_streaming_partial(item),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Snapshot {
        protocol_version: u32,
        meeting_state: MeetingState,
        /// Server-assigned id of the active meeting. `Some` when
        /// `meeting_state == Active`; `None` when idle.
        /// Clients use this to link to history (`GET /meetings/<id>`)
        /// and to reconcile across reconnects (same id = same meeting).
        #[serde(skip_serializing_if = "Option::is_none", default)]
        meeting_id: Option<String>,
        available_modes: Vec<ModeOption>,
        mode: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        display_tag: Option<String>,
        metadata: HashMap<String, String>,
        items: Vec<Item>,
        status: Status,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        prior_context: Option<PriorContextSummary>,
        /// All currently-registered devices, including the one this
        /// snapshot was sent to (if it has registered). Empty list is
        /// "no devices have registered yet."
        devices: Vec<Device>,
        /// The device whose audio is feeding the active meeting, if
        /// one is bound. None during idle or when no audio source is
        /// active (e.g., Phase 1 local meeting with no Mac client).
        #[serde(skip_serializing_if = "Option::is_none", default)]
        audio_source_device_id: Option<String>,
        /// Past meetings attached to the active meeting via the
        /// meeting-picker UI. Each entry is a past meeting's id; the
        /// agent can recall the past meeting's mnemo-stored summary
        /// on demand via the `recall_meeting` tool. Empty
        /// list during idle / a fresh meeting with nothing attached.
        #[serde(default)]
        attached_meeting_ids: Vec<String>,
        /// Active meeting's assist sensitivity. Carried in the
        /// snapshot so a client connecting mid-meeting (or
        /// reloading after a refresh) sees the current value
        /// without having to wait for a change event. Defaults
        /// to `Moderate` during idle so the picker's compose-
        /// screen default is well-defined.
        #[serde(default)]
        assist_sensitivity: AssistSensitivity,
    },
    /// Fired when the active meeting's assist sensitivity changes
    /// (via `SetAssistSensitivity`, or as a result of a fresh
    /// `StartMeeting` that included the field). Surfaces lets
    /// all connected clients stay in sync without polling.
    AssistSensitivityChanged {
        value: AssistSensitivity,
    },
    MeetingStateChanged {
        meeting_state: MeetingState,
        /// `Some` going to `Active`; `None` going to `Idle`.
        /// Lets clients track the current meeting id without
        /// waiting for the next snapshot.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        meeting_id: Option<String>,
    },
    /// Server-INTERNAL signal: the detached finalize task finished the
    /// offline post-processing for a stopped meeting. NOT forwarded to
    /// clients (filtered in the WS forward loop). Consumed by the mnemo
    /// pusher to retime its session reset so the drained transcript tail
    /// is pushed before the session closes.
    MeetingFinalized {
        meeting_id: String,
    },
    /// Server-INTERNAL: the post-stop STT drain tail for a finalized
    /// meeting, as full transcript `Item`s. Broadcast by
    /// `workers::finalize` BEFORE `MeetingFinalized`. Consumed by the
    /// persistence loop (JSONL append, addressed by `meeting_id` — no
    /// active-session lookup) and by the mnemo pusher (pushes to the
    /// still-open session when `meeting_id` matches). NOT forwarded to
    /// clients (filtered in the WS forward loop); no contract change.
    TranscriptTail {
        meeting_id: String,
        items: Vec<Item>,
    },
    ModeChanged {
        mode: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        display_tag: Option<String>,
        items: Vec<Item>,
    },
    DisplayTagChanged {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        tag: Option<String>,
    },
    MetadataChanged {
        metadata: HashMap<String, String>,
    },
    PriorContextChanged {
        summary: PriorContextSummary,
    },
    ItemsUpdate {
        mode: String,
        items: Vec<Item>,
    },
    /// One item changed in-place (today's only producer is the
    /// `expand_item` flow — agent writes the LLM expansion into
    /// the item's `detail` and broadcasts the updated item).
    /// Clients update by id, replacing the matching item in their
    /// items_by_mode map. Works uniformly across Replace and
    /// Append strategies — semantics is "replace this one row,"
    /// not "append" or "overwrite the whole list."
    ItemUpdated {
        mode: String,
        item: Item,
    },
    TranscriptInterim {
        text: String,
    },
    Status {
        status: Status,
    },
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        intent_ref: Option<String>,
    },
    /// Sent only to the connection that just successfully registered.
    /// Carries the server-assigned `device_id` so the client can
    /// identify "itself" in subsequent device lists / commands.
    DeviceRegistered {
        device: Device,
    },
    /// Broadcast whenever the device list changes (registration,
    /// unregistration on disconnect, capability update). Carries the
    /// full current list — clients replace their cache.
    DevicesChanged {
        devices: Vec<Device>,
    },
    /// Broadcast when the audio-source binding for the active meeting
    /// changes (start/stop/rebind). `None` means no device is bound
    /// (idle or local-only meeting).
    AudioSourceDeviceChanged {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        device_id: Option<String>,
    },
    /// Broadcast whenever the attached-artifact set for the user's
    /// active meeting changes (attach or detach). Carries the full
    /// current set of artifact IDs so clients overwrite their local
    /// mirror — no diff/incremental logic needed. Mac and PWA both
    /// pre-check rows in their attach picker against this set.
    ArtifactsChanged {
        artifact_ids: Vec<String>,
    },
    /// Broadcast whenever the attached-past-meetings set for the
    /// user's active meeting changes (attach or detach). Same
    /// "full current set" shape as ArtifactsChanged so clients
    /// overwrite their local mirror without diff bookkeeping.
    AttachedMeetingsChanged {
        meeting_ids: Vec<String>,
    },
    /// Sent after a WS-initiated `mark_moment` lands, asking the
    /// recipient to capture a screenshot and upload it via
    /// `POST /meetings/:id/moments/:moment_id/screenshot`. The
    /// server delivers this point-to-point — only the
    /// `screen_capture`-capable device bound as the audio source
    /// receives it. No client-side filtering needed.
    CaptureMomentScreenshot {
        meeting_id: String,
        moment_id: String,
        t_ms: i64,
    },
    /// Emitted by the moment-summary worker when it writes the
    /// final summary text for a moment. Today's only consumer is
    /// the mnemo pusher (which pushes the summary text — not the
    /// screenshot — as an assistant-role turn so it lands in
    /// long-term memory). PWA and Mac currently ignore this
    /// event; future "moment ready" toast or auto-refresh of a
    /// moments list would slot in here.
    MomentSummarized {
        moment_id: String,
        meeting_id: String,
        t_ms: i64,
        summary: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        note: Option<String>,
    },
    /// Direct response to `Intent::MintPairCode`. Sent only on the
    /// connection that requested the code — never broadcast, since
    /// the code is short-lived but still sensitive.
    PairCodeMinted {
        /// Display form ("XXXX-XXXX"). UI shows this as-is; the PWA's
        /// redeem call normalizes either form server-side.
        code: String,
        /// ISO-8601 expiry. Drives the countdown on the requesting
        /// surface.
        expires_at: String,
    },
    /// Fan-out signal that the user's paired-devices set changed
    /// (a device was redeemed, or revoked). Carries no payload —
    /// subscribers re-fetch via `GET /pair/devices`. Single source
    /// of truth + lets us extend the schema without bumping the
    /// event shape.
    PairedDevicesChanged,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let json = serde_json::to_string(value).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    #[test]
    fn intent_start_meeting_full() {
        let i = Intent::StartMeeting {
            description: Some("Q1 review".into()),
            metadata: Some(HashMap::from([("project".into(), "helix".into())])),
            audio_source_device_id: Some("dev-mac-1".into()),
            assist_sensitivity: None,
        };
        assert_eq!(round_trip(&i), i);
    }

    #[test]
    fn intent_start_meeting_minimal() {
        let i = Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        };
        let json = serde_json::to_string(&i).unwrap();
        assert!(!json.contains("description"));
        assert!(!json.contains("metadata"));
        assert_eq!(round_trip(&i), i);
    }

    #[test]
    fn intent_stop_and_legacy_pause_resume() {
        // Pause/Resume are kept as no-op legacy variants for old
        // TestFlight builds; verify they still round-trip on the
        // wire (the server's dispatcher logs and ignores them).
        for i in [Intent::StopMeeting, Intent::Pause, Intent::Resume] {
            assert_eq!(round_trip(&i), i);
        }
    }

    #[test]
    fn intent_set_mode() {
        let i = Intent::SetMode {
            mode: "highlights".into(),
        };
        assert_eq!(round_trip(&i), i);
    }

    #[test]
    fn intent_set_metadata_set_and_delete() {
        let set = Intent::SetMetadata {
            key: "project".into(),
            value: Some("helix".into()),
        };
        let del = Intent::SetMetadata {
            key: "project".into(),
            value: None,
        };
        assert_eq!(round_trip(&set), set);
        assert_eq!(round_trip(&del), del);
        // value: null must round-trip as Some(None) → None — the field is present.
        let json = serde_json::to_string(&del).unwrap();
        assert!(json.contains("\"value\":null"));
    }

    #[test]
    fn intent_mark_moment() {
        let i = Intent::MarkMoment {
            t: 1234,
            note: Some("nice".into()),
            id: None,
            self_capture: None,
        };
        assert_eq!(round_trip(&i), i);
    }

    #[test]
    fn intent_expand_item() {
        let i = Intent::ExpandItem {
            item_id: "abc".into(),
        };
        assert_eq!(round_trip(&i), i);
    }

    #[test]
    fn event_snapshot_round_trip() {
        let e = Event::Snapshot {
            protocol_version: PROTOCOL_VERSION,
            meeting_state: MeetingState::Idle,
            meeting_id: None,
            available_modes: vec![ModeOption {
                id: "highlights".into(),
                label: "Highlights".into(),
                update_strategy: UpdateStrategy::Replace,
            }],
            mode: "highlights".into(),
            display_tag: None,
            metadata: HashMap::new(),
            items: vec![],
            status: Status {
                listening: false,
                error: None,
            },
            prior_context: None,
            devices: vec![],
            audio_source_device_id: None,
            attached_meeting_ids: vec![],
            assist_sensitivity: AssistSensitivity::Moderate,
        };
        assert_eq!(round_trip(&e), e);
    }

    #[test]
    fn event_meeting_state_changed() {
        let e = Event::MeetingStateChanged {
            meeting_state: MeetingState::Active,
            meeting_id: Some("abc-123".into()),
        };
        assert_eq!(round_trip(&e), e);
    }

    #[test]
    fn event_meeting_state_changed_idle_omits_id() {
        // Going to Idle: meeting_id is None and `skip_serializing_if`
        // keeps it out of the wire JSON entirely.
        let e = Event::MeetingStateChanged {
            meeting_state: MeetingState::Idle,
            meeting_id: None,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            !json.contains("meeting_id"),
            "expected meeting_id omitted: {json}"
        );
        assert_eq!(round_trip(&e), e);
    }

    #[test]
    fn meeting_finalized_serializes_snake_case() {
        let e = Event::MeetingFinalized {
            meeting_id: "m-123".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(json, r#"{"type":"meeting_finalized","meeting_id":"m-123"}"#);
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn transcript_tail_round_trips_with_items() {
        // Server-internal variant: still must round-trip through serde
        // because it travels the broadcast bus as part of `UserEvent`.
        let e = Event::TranscriptTail {
            meeting_id: "m-123".into(),
            items: vec![Item {
                id: "c9".into(),
                text: "closing words".into(),
                detail: None,
                t: 5400,
                meta: Some(serde_json::json!({"speaker": "1"})),
            }],
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            json.contains(r#""type":"transcript_tail""#),
            "snake_case tag expected: {json}"
        );
        assert_eq!(round_trip(&e), e);
    }

    #[test]
    fn event_mode_changed_with_items() {
        let e = Event::ModeChanged {
            mode: "transcript".into(),
            display_tag: None,
            items: vec![Item {
                id: "i1".into(),
                text: "hello".into(),
                detail: None,
                t: 100,
                meta: None,
            }],
        };
        assert_eq!(round_trip(&e), e);
    }

    #[test]
    fn event_metadata_changed() {
        let e = Event::MetadataChanged {
            metadata: HashMap::from([("foo".into(), "bar".into())]),
        };
        assert_eq!(round_trip(&e), e);
    }

    #[test]
    fn event_items_update_carries_mode() {
        let e = Event::ItemsUpdate {
            mode: "highlights".into(),
            items: vec![Item {
                id: "h-0".into(),
                text: "key point".into(),
                detail: None,
                t: 0,
                meta: None,
            }],
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains(r#""type":"items_update""#));
        assert!(json.contains(r#""mode":"highlights""#));
        let round: Event = serde_json::from_str(&json).unwrap();
        match round {
            Event::ItemsUpdate { mode, items } => {
                assert_eq!(mode, "highlights");
                assert_eq!(items.len(), 1);
            }
            _ => panic!("expected ItemsUpdate"),
        }
    }

    #[test]
    fn event_transcript_interim_round_trip() {
        let e = Event::TranscriptInterim {
            text: "hello world".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains(r#""type":"transcript_interim""#));
        assert!(json.contains(r#""text":"hello world""#));
        let round: Event = serde_json::from_str(&json).unwrap();
        match round {
            Event::TranscriptInterim { text } => {
                assert_eq!(text, "hello world");
            }
            _ => panic!("expected TranscriptInterim"),
        }
    }

    #[test]
    fn event_status() {
        let e = Event::Status {
            status: Status {
                listening: true,
                error: None,
            },
        };
        assert_eq!(round_trip(&e), e);
    }

    #[test]
    fn event_error_with_intent_ref() {
        let e = Event::Error {
            code: "unknown_mode".into(),
            message: "no such mode".into(),
            intent_ref: Some("bogus".into()),
        };
        assert_eq!(round_trip(&e), e);
    }

    #[test]
    fn intent_type_discriminator_snake_case() {
        let i = Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
            assist_sensitivity: None,
        };
        let json = serde_json::to_string(&i).unwrap();
        assert!(json.contains("\"type\":\"start_meeting\""));
    }

    #[test]
    fn event_type_discriminator_snake_case() {
        let e = Event::MeetingStateChanged {
            meeting_state: MeetingState::Idle,
            meeting_id: None,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"type\":\"meeting_state_changed\""));
        assert!(json.contains("\"meeting_state\":\"idle\""));
    }

    #[test]
    fn unknown_intent_type_fails_decode() {
        let json = r#"{"type":"fly_to_moon"}"#;
        let r: Result<Intent, _> = serde_json::from_str(json);
        assert!(r.is_err());
    }

    // ─── Durability classification (EventBus routing) ───────────────

    fn transcript_items_update() -> Event {
        Event::ItemsUpdate {
            mode: "transcript".into(),
            items: vec![Item {
                id: "t-1".into(),
                text: "committed sentence.".into(),
                detail: None,
                t: 1000,
                meta: None,
            }],
        }
    }

    #[test]
    fn durability_items_update_is_durable() {
        assert!(transcript_items_update().is_durable());
        let highlights = Event::ItemsUpdate {
            mode: "highlights".into(),
            items: vec![],
        };
        assert!(highlights.is_durable());
    }

    #[test]
    fn durability_streaming_chat_partial_is_fanout_only() {
        let partial = Event::ItemUpdated {
            mode: "chat".into(),
            item: Item {
                id: "chat-a-1".into(),
                text: "partial tex".into(),
                detail: None,
                t: 0,
                meta: Some(serde_json::json!({"role": "assistant", "streaming": true})),
            },
        };
        assert!(!partial.is_durable());
        let expand = Event::ItemUpdated {
            mode: "highlights".into(),
            item: Item {
                id: "h-1".into(),
                text: "key point".into(),
                detail: Some("expansion".into()),
                t: 0,
                meta: None,
            },
        };
        assert!(expand.is_durable());
    }

    #[test]
    fn durability_interim_status_snapshot_are_fanout_only() {
        assert!(!Event::TranscriptInterim {
            text: "in flight".into()
        }
        .is_durable());
        assert!(!Event::Status {
            status: Status {
                listening: true,
                error: None,
            },
        }
        .is_durable());
        let snapshot = Event::Snapshot {
            protocol_version: PROTOCOL_VERSION,
            meeting_state: MeetingState::Idle,
            meeting_id: None,
            available_modes: vec![],
            mode: "transcript".into(),
            display_tag: None,
            metadata: HashMap::new(),
            items: vec![],
            status: Status {
                listening: false,
                error: None,
            },
            prior_context: None,
            devices: vec![],
            audio_source_device_id: None,
            attached_meeting_ids: vec![],
            assist_sensitivity: AssistSensitivity::Moderate,
        };
        assert!(!snapshot.is_durable());
    }

    #[test]
    fn user_event_new_carries_no_meeting_id_and_with_meeting_stamps_it() {
        let plain = UserEvent::new("u1", Event::PairedDevicesChanged);
        assert_eq!(plain.meeting_id, None);
        let stamped = UserEvent::with_meeting("u1", "m-42", Event::PairedDevicesChanged);
        assert_eq!(stamped.meeting_id.as_deref(), Some("m-42"));
        assert_eq!(stamped.user_id, "u1");
    }

    #[test]
    fn durability_meeting_lifecycle_and_metadata_are_durable() {
        assert!(Event::MeetingStateChanged {
            meeting_state: MeetingState::Active,
            meeting_id: Some("m-1".into()),
        }
        .is_durable());
        assert!(Event::MeetingStateChanged {
            meeting_state: MeetingState::Idle,
            meeting_id: None,
        }
        .is_durable());
        assert!(Event::MeetingFinalized {
            meeting_id: "m-1".into(),
        }
        .is_durable());
        assert!(Event::MetadataChanged {
            metadata: HashMap::new(),
        }
        .is_durable());
    }
}
