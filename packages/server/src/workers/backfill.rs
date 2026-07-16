//! Finalize-time backfill of a missing meeting title / description.
//!
//! Most meetings start with a user-typed description, from which the
//! title is derived at read time (`pick_meeting_title`). But a meeting
//! recorded with NO description has no title either — it shows as
//! "Untitled meeting". This worker fills that gap: after the STT drain,
//! `workers::finalize` runs it (in parallel with `summarize` / `wrap_up`)
//! on the COMPLETE transcript. It generates a title and/or description
//! from the transcript, but ONLY for fields that are genuinely empty —
//! it never overwrites anything the user provided.
//!
//! Writes to the auris DB only (the past-meeting view reads it there).
//! mnemo already stored the meeting from the live session and is left
//! as-is for now.

use std::collections::HashMap;

use crate::llm::{ExtractionError, LlmClient};
use crate::storage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Structured output: a title + description generated from the
/// transcript. Each is persisted only if the corresponding field was
/// missing (see `fields_to_backfill`).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct GeneratedMeta {
    /// Concise meeting title in 8 words or fewer, no trailing period.
    title: String,
    /// One or two plain sentences describing what the meeting was about.
    description: String,
}

const BACKFILL_PROMPT: &str = "\
You are titling a meeting transcript that had no title or description.

Produce a concise TITLE (<=8 words, no trailing period) and a short DESCRIPTION (one or two plain sentences) capturing what the meeting was about — the main topic or purpose. Be specific and factual; use names and concrete subjects from the transcript. No filler like \"This meeting covered…\".

Return JSON matching the schema. Keep the language of the transcript — don't translate.";

/// Generate and persist a missing title and/or description for a stopped
/// meeting. Spawned by `workers::finalize` on the complete transcript,
/// in parallel with `summarize` / `wrap_up`. Reads the meeting's current
/// description + metadata, generates only the genuinely-empty fields, and
/// writes them to the auris DB. Best-effort: every failure logs and
/// returns, leaving the meeting as-is.
pub async fn run(
    user_id: &str,
    meeting_id: &str,
    transcript_text: &str,
    chat_text: &str,
    llm: &LlmClient,
    db: &sqlx::PgPool,
) {
    if transcript_text.trim().is_empty() {
        return; // nothing to generate from (finalize already guards this)
    }

    let (description, metadata) =
        match storage::meetings::load_meta_for_backfill(db, meeting_id).await {
            Ok(Some(inputs)) => inputs,
            Ok(None) => {
                warn!(user_id, meeting_id, "backfill: meeting row not found");
                return;
            }
            Err(e) => {
                warn!(user_id, meeting_id, error = ?e, "backfill: failed to read meeting meta");
                return;
            }
        };

    let (need_title, need_description) = fields_to_backfill(description.as_deref(), &metadata);
    if !need_title && !need_description {
        return; // common case: the meeting started with a description
    }

    info!(
        user_id,
        meeting_id,
        need_title,
        need_description,
        transcript_chars = transcript_text.len(),
        "backfill generating missing meeting meta",
    );

    let system = crate::workers::chat_context::with_chat_authority(BACKFILL_PROMPT, chat_text);
    let input = crate::workers::chat_context::compose_extractor_input(transcript_text, chat_text);
    let generated: GeneratedMeta = match llm
        .extract_with_prompt::<GeneratedMeta>(user_id, &system, &input)
        .await
    {
        Ok(g) => g,
        Err(ExtractionError::QuotaExhausted(reason)) => {
            warn!(user_id, meeting_id, %reason, "backfill skipped: quota exhausted");
            return;
        }
        Err(e) => {
            warn!(user_id, meeting_id, error = ?e, "backfill failed");
            return;
        }
    };

    if need_title {
        let title = generated.title.trim();
        if title.is_empty() {
            warn!(
                user_id,
                meeting_id, "backfill: generated title was empty; skipping"
            );
        } else if let Err(e) = storage::meetings::set_meeting_title(db, meeting_id, title).await {
            warn!(meeting_id, error = ?e, "backfill: set_meeting_title failed");
        } else {
            info!(user_id, meeting_id, "backfill: title set");
        }
    }

    if need_description {
        let desc = generated.description.trim();
        if desc.is_empty() {
            warn!(
                user_id,
                meeting_id, "backfill: generated description was empty; skipping"
            );
        } else if let Err(e) =
            storage::meetings::set_meeting_description(db, meeting_id, desc).await
        {
            warn!(meeting_id, error = ?e, "backfill: set_meeting_description failed");
        } else {
            info!(user_id, meeting_id, "backfill: description set");
        }
    }
}

/// Decide which fields need generating, given the meeting's current
/// `description` column and parsed `metadata` map. Returns
/// `(need_title, need_description)`.
///
/// - `need_description`: the description column is NULL or whitespace.
/// - `need_title`: there is no usable title — `metadata.title` is
///   absent/empty AND there is no description to derive one from. When
///   a description exists the title is derivable, so it is NOT missing;
///   when a manual `metadata.title` exists it is NOT missing either.
fn fields_to_backfill(
    description: Option<&str>,
    metadata: &HashMap<String, String>,
) -> (bool, bool) {
    let has_description = description.is_some_and(|d| !d.trim().is_empty());
    let has_title = metadata.get("title").is_some_and(|t| !t.trim().is_empty());
    let need_description = !has_description;
    // The title is only "missing" when there is neither a manual title
    // nor a description to derive one from.
    let need_title = !has_title && !has_description;
    (need_title, need_description)
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
    fn description_present_no_metadata_backfills_nothing() {
        let (need_title, need_desc) = fields_to_backfill(Some("Plan the Q3 roadmap"), &meta(&[]));
        assert!(!need_title, "title derives from the description");
        assert!(!need_desc, "description is already present");
    }

    #[test]
    fn no_description_no_title_backfills_both() {
        let (need_title, need_desc) = fields_to_backfill(None, &meta(&[]));
        assert!(need_title);
        assert!(need_desc);
    }

    #[test]
    fn empty_description_no_title_backfills_both() {
        let (need_title, need_desc) = fields_to_backfill(Some("   \n  "), &meta(&[]));
        assert!(need_title);
        assert!(need_desc);
    }

    #[test]
    fn no_description_with_manual_title_backfills_description_only() {
        let (need_title, need_desc) = fields_to_backfill(None, &meta(&[("title", "Standup")]));
        assert!(!need_title, "a manual title already exists");
        assert!(need_desc, "description is still missing");
    }

    #[test]
    fn whitespace_title_is_treated_as_missing() {
        let (need_title, need_desc) = fields_to_backfill(None, &meta(&[("title", "   ")]));
        assert!(need_title, "a whitespace-only title is not usable");
        assert!(need_desc);
    }

    #[test]
    fn description_present_with_manual_title_backfills_nothing() {
        let (need_title, need_desc) =
            fields_to_backfill(Some("Sync with Ana"), &meta(&[("title", "Weekly sync")]));
        assert!(!need_title);
        assert!(!need_desc);
    }
}
