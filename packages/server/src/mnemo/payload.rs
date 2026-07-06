//! Pure payload builders for mnemo's `POST /events` API.
//!
//! Mirrors the wire schema documented in mnemo at
//! `server/internal/api/events.go` — a flat object with
//! `session_id`, `source`, `workstation`, `workdir`, optional
//! `project`, `turns`, and a free-form `attributes` map. Mnemo
//! denormalises `meeting_id` / `meeting_ended` out of `attributes`
//! server-side (see mnemo's `store/events.go`); we just inject them
//! as string entries.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub const SOURCE: &str = "auris";
pub const WORKDIR: &str = "/auris";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnRole {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub role: TurnRole,
    pub content: String,
}

/// Wire-format payload for mnemo's `/events` endpoint. Field names
/// and shape match mnemo's `eventRequest` struct verbatim — drift
/// here silently breaks ingestion (mnemo decodes JSON keys it knows
/// and ignores the rest, so a wrong key reads as the zero value).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IngestEvent {
    pub session_id: String,
    pub source: String,
    pub workstation: String,
    pub workdir: String,
    /// Mapped from the meeting's `project` metadata tag if present.
    /// Drives mnemo's project-scoped extraction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub turns: Vec<Turn>,
    /// Generic key/value bag — passes through mnemo unchanged into
    /// the `attributes` JSONB column. Mnemo denormalises a couple of
    /// keys server-side (`meeting_id`, `meeting_ended`); everything
    /// else is preserved for future extraction strategies.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub attributes: HashMap<String, String>,
}

/// `IngestRequest` is the full HTTP request body. mnemo's `/events`
/// endpoint accepts a single `IngestEvent` per request, so this is just
/// a typed wrapper.
pub type IngestRequest = IngestEvent;

/// Build the attributes bag that rides with every push for a
/// meeting. Promotes `project` to a top-level field on the event
/// (mnemo treats it specially); injects `meeting_id`, `mode`, and
/// optional `meeting_ended` into the attributes map so mnemo's
/// server-side denormalisation can pull them out.
fn build_event(
    session_id: &str,
    workstation: &str,
    metadata: &HashMap<String, String>,
    meeting_id: Option<&str>,
    mode: Option<&str>,
    meeting_ended: Option<chrono::DateTime<chrono::Utc>>,
    turns: Vec<Turn>,
) -> IngestEvent {
    let project = metadata.get("project").cloned();
    let mut attributes = metadata.clone();
    if let Some(id) = meeting_id {
        attributes.insert("meeting_id".to_string(), id.to_string());
    }
    if let Some(m) = mode {
        attributes.insert("mode".to_string(), m.to_string());
    }
    if let Some(end) = meeting_ended {
        attributes.insert("meeting_ended".to_string(), end.to_rfc3339());
    }
    IngestEvent {
        session_id: session_id.to_string(),
        source: SOURCE.to_string(),
        workstation: workstation.to_string(),
        workdir: WORKDIR.to_string(),
        project,
        turns,
        attributes,
    }
}

/// One transcript sentence → one user-role turn.
pub fn build_sentence_event(
    session_id: &str,
    workstation: &str,
    metadata: &HashMap<String, String>,
    meeting_id: Option<&str>,
    sentence: &str,
) -> IngestEvent {
    build_event(
        session_id,
        workstation,
        metadata,
        meeting_id,
        Some("transcript"),
        None,
        vec![Turn {
            role: TurnRole::User,
            content: sentence.to_string(),
        }],
    )
}

