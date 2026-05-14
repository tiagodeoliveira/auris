//! Recall types + URL builder for mnemo's `GET /recall` endpoint.
//!
//! mnemo's recall is opt-in by dimension — boolean flags (`about`,
//! `preferences`, `episodes`) and string scopes (`project`, `task`,
//! `date`). We default to `about + preferences + episodes` (generic,
//! cross-source memories about the user) and add `project` when the
//! current meeting's metadata has one.

use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct RecallParams {
    pub preferences: bool,
    pub about: bool,
    pub episodes: bool,
    pub project: Option<String>,
    /// Scope the recall to memories pushed under a specific meeting.
    pub meeting_id: Option<String>,
}

impl RecallParams {
    /// Defaults for an in-meeting recall: generic dimensions on, optional
    /// project scope when the meeting's metadata has one.
    pub fn for_meeting(project: Option<String>) -> Self {
        Self {
            preferences: true,
            about: true,
            episodes: true,
            project,
            meeting_id: None,
        }
    }

    /// Targeted recall against a specific past meeting. Disables the
    /// generic-dimension flags so the response is *just* memories
    /// pushed under that meeting's id.
    pub fn for_meeting_id(meeting_id: String) -> Self {
        Self {
            preferences: false,
            about: false,
            episodes: false,
            project: None,
            meeting_id: Some(meeting_id),
        }
    }

    /// Render to a `?key=value&...` query string (no leading `?`).
    pub fn to_query(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.preferences {
            parts.push("preferences=true".into());
        }
        if self.about {
            parts.push("about=true".into());
        }
        if self.episodes {
            parts.push("episodes=true".into());
        }
        if let Some(p) = &self.project {
            if !p.is_empty() {
                parts.push(format!("project={}", urlencode(p)));
            }
        }
        if let Some(id) = &self.meeting_id {
            if !id.is_empty() {
                parts.push(format!("meeting={}", urlencode(id)));
            }
        }
        parts.join("&")
    }

    /// True if at least one dimension is requested.
    pub fn has_any(&self) -> bool {
        self.preferences
            || self.about
            || self.episodes
            || self.project.is_some()
            || self.meeting_id.is_some()
    }
}

/// Minimal URL-encoder for query values. Only encodes characters that
/// would break a URL: space, &, =, +, ?, #, /. Sufficient for project
/// names; for arbitrary input we'd reach for a real percent-encoder.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '&' => out.push_str("%26"),
            '=' => out.push_str("%3D"),
            '+' => out.push_str("%2B"),
            '?' => out.push_str("%3F"),
            '#' => out.push_str("%23"),
            '/' => out.push_str("%2F"),
            _ => out.push(c),
        }
    }
    out
}

/// One recalled memory item (v2 shape).
#[derive(Debug, Clone, Deserialize)]
pub struct RecalledItem {
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub reinforced_count: i32,
    #[serde(default)]
    pub similarity: Option<f32>,
}

/// One dimension group in the recall response.
#[derive(Debug, Clone, Deserialize)]
pub struct RecalledDimension {
    pub dimension: String,
    pub namespace: String,
    pub items: Vec<RecalledItem>,
}

/// mnemo's `/recall` response (v2 item-shape).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RecalledContext {
    #[serde(default)]
    pub dimensions: Vec<RecalledDimension>,
}

impl RecalledContext {
    pub fn is_empty(&self) -> bool {
        self.dimensions.iter().all(|d| d.items.is_empty())
    }

    /// Counts of records per known dimension. Used to push a small visibility
    /// summary to the PWA. Unknown dimensions are counted under their closest
    /// category; project/task/meeting/daily → project_memories.
    pub fn summary(&self) -> crate::contract::PriorContextSummary {
        let mut preferences = 0usize;
        let mut facts = 0usize;
        let mut episodes = 0usize;
        let mut project_memories = 0usize;
        for dim in &self.dimensions {
            let n = dim.items.len();
            match dim.dimension.as_str() {
                "preferences" => preferences += n,
                "about" | "facts" => facts += n,
                "episodes" => episodes += n,
                _ => project_memories += n,
            }
        }
        crate::contract::PriorContextSummary {
            preferences,
            facts,
            episodes,
            project_memories,
        }
    }

