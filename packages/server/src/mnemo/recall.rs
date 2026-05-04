//! Recall types + URL builder for mnemo's `GET /recall` endpoint.
//!
//! mnemo's recall is opt-in by dimension — boolean flags (`facts`,
//! `preferences`, `episodes`) and string scopes (`project`, `task`,
//! `date`). We default to `facts + preferences + episodes` (generic,
//! cross-source memories about the user) and add `project` when the
//! current meeting's metadata has one.

use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct RecallParams {
    pub preferences: bool,
    pub facts: bool,
    pub episodes: bool,
    pub project: Option<String>,
}

impl RecallParams {
    /// Defaults for an in-meeting recall: generic dimensions on, optional
    /// project scope when the meeting's metadata has one.
    pub fn for_meeting(project: Option<String>) -> Self {
        Self {
            preferences: true,
            facts: true,
            episodes: true,
            project,
        }
    }

    /// Render to a `?key=value&...` query string (no leading `?`).
    pub fn to_query(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.preferences {
            parts.push("preferences=true".into());
        }
        if self.facts {
            parts.push("facts=true".into());
        }
        if self.episodes {
            parts.push("episodes=true".into());
        }
        if let Some(p) = &self.project {
            if !p.is_empty() {
                parts.push(format!("project={}", urlencode(p)));
            }
        }
        parts.join("&")
    }

