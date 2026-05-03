//! Metadata extraction from meeting descriptions.
//! Phase 2 step 16: thin wrapper that delegates to LlmClient.
//! See `docs/specs/phase-2-llm-extraction.md`.

use std::collections::HashMap;

use crate::llm::{ExtractionError, LlmClient};

pub async fn extract_metadata(
    client: &LlmClient,
    description: &str,
) -> Result<HashMap<String, String>, ExtractionError> {
    client.extract(description).await
}

/// Manual values win on conflict (architecture-stated rule, server.md §4.5).
pub fn merge_manual_wins(
    extracted: HashMap<String, String>,
    manual: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut out = extracted;
    for (k, v) in manual {
        out.insert(k.clone(), v.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_manual_wins_on_conflict() {
        let extracted = HashMap::from([
            ("project".to_string(), "extracted".to_string()),
            ("title".to_string(), "auto title".to_string()),
        ]);
        let manual = HashMap::from([("project".to_string(), "helix".to_string())]);
        let merged = merge_manual_wins(extracted, &manual);
        assert_eq!(merged.get("project"), Some(&"helix".to_string()));
        assert_eq!(merged.get("title"), Some(&"auto title".to_string()));
    }

    #[test]
    fn merge_manual_wins_with_empty_extracted() {
        let extracted = HashMap::new();
        let manual = HashMap::from([("foo".to_string(), "bar".to_string())]);
        let merged = merge_manual_wins(extracted, &manual);
        assert_eq!(merged.get("foo"), Some(&"bar".to_string()));
    }

    #[test]
    fn merge_manual_wins_with_empty_manual() {
        let extracted = HashMap::from([("title".to_string(), "x".to_string())]);
        let manual = HashMap::new();
        let merged = merge_manual_wins(extracted, &manual);
        assert_eq!(merged.get("title"), Some(&"x".to_string()));
    }
}
