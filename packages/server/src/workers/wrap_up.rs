//! Post-meeting wrap-up extractor.
//!
//! After `stop_meeting` lands, the live agent has been torn down
//! along with the in-memory `items_per_mode` buckets. This module
//! takes the captured transcript (snapshotted in `handle_stop_meeting`
//! before the wipe) and runs ONE focused LLM call to pull
//! actions + open_questions out of the full conversation in a single
//! pass.
//!
//! Why post-hoc instead of live: actions/open_questions are most
//! accurate when the model sees the WHOLE meeting at once — including
//! resolution context ("actually never mind", a question answered
//! 20 minutes after it was raised, etc.). The live agent's
//! incremental view tends to over-emit and miss resolutions.
//!
//! Persistence path: the in-memory state has already been cleared
//! (meeting is idle), so we write directly to the DB via
//! `storage::items::replace_items_for_meeting_mode` (replace strategy,
//! so re-running on a retry is idempotent). The standard `persist_items_update`
//! consumer in `persistence.rs` looks up the user's
//! `current_meeting_id`, which is `None` post-stop, so broadcasting
//! an `ItemsUpdate` here would be a no-op for persistence. The
//! direct DB write is the source of truth; clients see the items
//! when they open the past-meeting view (which reads from the DB).
//!
//! Status: failure logs only for v1. A `meetings.wrap_up_status`
//! column + UI banner is a follow-up that needs a migration.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::api::WrapUpRetry;
use crate::context::ServerHandle;
use crate::llm::{ExtractionError, LlmClient};
use crate::protocol::Item;
use crate::storage;

/// Single action item the LLM extracts from the transcript. Fields
/// mirror the live `push_action` tool's shape so the past-meeting
/// view (which renders items_by_mode["actions"]) sees the same
/// schema either way.
#[derive(Debug, Deserialize, Serialize, JsonSchema, Clone)]
pub struct ExtractedAction {
    /// The action statement, e.g. "Send the design doc by EOW".
    pub text: String,
    /// Person responsible, when stated explicitly in the transcript.
    /// `None` (not empty string) when no owner was named.
    #[serde(default)]
    pub owner: Option<String>,
    /// Deadline, when stated. Free-form ("next week", "by Friday",
    /// "EOM") — no parsing into structured dates here.
    #[serde(default)]
    pub due: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, Clone)]
pub struct ExtractedOpenQuestion {
    pub question: String,
    /// Optional category, e.g. "factual" / "decision" / "design".
    #[serde(default)]
    pub kind: Option<String>,
    /// One-line context for why it's open / what was being discussed
    /// when it came up.
    #[serde(default)]
    pub context: Option<String>,
}

/// The full structured output the LLM produces. Both lists may be
/// empty if the transcript had nothing extractable — that's a valid
/// outcome and gets persisted as zero rows.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct WrapUpExtraction {
    pub actions: Vec<ExtractedAction>,
    pub open_questions: Vec<ExtractedOpenQuestion>,
}

const WRAP_UP_PROMPT: &str = "\
You are extracting actions and open questions from a meeting transcript that has just ended.

