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
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStrategy {
    Replace,
    Append,
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
    pub paused: bool,
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
    },
    StopMeeting,
    Pause,
    Resume,
    SetMode {
        mode: String,
    },
    SetMetadata {
        key: String,
        value: Option<String>,
    },
    /// Extract metadata from a description without starting the meeting.
    /// Runs the same extraction pipeline as `start_meeting` but in idle
    /// state, so the user can review/edit chips before starting.
    ExtractMetadata {
        description: String,
    },
    /// Capability-bearing client (Mac app, future glasses) declaring
    /// itself to the server. Server returns the assigned `device_id`
    /// via `Event::DeviceRegistered` (to the registering client) and
    /// broadcasts `Event::DevicesChanged` (to everyone).
    RegisterDevice {
        hostname: String,
        capabilities: Vec<Capability>,
    },
    MarkMoment {
        t: u64,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        note: Option<String>,
    },
    ExpandItem {
        item_id: String,
    },
    /// Chat with the agent during an active meeting. The user's
    /// question is rendered as the user-side bubble in chat mode
    /// (Replace strategy, single Q+A pair); the agent's reply
    /// becomes the assistant-side bubble. Allowed only when a
    /// meeting is active or paused — chat is per-meeting only,
    /// no persistence across meetings in v1.
    Chat {
        text: String,
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
    pub event: Event,
}

impl UserEvent {
    pub fn new(user_id: impl Into<String>, event: Event) -> Self {
        Self {
            user_id: user_id.into(),
            event,
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
        /// `meeting_state == Active` or `Paused`; `None` when idle.
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
        /// on demand via the `fetch_meeting_summary` tool. Empty
        /// list during idle / a fresh meeting with nothing attached.
        #[serde(default)]
        attached_meeting_ids: Vec<String>,
    },
    MeetingStateChanged {
        meeting_state: MeetingState,
        /// `Some` going to `Active`/`Paused`; `None` going to `Idle`.
        /// Lets clients track the current meeting id without
        /// waiting for the next snapshot.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        meeting_id: Option<String>,
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
        };
        assert_eq!(round_trip(&i), i);
    }

    #[test]
    fn intent_start_meeting_minimal() {
        let i = Intent::StartMeeting {
            description: None,
            metadata: None,
            audio_source_device_id: None,
        };
        let json = serde_json::to_string(&i).unwrap();
        assert!(!json.contains("description"));
        assert!(!json.contains("metadata"));
        assert_eq!(round_trip(&i), i);
    }

    #[test]
    fn intent_stop_pause_resume() {
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
                paused: false,
                error: None,
            },
            prior_context: None,
            devices: vec![],
            audio_source_device_id: None,
            attached_meeting_ids: vec![],
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
                paused: false,
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
}
