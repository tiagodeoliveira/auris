//! Pure payload builders for mnemo's `POST /events` API.
//!
//! Mirrors the schema documented in
//! `/Users/tiago/src/github.com/tiagodeoliveira/mnemo` —
//! sessionId + turns + context (workstation, workdir, timestamp, source,
//! optional project, optional date, optional generic attributes).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::contract::Item;

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

/// One chat exchange → one event with two turns: user-role
/// question + assistant-role reply. Streams the same way as
/// transcript sentences (per fire), so mnemo recall captures
/// the question and the agent's answer as paired memories.
pub fn build_chat_event(
    session_id: &str,
    workstation: &str,
    metadata: &HashMap<String, String>,
    started_at: chrono::DateTime<chrono::Utc>,
    meeting_id: Option<&str>,
    question: &str,
    answer: &str,
) -> IngestEvent {
    IngestEvent {
        session_id: session_id.to_string(),
        turns: vec![
            Turn {
                role: TurnRole::User,
                content: question.to_string(),
            },
            Turn {
                role: TurnRole::Assistant,
                content: answer.to_string(),
            },
        ],
        context: build_context(
            workstation,
            metadata,
            started_at,
            meeting_id,
            Some("chat"),
            None,
        ),
    }
}

/// Moment summary → one assistant-role turn carrying the
/// transcript-window + screenshot-synthesis text. The screenshot
/// itself isn't sent (mnemo only takes text); only the summary
/// the LLM produced. `note` (when the user attached one at mark
/// time) prefixes the turn so recall surfaces "the user's intent"
/// alongside the auto-generated summary.
pub fn build_moment_event(
    session_id: &str,
    workstation: &str,
    metadata: &HashMap<String, String>,
    started_at: chrono::DateTime<chrono::Utc>,
    meeting_id: Option<&str>,
    t_ms: i64,
    summary: &str,
    note: Option<&str>,
) -> IngestEvent {
    let total_secs = (t_ms.max(0) / 1000) as u64;
    let timestamp = format!("{:02}:{:02}", total_secs / 60, total_secs % 60);
    let mut content = format!("Moment at {timestamp}: {summary}");
    if let Some(n) = note.filter(|s| !s.trim().is_empty()) {
        content = format!("Moment at {timestamp} — user note: {n}\n{summary}");
    }
    IngestEvent {
        session_id: session_id.to_string(),
        turns: vec![Turn {
            role: TurnRole::Assistant,
            content,
        }],
        context: build_context(
            workstation,
            metadata,
            started_at,
            meeting_id,
            Some("moment"),
            None,
        ),
    }
}

/// At meeting stop, bundle the LLM-extracted per-mode summaries into one
/// event of assistant-role turns. Skips empty modes. Skips entirely if
/// nothing was extracted (returns `None`).
pub fn build_summary_event(
    session_id: &str,
    workstation: &str,
    metadata: &HashMap<String, String>,
    started_at: chrono::DateTime<chrono::Utc>,
    meeting_id: Option<&str>,
    meeting_ended: Option<chrono::DateTime<chrono::Utc>>,
    items_by_mode: &HashMap<String, Vec<Item>>,
) -> Option<IngestEvent> {
    let mut turns = Vec::new();
    // Stable, human-readable order: actions first, then highlights, then
    // open questions. Transcript mode is intentionally omitted — those
    // items were already streamed sentence-by-sentence.
    for (mode_id, header) in [
        ("actions", "Action items:"),
        ("highlights", "Highlights:"),
        ("open_questions", "Open questions:"),
    ] {
        let items = match items_by_mode.get(mode_id) {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };
        let mut block = String::from(header);
        for item in items {
            block.push_str("\n- ");
            block.push_str(&item.text);
        }
        turns.push(Turn {
            role: TurnRole::Assistant,
            content: block,
        });
    }
    if turns.is_empty() {
        return None;
    }
    Some(IngestEvent {
        session_id: session_id.to_string(),
        turns,
        context: build_context(
            workstation,
            metadata,
            started_at,
            meeting_id,
            Some("summary_bundle"),
            meeting_ended,
        ),
    })
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

    fn item(id: &str, text: &str) -> Item {
        Item {
            id: id.to_string(),
            text: text.to_string(),
            detail: None,
            t: 0,
            meta: None,
        }
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
    fn summary_event_groups_actions_highlights_questions() {
        let mut by_mode = HashMap::new();
        by_mode.insert(
            "actions".into(),
            vec![item("a1", "Send recap"), item("a2", "Follow up with PM")],
        );
        by_mode.insert("highlights".into(), vec![item("h1", "Helix shipped v2")]);
        by_mode.insert("open_questions".into(), vec![item("q1", "Pricing tier?")]);
        // Transcript items must be skipped.
        by_mode.insert("transcript".into(), vec![item("t1", "We talked.")]);

        let ev = build_summary_event("s", "h", &meta(&[]), ts(), None, None, &by_mode)
            .expect("non-empty");
        assert_eq!(ev.turns.len(), 3);
        assert!(ev.turns[0].content.starts_with("Action items:"));
        assert!(ev.turns[0].content.contains("- Send recap"));
        assert!(ev.turns[0].content.contains("- Follow up with PM"));
        assert!(ev.turns[1].content.starts_with("Highlights:"));
        assert!(ev.turns[2].content.starts_with("Open questions:"));
        // Transcript not duplicated here.
        assert!(!ev.turns.iter().any(|t| t.content.contains("We talked")));
    }

    #[test]
    fn summary_event_skips_empty_modes() {
        let mut by_mode = HashMap::new();
        by_mode.insert("actions".into(), vec![item("a1", "Only this")]);
        by_mode.insert("highlights".into(), vec![]); // empty
        let ev = build_summary_event("s", "h", &meta(&[]), ts(), None, None, &by_mode)
            .expect("non-empty");
        assert_eq!(ev.turns.len(), 1);
        assert!(ev.turns[0].content.starts_with("Action items:"));
    }

    #[test]
    fn summary_event_returns_none_when_nothing_to_push() {
        let ev = build_summary_event("s", "h", &meta(&[]), ts(), None, None, &HashMap::new());
        assert!(ev.is_none());
    }

    #[test]
    fn summary_event_carries_meeting_ended_attribute_when_provided() {
        let mut by_mode = HashMap::new();
        by_mode.insert("actions".into(), vec![item("a1", "follow up")]);
        let end = chrono::DateTime::parse_from_rfc3339("2026-05-04T13:30:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let ev = build_summary_event(
            "s",
            "h",
            &meta(&[]),
            ts(),
            Some("m-42"),
            Some(end),
            &by_mode,
        )
        .expect("non-empty");
        assert_eq!(
            ev.context.attributes.get("meeting_ended"),
            Some(&end.to_rfc3339()),
            "meeting_ended should land in attributes when set"
        );
        // meeting_id rides along too.
        assert_eq!(
            ev.context.attributes.get("meeting_id"),
            Some(&"m-42".to_string()),
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