You see the FULL transcript at once, including the closing minutes — so you can use later context to resolve or cancel earlier statements. Examples:
- \"I'll send the slides\" later followed by \"actually, John will send them\" → ONE action: John sends the slides.
- A question raised at minute 5 and answered at minute 30 → SKIP (resolved).
- A commitment that was retracted later (\"never mind, scrap that\") → SKIP.

ACTIONS — commitments to do something:
- text: short imperative phrasing (\"Send slides\", \"Update the doc\", \"Schedule follow-up\")
- owner: person named or self-referenced (only when explicit — don't infer from context)
- due: deadline if stated, free-form (\"next week\", \"by Friday\", \"EOM\")

OPEN_QUESTIONS — questions that were raised but NOT resolved in the transcript:
- question: the question text, lightly cleaned up
- kind: optional category (\"factual\", \"decision\", \"design\", \"process\")
- context: one-line context for why it's open

Return JSON matching the schema. Empty arrays are valid if nothing applies.
Don't translate — keep the language of the transcript.";

/// Run the wrap-up extractor for a stopped meeting. Spawned as a
/// background task by the ws layer when `IntentOutcome::start_wrap_up`
/// is set; the meeting is already in idle state by the time this
/// runs, so the only state side-effect is the DB write.
pub async fn extract(
    user_id: &str,
    meeting_id: &str,
    transcript_text: &str,
    chat_text: &str,
    llm: &LlmClient,
    db: &sqlx::PgPool,
) {
    info!(
        user_id,
        meeting_id,
        transcript_chars = transcript_text.len(),
        "wrap_up extractor starting",
    );

    // Mark the meeting as 'running' immediately so the past-meeting
    // view can show a "still extracting…" hint if the user opens
    // it before this task completes. Failure to write the status is
    // logged but doesn't abort the extraction — the worst case is
    // a missing transition for THIS run, not a stuck state.
    if let Err(e) = storage::meetings::set_wrap_up_status(db, meeting_id, "running").await {
        warn!(meeting_id, error = ?e, "wrap_up: failed to mark running");
    }

    let system = crate::workers::chat_context::with_chat_authority(WRAP_UP_PROMPT, chat_text);
    let input = crate::workers::chat_context::compose_extractor_input(transcript_text, chat_text);
    let extracted: WrapUpExtraction = match llm
        .extract_with_prompt::<WrapUpExtraction>(user_id, &system, &input)
        .await
    {
        Ok(e) => e,
        Err(ExtractionError::QuotaExhausted(reason)) => {
            warn!(user_id, meeting_id, %reason, "wrap_up skipped: quota exhausted");
            // Quota exhaustion is a server-side condition the user
            // didn't cause — but from their perspective the extractor
            // didn't run. Mark as 'failed' so the UI can show the
            // banner; users can choose to retry once quota recovers.
            if let Err(e) = storage::meetings::set_wrap_up_status(db, meeting_id, "failed").await {
                warn!(meeting_id, error = ?e, "wrap_up: failed to mark failed");
            }
            return;
        }
        Err(e) => {
            warn!(user_id, meeting_id, error = ?e, "wrap_up extraction failed");
            if let Err(e) = storage::meetings::set_wrap_up_status(db, meeting_id, "failed").await {
                warn!(meeting_id, error = ?e, "wrap_up: failed to mark failed");
            }
            return;
        }
    };

    info!(
        user_id,
        meeting_id,
        actions = extracted.actions.len(),
        open_questions = extracted.open_questions.len(),
        "wrap_up extraction complete",
    );

    // Persist directly — `persist_items_update` short-circuits when
    // current_meeting_id is None (which it is post-stop), so emitting
    // ItemsUpdate events would have no persistence effect.
    //
    // Replace strategy (not append): a retry re-runs this on a meeting
    // that may already hold actions/open_questions from a prior run, and
    // every item gets a fresh random UUID, so an append would duplicate
    // the whole set on each retry. `replace_items_for_meeting_mode`
    // clears the `(meeting_id, mode)` slice first, making the extractor
    // idempotent. On the live finalize path there are no prior rows, so
    // replace == insert and behaviour there is unchanged. Mirrors
    // `summarize::run`, the sibling extractor.
    let action_items = build_action_items(&extracted.actions);
    let question_items = build_open_question_items(&extracted.open_questions);
    if let Err(e) =
        storage::items::replace_items_for_meeting_mode(db, meeting_id, "actions", &action_items)
            .await
    {
        warn!(meeting_id, error = ?e, "wrap_up: replace actions failed");
    }
    if let Err(e) = storage::items::replace_items_for_meeting_mode(
        db,
        meeting_id,
        "open_questions",
        &question_items,
    )
    .await
    {
        warn!(meeting_id, error = ?e, "wrap_up: replace open_questions failed");
    }

    // Mark as success even when the LLM emitted zero items — that
    // means the meeting had nothing to extract (canceled / silent /
    // no commitments or questions), which is a legitimate outcome,
    // not a failure. The UI banner only fires on explicit 'failed'.
    if let Err(e) = storage::meetings::set_wrap_up_status(db, meeting_id, "success").await {
        warn!(meeting_id, error = ?e, "wrap_up: failed to mark success");
    }
}

/// Map extracted actions to `Item` rows. `a-<uuid>` ids, `t: 0`, and
/// `meta: {"owner","due"}` only for non-empty values — mirroring the
/// live `push_action` tool so the past-meeting view renders them
/// identically. Pulled out for unit testing.
fn build_action_items(actions: &[ExtractedAction]) -> Vec<Item> {
    actions
        .iter()
        .map(|a| {
            let mut meta = serde_json::Map::new();
            if let Some(owner) = a.owner.as_deref().filter(|s| !s.trim().is_empty()) {
                meta.insert("owner".into(), serde_json::Value::String(owner.to_string()));
            }
            if let Some(due) = a.due.as_deref().filter(|s| !s.trim().is_empty()) {
                meta.insert("due".into(), serde_json::Value::String(due.to_string()));
            }
            Item {
                id: format!("a-{}", uuid::Uuid::new_v4()),
                text: a.text.clone(),
                detail: None,
                t: 0,
                meta: if meta.is_empty() {
                    None
                } else {
                    Some(serde_json::Value::Object(meta))
                },
            }
        })
        .collect()
}

/// Map extracted open questions to `Item` rows. `q-<uuid>` ids, `t: 0`,
/// `meta: {"kind","context"}` only for non-empty values. Pulled out for
/// unit testing.
fn build_open_question_items(questions: &[ExtractedOpenQuestion]) -> Vec<Item> {
    questions
        .iter()
        .map(|q| {
            let mut meta = serde_json::Map::new();
            if let Some(kind) = q.kind.as_deref().filter(|s| !s.trim().is_empty()) {
                meta.insert("kind".into(), serde_json::Value::String(kind.to_string()));
            }
            if let Some(context) = q.context.as_deref().filter(|s| !s.trim().is_empty()) {
                meta.insert(
                    "context".into(),
                    serde_json::Value::String(context.to_string()),
                );
            }
            Item {
                id: format!("q-{}", uuid::Uuid::new_v4()),
                text: q.question.clone(),
                detail: None,
                t: 0,
                meta: if meta.is_empty() {
                    None
                } else {
                    Some(serde_json::Value::Object(meta))
                },
            }
        })
        .collect()
}

/// Reconstruct the extractor's transcript input from the persisted
/// transcript items. The live finalize path feeds `extract` a
/// newline-joined string (see `finalize::assemble_transcript`); the
/// JSONL blob stores one `Item` per committed transcript chunk, so
/// joining their `text` reproduces the same shape on a retry.
fn join_transcript(items: &[Item]) -> String {
    items
        .iter()
        .map(|i| i.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Spawn the wrap-up retry worker. One subscriber to
/// `wrap_up_retry_tx`; lives for the server lifetime. Each retry
/// signal re-runs BOTH post-meeting extractors (summary+highlights and
/// actions+open_questions) on the meeting's persisted transcript,
/// exactly as the live finalize path would — the only difference is the
/// transcript comes off disk instead of the in-flight STT drain.
pub fn spawn_retry_worker(handle: ServerHandle) {
    let mut rx = handle.wrap_up_retry_tx.subscribe();
    let llm = handle.background_llm.clone();
    let db = handle.db.clone();
    // Fan-outs go through the finalize TaskTracker so shutdown waits
    // for an in-flight retry the same way it waits for a finalize. The
    // outer loop below stays a bare spawn — it never finishes, and
    // tracking it would deadlock the tracker's wait().
    let tracker = handle.tasks.clone();
    tokio::spawn(async move {
        info!("wrap-up retry worker started");
        loop {
            match rx.recv().await {
                Ok(req) => {
                    // Fan out per-meeting so a long transcript on one
                    // retry doesn't block the next.
                    let db = db.clone();
                    let llm = llm.clone();
                    tracker.spawn(async move {
                        process_retry(&db, &llm, &req).await;
                    });
                }
                Err(broadcast::error::RecvError::Closed) => return,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(lagged = n, "wrap-up retry worker lagged");
                }
            }
        }
    });
}

/// Boot-time recovery for finalizes a previous process killed mid-run
/// (e.g. a redeploy seconds after `StopMeeting`). A killed finalize
/// leaves `ended_at` set and `wrap_up_status` stuck at `'running'` — a
/// state `POST /meetings/:id/retry-wrap-up` rejects with
/// `already_running`, so without this scan the meeting loses its
/// summary/actions permanently. Publishes one `WrapUpRetry` per
/// interrupted meeting onto the retry worker's channel and returns how
/// many were sent. Call AFTER `spawn_retry_worker`: the worker
/// subscribes synchronously before its task spawns, so these sends are
/// guaranteed a receiver. Idempotent across boots — the retry worker
/// always drives the status to a terminal `success`/`failed`, so each
/// interruption is re-kicked at most once.
pub async fn rekick_interrupted(db: &sqlx::PgPool, tx: &broadcast::Sender<WrapUpRetry>) -> usize {
    let rows = match storage::meetings::find_interrupted_wrap_ups(db).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = ?e, "find_interrupted_wrap_ups failed; skipping wrap-up re-kick");
            return 0;
        }
    };
    let mut sent = 0;
    for (user_id, meeting_id) in rows {
        info!(%user_id, %meeting_id, "re-kicking wrap-up interrupted by previous shutdown");
        if tx
            .send(WrapUpRetry {
                user_id,
                meeting_id,
            })
            .is_ok()
        {
            sent += 1;
        }
    }
    sent
}

/// Handle one retry: read the transcript blob, re-run both post-meeting
/// extractors (`summarize::run` for summary+highlights and `extract` for
/// actions+open_questions) on it, mirroring `finalize`. The HTTP handler
/// already flipped the status to `running`; `extract` re-asserts that and
/// owns the terminal `success`/`failed` write plus the actions/questions
/// inserts, while `summarize::run` owns the summary/highlights replace —
/// so this stays a thin adapter from "blob on disk" to "the same calls
/// finalize makes".
async fn process_retry(db: &sqlx::PgPool, llm: &Arc<LlmClient>, req: &WrapUpRetry) {
    let items = match storage::persistence_loop::read_transcription(&req.meeting_id).await {
        Ok(items) => items,
        Err(e) => {
            warn!(meeting_id = %req.meeting_id, error = ?e, "wrap_up retry: read transcript failed");
            if let Err(e) =
                storage::meetings::set_wrap_up_status(db, &req.meeting_id, "failed").await
            {
                warn!(meeting_id = %req.meeting_id, error = ?e, "wrap_up retry: failed to mark failed");
            }
            return;
        }
    };
    let transcript_text = join_transcript(&items);
    if transcript_text.trim().is_empty() {
        // No persisted transcript to extract from. Mirror finalize's
        // empty-transcript branch: there's nothing to do, so clear the
        // failed banner by marking success rather than looping forever.
        info!(meeting_id = %req.meeting_id, "wrap_up retry: empty transcript; marking success");
        if let Err(e) = storage::meetings::set_wrap_up_status(db, &req.meeting_id, "success").await
        {
            warn!(meeting_id = %req.meeting_id, error = ?e, "wrap_up retry: failed to mark success");
        }
        return;
    }
    // Mirror the live finalize path (see `workers::finalize`): regenerate
    // summary + highlights (`summarize::run`) alongside actions +
    // open_questions (`extract`) on the complete transcript, in parallel.
    // The retry worker historically ran ONLY `extract`, so a retried
    // meeting recovered its actions/open_questions but never got its
    // summary/highlights back — the two post-meeting extractors are
    // siblings and finalize always runs both, so retry must too.
    // `backfill::run` is included for the same reason: finalize runs it
    // too, and it only fills genuinely-empty title/description fields —
    // a no-op for meetings that already have them.
    let chat_text = crate::workers::chat_context::load_chat_context(db, &req.meeting_id).await;
    tokio::join!(
        crate::workers::summarize::run(
            &req.user_id,
            &req.meeting_id,
            &transcript_text,
            &chat_text,
            llm,
            db
        ),
        extract(
            &req.user_id,
            &req.meeting_id,
            &transcript_text,
            &chat_text,
            llm,
            db
        ),
        crate::workers::backfill::run(
            &req.user_id,
            &req.meeting_id,
            &transcript_text,
            &chat_text,
            llm,
            db
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str) -> Item {
        Item {
            id: "x".into(),
            text: text.into(),
            detail: None,
            t: 0,
            meta: None,
        }
    }

    #[test]
    fn join_transcript_newline_joins_item_text() {
        let items = vec![item("hello there"), item("how are you")];
        assert_eq!(join_transcript(&items), "hello there\nhow are you");
    }

    #[test]
    fn join_transcript_empty_items_is_empty_string() {
        assert_eq!(join_transcript(&[]), "");
    }

    #[test]
    fn build_action_items_maps_text_and_meta() {
        let actions = vec![
            ExtractedAction {
                text: "Send slides".into(),
                owner: Some("John".into()),
                due: Some("Friday".into()),
            },
            ExtractedAction {
                text: "Book room".into(),
                owner: None,
                due: Some("  ".into()),
            },
        ];
        let items = build_action_items(&actions);
        assert_eq!(items.len(), 2);
        assert!(items[0].id.starts_with("a-"));
        assert_eq!(items[0].t, 0);
        assert_eq!(items[0].meta.as_ref().unwrap()["owner"], "John");
        assert_eq!(items[0].meta.as_ref().unwrap()["due"], "Friday");
        // Owner absent, due whitespace-only → no meta at all.
        assert!(items[1].meta.is_none());
    }

    #[test]
    fn build_open_question_items_maps_text_and_meta() {
        let questions = vec![
            ExtractedOpenQuestion {
                question: "Who owns rollout?".into(),
                kind: Some("decision".into()),
                context: Some("raised at minute 5".into()),
            },
            ExtractedOpenQuestion {
                question: "What's the budget?".into(),
                kind: Some("   ".into()),
                context: None,
            },
        ];
        let items = build_open_question_items(&questions);
        assert_eq!(items.len(), 2);
        assert!(items[0].id.starts_with("q-"));
        assert_eq!(items[0].meta.as_ref().unwrap()["kind"], "decision");
        assert_eq!(
            items[0].meta.as_ref().unwrap()["context"],
            "raised at minute 5"
        );
        assert!(items[1].meta.is_none());
    }

    #[test]
    fn build_items_handle_empty_input() {
        assert!(build_action_items(&[]).is_empty());
        assert!(build_open_question_items(&[]).is_empty());
    }

    #[sqlx::test]
    async fn rekick_interrupted_publishes_retry_per_interrupted_meeting(pool: sqlx::PgPool) {
        use crate::storage::meetings::{end_meeting, insert_meeting, set_wrap_up_status};
        use crate::storage::users::upsert_user_by_auth0_sub;

        let sub = format!("test|{}", uuid::Uuid::new_v4());
        let uid = upsert_user_by_auth0_sub(&pool, &sub, None, None)
            .await
            .unwrap()
            .id;
        let now = chrono::Utc::now();

        // Interrupted: ended + stuck 'running' → must be re-kicked.
        let interrupted = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &interrupted, &uid, now, None, "{}", None)
            .await
            .unwrap();
        end_meeting(&pool, &interrupted, now).await.unwrap();
        set_wrap_up_status(&pool, &interrupted, "running")
            .await
            .unwrap();

        // Completed: ended + 'success' → ignored.
        let completed = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &completed, &uid, now, None, "{}", None)
            .await
            .unwrap();
        end_meeting(&pool, &completed, now).await.unwrap();
        set_wrap_up_status(&pool, &completed, "success")
            .await
            .unwrap();

        // Live meeting → ignored (live-meeting boot recovery's job).
        let live = uuid::Uuid::new_v4().to_string();
        insert_meeting(&pool, &live, &uid, now, None, "{}", None)
            .await
            .unwrap();
        set_wrap_up_status(&pool, &live, "running").await.unwrap();

        // Subscribe BEFORE calling — mirrors the boot ordering contract
        // (spawn_retry_worker subscribes synchronously before its task
        // spawns, so the re-kick's sends always have a receiver).
        let (tx, mut rx) = broadcast::channel::<WrapUpRetry>(8);
        let kicked = rekick_interrupted(&pool, &tx).await;

        assert_eq!(kicked, 1, "exactly one interrupted wrap-up re-kicked");
        let req = rx.try_recv().expect("one WrapUpRetry published");
        assert_eq!(req.meeting_id, interrupted);
        assert_eq!(req.user_id, uid);
        assert!(
            matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
            "no extra retry signals"
        );
    }
}
