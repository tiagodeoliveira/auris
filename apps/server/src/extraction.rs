//! Simulated LLM metadata extraction. See `docs/specs/server.md` §8.4.

use std::collections::HashMap;

pub fn extract_metadata(description: &str) -> HashMap<String, String> {
    let title = description.split_whitespace().take(8).collect::<Vec<_>>().join(" ");
    HashMap::from([
        ("title".to_string(), title),
        ("project".to_string(), "sim-extracted".to_string()),
    ])
}

/// Manual values win on conflict (architecture-stated rule).
pub fn merge_manual_wins(extracted: HashMap<String, String>, manual: &HashMap<String, String>) -> HashMap<String, String> {
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
    fn extract_takes_first_8_words() {
        let d = "Q1 budget review for the helix product launch and beyond";
        let m = extract_metadata(d);
        assert_eq!(m["title"], "Q1 budget review for the helix product launch");
        assert_eq!(m["project"], "sim-extracted");
    }

    #[test]
    fn merge_manual_wins_on_conflict() {
        let extracted = HashMap::from([
            ("project".into(), "sim-extracted".into()),
            ("title".into(), "auto title".into()),
        ]);
        let manual = HashMap::from([("project".into(), "helix".into())]);
        let merged = merge_manual_wins(extracted, &manual);
        assert_eq!(merged["project"], "helix");
        assert_eq!(merged["title"], "auto title");
    }
}