    /// Render as a markdown preamble for LLM prompts. Empty string when
    /// nothing was recalled, so callers can prepend unconditionally.
    pub fn format_for_prompt(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut out = String::from("## Prior context (from past meetings)\n\n");
        for dim in &self.dimensions {
            if dim.items.is_empty() {
                continue;
            }
            let heading = match dim.dimension.as_str() {
                "preferences" => "Preferences",
                "about" => "About",
                "facts" => "Facts",
                "episodes" => "Past discussions",
                "project" => "Project memories",
                "task" => "Task memories",
                "meeting" => "Meeting notes",
                other => other,
            };
            out.push_str(&format!("### {heading}\n"));
            for item in &dim.items {
                out.push_str("- ");
                out.push_str(item.content.trim());
                out.push('\n');
            }
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_string_orders_and_encodes() {
        let p = RecallParams {
            preferences: true,
            about: true,
            episodes: false,
            project: Some("helix project".into()),
            meeting_id: None,
        };
        let q = p.to_query();
        assert!(q.contains("preferences=true"));
        assert!(q.contains("about=true"));
        assert!(!q.contains("episodes=true"));
        assert!(q.contains("project=helix%20project"));
    }

    #[test]
    fn empty_project_skipped() {
        let p = RecallParams {
            preferences: true,
            about: false,
            episodes: false,
            project: Some(String::new()),
            meeting_id: None,
        };
        let q = p.to_query();
        assert_eq!(q, "preferences=true");
    }

    #[test]
    fn for_meeting_id_scopes_query() {
        let p = RecallParams::for_meeting_id("m-42".into());
        let q = p.to_query();
        assert_eq!(q, "meeting=m-42");
        assert!(!q.contains("preferences=true"));
        assert!(!q.contains("about=true"));
        assert!(!q.contains("episodes=true"));
    }

    #[test]
    fn meeting_id_with_special_chars_encoded() {
        let p = RecallParams::for_meeting_id("uuid with/slash".into());
        assert!(p.to_query().contains("meeting=uuid%20with%2Fslash"));
    }

    #[test]
    fn has_any_recognizes_meeting_id() {
        let p = RecallParams {
            preferences: false,
            about: false,
            episodes: false,
            project: None,
            meeting_id: Some("m-1".into()),
        };
        assert!(p.has_any());
    }

    #[test]
    fn for_meeting_sets_generic_dims() {
        let p = RecallParams::for_meeting(Some("helix".into()));
        assert!(p.preferences);
        assert!(p.about);
        assert!(p.episodes);
        assert_eq!(p.project.as_deref(), Some("helix"));
    }

    #[test]
    fn deserialize_full_response() {
        let json = r#"{
            "dimensions": [
                {
                    "dimension": "preferences",
                    "namespace": "/preferences/actor1/",
                    "items": [
                        {"id":"p1","content":"Prefers async","tags":[],"created_at":"2026-04-01T10:00:00Z","updated_at":"2026-04-01T10:00:00Z","reinforced_count":3}
                    ]
                },
                {
                    "dimension": "about",
                    "namespace": "/about/actor1/",
                    "items": []
                },
                {
                    "dimension": "episodes",
                    "namespace": "/episodes/actor1/",
                    "items": [
                        {"id":"e1","content":"Discussed pricing","tags":["sales"],"created_at":"2026-04-02T10:00:00Z","updated_at":"2026-04-02T10:00:00Z","reinforced_count":1}
                    ]
                }
            ]
        }"#;
        let r: RecalledContext = serde_json::from_str(json).unwrap();
        assert_eq!(r.dimensions.len(), 3);
        let pref = r
            .dimensions
            .iter()
            .find(|d| d.dimension == "preferences")
            .unwrap();
        assert_eq!(pref.items.len(), 1);
        assert_eq!(pref.items[0].content, "Prefers async");
        let ep = r
            .dimensions
            .iter()
            .find(|d| d.dimension == "episodes")
            .unwrap();
        assert_eq!(ep.items.len(), 1);
    }

    #[test]
    fn deserialize_empty_response() {
        let r: RecalledContext = serde_json::from_str("{\"dimensions\":[]}").unwrap();
        assert!(r.is_empty());
        assert_eq!(r.format_for_prompt(), "");
    }

    #[test]
    fn deserialize_missing_dimensions_field() {
        let r: RecalledContext = serde_json::from_str("{}").unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn format_includes_all_present_dimensions() {
        let json = r#"{
            "dimensions": [
                {
                    "dimension": "preferences",
                    "namespace": "/preferences/actor1/",
                    "items": [
                        {"id":"p1","content":"Prefers async","tags":[],"created_at":"2026-04-01T10:00:00Z","updated_at":"2026-04-01T10:00:00Z","reinforced_count":3}
                    ]
                },
                {
                    "dimension": "about",
                    "namespace": "/about/actor1/",
                    "items": [
                        {"id":"a1","content":"User is in CET","tags":[],"created_at":"2026-04-01T10:00:00Z","updated_at":"2026-04-01T10:00:00Z","reinforced_count":1}
                    ]
                }
            ]
        }"#;
        let r: RecalledContext = serde_json::from_str(json).unwrap();
        let p = r.format_for_prompt();
        assert!(p.starts_with("## Prior context"));
        assert!(p.contains("### Preferences"));
        assert!(p.contains("- Prefers async"));
        assert!(p.contains("### About"));
        assert!(p.contains("- User is in CET"));
    }

    #[test]
    fn summary_maps_dimensions_correctly() {
        let json = r#"{
            "dimensions": [
                {"dimension":"preferences","namespace":"/preferences/a/","items":[
                    {"id":"p1","content":"x","tags":[],"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","reinforced_count":1},
                    {"id":"p2","content":"y","tags":[],"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","reinforced_count":1}
                ]},
                {"dimension":"about","namespace":"/about/a/","items":[
                    {"id":"a1","content":"z","tags":[],"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","reinforced_count":1}
                ]},
                {"dimension":"project","namespace":"/projects/a/foo/","items":[
                    {"id":"pr1","content":"w","tags":[],"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","reinforced_count":1}
                ]}
            ]
        }"#;
        let r: RecalledContext = serde_json::from_str(json).unwrap();
        let s = r.summary();
        assert_eq!(s.preferences, 2);
        assert_eq!(s.facts, 1);
        assert_eq!(s.episodes, 0);
        assert_eq!(s.project_memories, 1);
    }
}
