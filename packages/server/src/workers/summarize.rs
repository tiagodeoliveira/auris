//! Post-meeting summary + highlights extractor.
//!
//! Sibling to `workers::wrap_up`. After the STT drain completes, the
//! finalize task runs this on the COMPLETE transcript to regenerate
//! summary + highlights from a whole-meeting view (dedupes, resolves,
//! captures the drained tail) and OVERWRITE the incrementally-built
//! live versions. Writes directly to the DB via the same replace
//! strategy the live `replace_summary` / `replace_highlights` tools use,
//! so the past-meeting view reads them identically.
//!
//! The live in-meeting summary (active agent firing during the meeting)
//! is unchanged; this is the authoritative final pass layered on top.

use crate::llm::{ExtractionError, LlmClient};
use crate::protocol::Item;
use crate::storage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// One highlight the LLM extracts. Mirrors the live `replace_highlights`
/// item shape so the past-meeting view renders it identically.
#[derive(Debug, Deserialize, Serialize, JsonSchema, Clone)]
pub struct ExtractedHighlight {
    pub text: String,
    /// Optional importance tag, e.g. "high" / "medium". `None` when not
    /// stated — mirrors the live tool's optional `importance`.
    #[serde(default)]
    pub importance: Option<String>,
}

/// Structured output: the whole-meeting narrative summary + highlights.
/// The summary may be an empty string and highlights an empty list
/// (silent / contentless meeting) — valid, persisted as zero rows.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct MeetingSummary {
    /// A flowing NARRATIVE recap of the whole meeting, written as
    /// connected prose paragraphs (blank-line separated) — NOT a bullet
    /// list. Captures the meaning and arc of the conversation, not just
    /// disparate facts. Length scales with the meeting's substance.
    /// Empty string when the meeting had nothing to summarize.
    pub summary: String,
    pub highlights: Vec<ExtractedHighlight>,
}

const SUMMARIZE_PROMPT: &str = "\
You are writing the FINAL recap for a meeting transcript that has just ended. A separate, incremental \"running summary\" already gave the wearer live bullet points during the meeting; your job is different — write the authoritative, reflective whole-meeting account they read afterwards.

You see the WHOLE transcript at once, so resolve things that changed during the meeting (a decision reversed later reflects the final state) and tell the real story of what happened.

SUMMARY — a flowing NARRATIVE in prose, not a list of bullets. Capture what this meeting was actually about: the arc of the conversation, what was discussed and decided, the reasoning and any tension behind the key points, and where things ultimately landed. Convey the meaning and the throughline — a reader should come away understanding the meeting's spirit, not just a pile of disconnected facts. Write in connected paragraphs separated by a blank line. Use specific names, numbers, and concrete subjects from the transcript; don't be vague or generic. Let the length follow the substance: a brief meeting gets a tight paragraph or two, a rich one gets several — long enough to be meaningful, never padded, and far shorter than the meeting itself. Do NOT include open questions or follow-up actions (those are separate wrap-up surfaces).

HIGHLIGHTS — 0-10 standalone noteworthy moments a person re-reading the meeting would want to remember (decisions, surprising facts, named entities, specific numbers). SKIP pleasantries, intros, small talk, meta-commentary. Each highlight has `text` and an optional `importance` tag when one is clearly warranted.

Return JSON matching the schema. An empty summary string and/or empty highlights array are valid when there's nothing to capture.
Don't translate — keep the language of the transcript.";

/// Run the summary+highlights extractor for a stopped meeting. Spawned
/// by `workers::finalize` on the complete transcript. The meeting is
/// already idle; the only side-effect is the DB write (replace strategy).
pub async fn run(
    user_id: &str,
    meeting_id: &str,
    transcript_text: &str,
    llm: &LlmClient,
    db: &sqlx::PgPool,
) {
    if transcript_text.trim().is_empty() {
        info!(user_id, meeting_id, "summarize skipped: empty transcript");
        return;
    }

    info!(
        user_id,
        meeting_id,
        transcript_chars = transcript_text.len(),
        "summarize starting",
    );

    let extracted: MeetingSummary = match llm
        .extract_with_prompt::<MeetingSummary>(user_id, SUMMARIZE_PROMPT, transcript_text)
        .await
    {
        Ok(e) => e,
        Err(ExtractionError::QuotaExhausted(reason)) => {
            warn!(user_id, meeting_id, %reason, "summarize skipped: quota exhausted");
            return;
        }
        Err(e) => {
            warn!(user_id, meeting_id, error = ?e, "summarize failed");
            return;
        }
    };

    let (summary_items, highlight_items) = build_items(&extracted);

    if let Err(e) =
        storage::items::replace_items_for_meeting_mode(db, meeting_id, "summary", &summary_items)
            .await
    {
        warn!(meeting_id, error = ?e, "summarize: replace summary failed");
    }
    if let Err(e) = storage::items::replace_items_for_meeting_mode(
        db,
        meeting_id,
        "highlights",
        &highlight_items,
    )
    .await
    {
        warn!(meeting_id, error = ?e, "summarize: replace highlights failed");
    }

    info!(
        user_id,
        meeting_id,
        summary = summary_items.len(),
        highlights = highlight_items.len(),
        "summarize complete",
    );
}

