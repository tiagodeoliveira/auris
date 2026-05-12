//! Pure payload builders for mnemo's `POST /events` API.
//!
//! Mirrors the schema documented in
//! `/Users/tiago/src/github.com/tiagodeoliveira/mnemo` —
//! sessionId + turns + context (workstation, workdir, timestamp, source,
//! optional project, optional date, optional generic attributes).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub const SOURCE: &str = "meeting_companion";
pub const WORKDIR: &str = "/meeting_companion";

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IngestContext {
    pub workstation: String,
    pub workdir: String,
    /// ISO 8601 timestamp.
    pub timestamp: String,
    pub source: String,
    /// Mapped from the meeting's `project` metadata tag if present.
    /// Drives mnemo's existing project-scoped extraction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// `YYYY-MM-DD` of the meeting's start date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// Generic key/value bag — passes through mnemo unchanged. Future
    /// extraction strategies can read these without an API change.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub attributes: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IngestEvent {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub turns: Vec<Turn>,
    pub context: IngestContext,
}

/// `IngestRequest` is the full HTTP request body. mnemo's `/events`
/// endpoint accepts a single `IngestEvent` per request, so this is just
/// a typed wrapper.
pub type IngestRequest = IngestEvent;

/// Build the context block that's sent with every push for a meeting.
/// Reused for sentence / chat / moment / summary-bundle events.
///
/// `meeting_id`, `mode`, and `meeting_ended` (when set) ride along in
/// `attributes` so mnemo's recall layer can filter by them.
/// - `meeting_id` keeps the door open for per-meeting recall ("what
///   did I record in meeting X?") and the per-user mnemo identity
///   work flagged in PLAN.md §4.2.
/// - `mode` distinguishes the source pipeline of each turn
///   (transcript / chat / moment / summary_bundle).
/// - `meeting_ended` is only set on the summary-bundle event fired at
///   stop_meeting — mid-meeting pushes can't carry it because the
///   end time isn't known yet. Recall can use this to find "meetings
///   that ended after X" by filtering summary_bundle events.
pub fn build_context(
    workstation: &str,
    metadata: &HashMap<String, String>,
    started_at: chrono::DateTime<chrono::Utc>,
    meeting_id: Option<&str>,
    mode: Option<&str>,
    meeting_ended: Option<chrono::DateTime<chrono::Utc>>,
) -> IngestContext {
    let project = metadata.get("project").cloned();
    let date = Some(started_at.format("%Y-%m-%d").to_string());
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
    IngestContext {
        workstation: workstation.to_string(),
        workdir: WORKDIR.to_string(),
        timestamp: started_at.to_rfc3339(),
        source: SOURCE.to_string(),
        project,
        date,
        attributes,
    }
}

/// One transcript sentence → one user-role turn.
pub fn build_sentence_event(
    session_id: &str,
    workstation: &str,
    metadata: &HashMap<String, String>,
    started_at: chrono::DateTime<chrono::Utc>,
    meeting_id: Option<&str>,
    sentence: &str,
) -> IngestEvent {
    IngestEvent {
        session_id: session_id.to_string(),
        turns: vec![Turn {
            role: TurnRole::User,
            content: sentence.to_string(),
        }],
        context: build_context(
            workstation,
            metadata,
            started_at,
            meeting_id,
            Some("transcript"),
            None,
        ),
    }
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

    fn ts() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-05-04T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn sentence_event_has_one_user_turn() {
        let ev = build_sentence_event(
            "sess-1",
            "host",
            &meta(&[("project", "helix")]),
            ts(),
            None,
            "We talked about the demo.",
        );
        assert_eq!(ev.session_id, "sess-1");
        assert_eq!(ev.turns.len(), 1);
        assert_eq!(ev.turns[0].role, TurnRole::User);
        assert_eq!(ev.turns[0].content, "We talked about the demo.");
        assert_eq!(ev.context.project.as_deref(), Some("helix"));
        assert_eq!(ev.context.date.as_deref(), Some("2026-05-04"));
        assert_eq!(ev.context.source, SOURCE);
    }

    #[test]
    fn project_field_promotes_from_metadata() {
        let no_project = build_sentence_event("s", "h", &meta(&[("title", "x")]), ts(), None, "hi");
        assert!(no_project.context.project.is_none());
        // Other metadata still rides along in attributes.
        assert_eq!(
            no_project.context.attributes.get("title"),
            Some(&"x".to_string())
        );
    }

    #[test]
    fn attributes_round_trip_full_metadata() {
        let m = meta(&[
            ("project", "helix"),
            ("owner", "tiago"),
            ("title", "SMB demo"),
        ]);
        let ev = build_sentence_event("s", "h", &m, ts(), None, "x");
        // 3 user-set attributes + 1 injected `mode = "transcript"`.
        assert_eq!(ev.context.attributes.len(), 4);
        assert_eq!(
            ev.context.attributes.get("owner"),
            Some(&"tiago".to_string())
        );
    }

    #[test]
    fn sentence_event_omits_meeting_ended_attribute() {
        // Mid-meeting events don't know the end time yet; meeting_ended
        // must NOT appear on those, otherwise recall would return
        // stale rows when filtering by end-time ranges.
        let ev = build_sentence_event("s", "h", &meta(&[]), ts(), Some("m-1"), "hi");
        assert!(
            !ev.context.attributes.contains_key("meeting_ended"),
            "meeting_ended must not leak onto mid-meeting events"
        );
    }

    #[test]
    fn serializes_to_expected_shape() {
        let ev = build_sentence_event(
            "sess-1",
            "host-x",
            &meta(&[("project", "helix"), ("owner", "tiago")]),
            ts(),
            Some("m-42"),
            "hello world",
        );
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["sessionId"], "sess-1");
        assert_eq!(json["turns"][0]["role"], "user");
        assert_eq!(json["turns"][0]["content"], "hello world");
        assert_eq!(json["context"]["source"], "meeting_companion");
        assert_eq!(json["context"]["project"], "helix");
        assert_eq!(json["context"]["date"], "2026-05-04");
        assert_eq!(json["context"]["attributes"]["owner"], "tiago");
    }
}