    /// True if at least one dimension is requested.
    pub fn has_any(&self) -> bool {
        self.preferences || self.facts || self.episodes || self.project.is_some()
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

#[derive(Debug, Clone, Deserialize)]
pub struct RecalledMemory {
    pub id: String,
    pub content: String,
    pub score: f64,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectMemories {
    pub name: String,
    pub memories: Vec<RecalledMemory>,
}

/// mnemo's `/recall` response. All dimensions are optional — present only
/// when requested *and* yielding records.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RecalledContext {
    #[serde(default)]
    pub preferences: Vec<RecalledMemory>,
    #[serde(default)]
    pub facts: Vec<RecalledMemory>,
    #[serde(default)]
    pub episodes: Vec<RecalledMemory>,
    #[serde(default)]
    pub project: Option<ProjectMemories>,
}

impl RecalledContext {
    pub fn is_empty(&self) -> bool {
        self.preferences.is_empty()
            && self.facts.is_empty()
            && self.episodes.is_empty()
            && self
                .project
                .as_ref()
                .map_or(true, |p| p.memories.is_empty())
    }

    /// Counts of records per dimension. Used to push a small visibility
    /// summary to the PWA without sending the full memory contents over
    /// the wire.
    pub fn summary(&self) -> crate::contract::PriorContextSummary {
        crate::contract::PriorContextSummary {
            preferences: self.preferences.len(),
            facts: self.facts.len(),
            episodes: self.episodes.len(),
            project_memories: self.project.as_ref().map_or(0, |p| p.memories.len()),
        }
    }

    /// Render as a markdown preamble for LLM prompts. Empty string when
    /// nothing was recalled, so callers can prepend unconditionally.
    pub fn format_for_prompt(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut out = String::from("## Prior context (from past meetings)\n\n");
        if !self.preferences.is_empty() {
            out.push_str("### Preferences\n");
            for m in &self.preferences {
                out.push_str("- ");
                out.push_str(m.content.trim());
                out.push('\n');
            }
            out.push('\n');
        }
        if !self.facts.is_empty() {
            out.push_str("### Facts\n");
            for m in &self.facts {
                out.push_str("- ");
                out.push_str(m.content.trim());
                out.push('\n');
            }
            out.push('\n');
        }
        if !self.episodes.is_empty() {
            out.push_str("### Past discussions\n");
            for m in &self.episodes {
                out.push_str("- ");
                out.push_str(m.content.trim());
                out.push('\n');
            }
            out.push('\n');
        }
        if let Some(p) = &self.project {
            if !p.memories.is_empty() {
                out.push_str(&format!("### Project: {}\n", p.name));
                for m in &p.memories {
                    out.push_str("- ");
                    out.push_str(m.content.trim());
                    out.push('\n');
                }
                out.push('\n');
            }
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
            facts: true,
            episodes: false,
            project: Some("helix project".into()),
        };
        let q = p.to_query();
        assert!(q.contains("preferences=true"));
        assert!(q.contains("facts=true"));
        assert!(!q.contains("episodes=true"));
        assert!(q.contains("project=helix%20project"));
    }

    #[test]
    fn empty_project_skipped() {
        let p = RecallParams {
            preferences: true,
            facts: false,
            episodes: false,
            project: Some(String::new()),
        };
        let q = p.to_query();
        assert_eq!(q, "preferences=true");
    }

    #[test]
    fn for_meeting_sets_generic_dims() {
        let p = RecallParams::for_meeting(Some("helix".into()));
        assert!(p.preferences);
        assert!(p.facts);
        assert!(p.episodes);
        assert_eq!(p.project.as_deref(), Some("helix"));
    }

    #[test]
    fn deserialize_full_response() {
        let json = r#"{
            "preferences": [{"id":"p1","content":"Prefers async","score":0.9,"createdAt":"2026-04-01T10:00:00Z"}],
            "facts": [],
            "episodes": [{"id":"e1","content":"Discussed pricing","score":0.7,"createdAt":"2026-04-02T10:00:00Z"}],
            "project": {"name":"helix","memories":[{"id":"pr1","content":"Helix v2 shipped","score":0.8,"createdAt":"2026-03-01T10:00:00Z"}]}
        }"#;
        let r: RecalledContext = serde_json::from_str(json).unwrap();
        assert_eq!(r.preferences.len(), 1);
        assert_eq!(r.preferences[0].content, "Prefers async");
        assert!(r.facts.is_empty());
        assert_eq!(r.episodes.len(), 1);
        assert_eq!(r.project.as_ref().unwrap().memories.len(), 1);
    }

    #[test]
    fn deserialize_empty_response() {
        let r: RecalledContext = serde_json::from_str("{}").unwrap();
        assert!(r.is_empty());
        assert_eq!(r.format_for_prompt(), "");
    }

    #[test]
    fn format_includes_all_present_dimensions() {
        let json = r#"{
            "preferences": [{"id":"p1","content":"Prefers async","score":0.9,"createdAt":"2026-04-01T10:00:00Z"}],
            "facts": [{"id":"f1","content":"User is in CET","score":0.8,"createdAt":"2026-04-01T10:00:00Z"}],
            "episodes": [{"id":"e1","content":"Discussed pricing","score":0.7,"createdAt":"2026-04-02T10:00:00Z"}],
            "project": {"name":"helix","memories":[{"id":"pr1","content":"Helix v2 shipped","score":0.8,"createdAt":"2026-03-01T10:00:00Z"}]}
        }"#;
        let r: RecalledContext = serde_json::from_str(json).unwrap();
        let p = r.format_for_prompt();
        assert!(p.starts_with("## Prior context"));
        assert!(p.contains("### Preferences"));
        assert!(p.contains("- Prefers async"));
        assert!(p.contains("### Facts"));
        assert!(p.contains("### Past discussions"));
        assert!(p.contains("### Project: helix"));
        assert!(p.contains("- Helix v2 shipped"));
    }

    #[test]
    fn project_with_no_memories_omitted_from_format() {
        let json = r#"{
            "facts": [{"id":"f1","content":"Hello","score":0.5,"createdAt":"2026-04-01T10:00:00Z"}],
            "project": {"name":"helix","memories":[]}
        }"#;
        let r: RecalledContext = serde_json::from_str(json).unwrap();
        let p = r.format_for_prompt();
        assert!(p.contains("### Facts"));
        assert!(!p.contains("### Project"));
    }
}
