//! WebSocket message contract. Mirrors `packages/contract/src/index.ts`.
//! See `docs/specs/server.md` §2.6.

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Intent {
    StartMeeting {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        metadata: Option<HashMap<String, String>>,
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
    MarkMoment {
        t: u64,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        note: Option<String>,
    },
    ExpandItem {
        item_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Snapshot {
        protocol_version: u32,
        meeting_state: MeetingState,
        available_modes: Vec<ModeOption>,
        mode: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        display_tag: Option<String>,
        metadata: HashMap<String, String>,
        items: Vec<Item>,
        status: Status,
    },
    MeetingStateChanged {
        meeting_state: MeetingState,
    },
    AvailableModesChanged {
        available_modes: Vec<ModeOption>,
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
    ItemsUpdate {
        items: Vec<Item>,
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
        };
        assert_eq!(round_trip(&i), i);
    }

    #[test]
    fn intent_start_meeting_minimal() {
        let i = Intent::StartMeeting {
            description: None,
            metadata: None,
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
        };
        assert_eq!(round_trip(&e), e);
    }

    #[test]
    fn event_meeting_state_changed() {
        let e = Event::MeetingStateChanged {
            meeting_state: MeetingState::Active,
        };
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
    fn event_items_update() {
        let e = Event::ItemsUpdate { items: vec![] };
        assert_eq!(round_trip(&e), e);
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
        };
        let json = serde_json::to_string(&i).unwrap();
        assert!(json.contains("\"type\":\"start_meeting\""));
    }

    #[test]
    fn event_type_discriminator_snake_case() {
        let e = Event::MeetingStateChanged {
            meeting_state: MeetingState::Idle,
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