/// End-of-meeting signal event. Carries `meeting_ended` (RFC3339 timestamp)
/// so mnemo flips meeting_ended=true and enqueues finalize_meeting. mnemo
/// requires a non-empty turns array; the summary reads every meeting event,
/// so this single marker turn just tails the transcript harmlessly.
pub fn build_meeting_ended_event(
    session_id: &str,
    workstation: &str,
    metadata: &HashMap<String, String>,
    meeting_id: &str,
    ended_at: chrono::DateTime<chrono::Utc>,
) -> IngestEvent {
    build_event(
        session_id,
        workstation,
        metadata,
        Some(meeting_id),
        None,           // mode
        Some(ended_at), // meeting_ended → RFC3339 string in attributes
        vec![Turn {
            role: TurnRole::User,
            content: "(meeting ended)".to_string(),
        }],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn sentence_event_has_one_user_turn() {
        let ev = build_sentence_event(
            "sess-1",
            "host",
            &meta(&[("project", "helix")]),
            None,
            "We talked about the demo.",
        );
        assert_eq!(ev.session_id, "sess-1");
        assert_eq!(ev.turns.len(), 1);
        assert_eq!(ev.turns[0].role, TurnRole::User);
        assert_eq!(ev.turns[0].content, "We talked about the demo.");
        assert_eq!(ev.project.as_deref(), Some("helix"));
        assert_eq!(ev.source, SOURCE);
    }

    #[test]
    fn project_field_promotes_from_metadata() {
        let no_project = build_sentence_event("s", "h", &meta(&[("title", "x")]), None, "hi");
        assert!(no_project.project.is_none());
        // Other metadata still rides along in attributes.
        assert_eq!(no_project.attributes.get("title"), Some(&"x".to_string()));
    }

    #[test]
    fn attributes_round_trip_full_metadata() {
        let m = meta(&[
            ("project", "helix"),
            ("owner", "tiago"),
            ("title", "SMB demo"),
        ]);
        let ev = build_sentence_event("s", "h", &m, None, "x");
        // 3 user-set attributes + 1 injected `mode = "transcript"`.
        assert_eq!(ev.attributes.len(), 4);
        assert_eq!(ev.attributes.get("owner"), Some(&"tiago".to_string()));
        assert_eq!(ev.attributes.get("mode"), Some(&"transcript".to_string()));
    }

    #[test]
    fn sentence_event_omits_meeting_ended_attribute() {
        // Mid-meeting events don't know the end time yet; meeting_ended
        // must NOT appear on those, otherwise mnemo's denormalisation
        // would mark the row as a finished-meeting summary.
        let ev = build_sentence_event("s", "h", &meta(&[]), Some("m-1"), "hi");
        assert!(
            !ev.attributes.contains_key("meeting_ended"),
            "meeting_ended must not leak onto mid-meeting events"
        );
    }

    #[test]
    fn meeting_ended_event_carries_meeting_ended_and_marker_turn() {
        let ended = chrono::DateTime::parse_from_rfc3339("2026-05-30T01:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let ev = build_meeting_ended_event(
            "sess-9",
            "host",
            &meta(&[("project", "helix")]),
            "m-7",
            ended,
        );
        // Single marker turn that harmlessly tails the transcript.
        assert_eq!(ev.turns.len(), 1);
        assert_eq!(ev.turns[0].role, TurnRole::User);
        assert_eq!(ev.turns[0].content, "(meeting ended)");
        // meeting_ended denormalisation key present as RFC3339; meeting_id too.
        assert_eq!(
            ev.attributes.get("meeting_ended"),
            Some(&"2026-05-30T01:00:00+00:00".to_string())
        );
        assert_eq!(ev.attributes.get("meeting_id"), Some(&"m-7".to_string()));
        // No transcript mode on the end marker.
        assert!(!ev.attributes.contains_key("mode"));
    }

    #[test]
    fn serializes_to_mnemo_wire_shape() {
        // This test pins the on-wire field names. Mnemo's
        // `eventRequest` is the source of truth; any rename here
        // must be matched on mnemo's side or every push will fail
        // with `400 session_id required` / silent attribute drops.
        let ev = build_sentence_event(
            "sess-1",
            "host-x",
            &meta(&[("project", "helix"), ("owner", "tiago")]),
            Some("m-42"),
            "hello world",
        );
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["session_id"], "sess-1");
        assert_eq!(json["source"], "auris");
        assert_eq!(json["workstation"], "host-x");
        assert_eq!(json["workdir"], "/auris");
        assert_eq!(json["project"], "helix");
        assert_eq!(json["turns"][0]["role"], "user");
        assert_eq!(json["turns"][0]["content"], "hello world");
        assert_eq!(json["attributes"]["owner"], "tiago");
        assert_eq!(json["attributes"]["meeting_id"], "m-42");
        assert_eq!(json["attributes"]["mode"], "transcript");
        // Top-level keys mnemo expects, and ONLY those. No `context`
        // wrapper; no `timestamp` / `date` (mnemo uses its own NOW()
        // for created_at and ignores any wire-side timestamp).
        let top_keys: std::collections::BTreeSet<_> = json
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        let expected: std::collections::BTreeSet<&str> = [
            "session_id",
            "source",
            "workstation",
            "workdir",
            "project",
            "turns",
            "attributes",
        ]
        .into_iter()
        .collect();
        assert_eq!(top_keys, expected);
    }
}