/// Map the LLM output to `Item` rows. The narrative summary becomes a
/// SINGLE summary item carrying `meta: {"kind": "narrative"}` so the
/// clients render it as flowing prose (no bullet glyph) instead of the
/// live running-summary bullets. Highlights keep the live tool's
/// conventions: `h-<uuid>`, `t: 0`, `meta: {"importance": ...}` when
/// present. Pulled out for unit testing.
fn build_items(extracted: &MeetingSummary) -> (Vec<Item>, Vec<Item>) {
    // The narrative replaces the running bullets at finalize: one item
    // holding the whole prose recap (paragraph breaks preserved). An
    // empty/whitespace narrative yields no item (contentless meeting).
    let narrative = extracted.summary.trim();
    let summary_items: Vec<Item> = if narrative.is_empty() {
        Vec::new()
    } else {
        vec![Item {
            id: format!("summary-{}", uuid::Uuid::new_v4()),
            text: narrative.to_string(),
            detail: None,
            t: 0,
            meta: Some(serde_json::json!({ "kind": "narrative" })),
        }]
    };

    let highlight_items: Vec<Item> = extracted
        .highlights
        .iter()
        .filter(|h| !h.text.trim().is_empty())
        .map(|h| Item {
            id: format!("h-{}", uuid::Uuid::new_v4()),
            text: h.text.trim().to_string(),
            detail: None,
            t: 0,
            meta: h
                .importance
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(|i| serde_json::json!({ "importance": i })),
        })
        .collect();

    (summary_items, highlight_items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_items_maps_narrative_and_highlights() {
        let extracted = MeetingSummary {
            summary: "  The team aligned on the Q3 roadmap.\n\nThey deferred the billing rework.  "
                .into(),
            highlights: vec![
                ExtractedHighlight {
                    text: "A decision was made.".into(),
                    importance: Some("high".into()),
                },
                ExtractedHighlight {
                    text: "  ".into(),
                    importance: None,
                },
            ],
        };
        let (summary, highlights) = build_items(&extracted);

        // The narrative is a SINGLE item, end-trimmed but with its
        // internal paragraph break preserved, tagged as a narrative.
        assert_eq!(summary.len(), 1);
        assert_eq!(
            summary[0].text,
            "The team aligned on the Q3 roadmap.\n\nThey deferred the billing rework."
        );
        assert!(summary[0].id.starts_with("summary-"));
        assert_eq!(summary[0].t, 0);
        assert_eq!(summary[0].meta.as_ref().unwrap()["kind"], "narrative");

        assert_eq!(highlights.len(), 1);
        assert_eq!(highlights[0].text, "A decision was made.");
        assert!(highlights[0].id.starts_with("h-"));
        assert_eq!(highlights[0].meta.as_ref().unwrap()["importance"], "high");
    }

    #[test]
    fn build_items_drops_whitespace_only_importance() {
        let extracted = MeetingSummary {
            summary: String::new(),
            highlights: vec![ExtractedHighlight {
                text: "Has blank importance.".into(),
                importance: Some("   ".into()),
            }],
        };
        let (_s, highlights) = build_items(&extracted);
        assert_eq!(highlights.len(), 1);
        assert!(
            highlights[0].meta.is_none(),
            "whitespace-only importance must not produce meta"
        );
    }

    #[test]
    fn build_items_empty_narrative_yields_no_summary_item() {
        let extracted = MeetingSummary {
            summary: "   \n  ".into(),
            highlights: vec![],
        };
        let (summary, highlights) = build_items(&extracted);
        assert!(
            summary.is_empty(),
            "a whitespace-only narrative must produce no summary item"
        );
        assert!(highlights.is_empty());
    }

    #[test]
    fn build_items_omits_meta_when_no_importance() {
        let extracted = MeetingSummary {
            summary: String::new(),
            highlights: vec![ExtractedHighlight {
                text: "Plain highlight.".into(),
                importance: None,
            }],
        };
        let (_s, highlights) = build_items(&extracted);
        assert_eq!(highlights.len(), 1);
        assert!(highlights[0].meta.is_none());
    }
}
