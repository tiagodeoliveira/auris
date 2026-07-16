//! Drift ratchet for the chat-into-artifacts wiring
//! (`workers::chat_context`).
//!
//! The design's "drift guard" reasoning was circular: it claimed a
//! shared `load_chat_context` function keeps `finalize` and
//! `wrap_up::process_retry` from drifting apart, then called that
//! "enforced by review, not by a test." But the bug this branch fixed
//! was an OMISSION, not a divergence — `process_retry` didn't call the
//! summary/highlights extractor (`summarize::run`) at all (see the
//! historical-drift comment on `process_retry` in
//! `src/workers/wrap_up.rs`). A shared helper can't stop a call site
//! from simply not calling it; only a source-level check that both
//! sites actually invoke the shared machinery closes that gap. Same
//! pattern as `tests/layering.rs` — read source text and assert on it,
//! rather than trusting review to catch a regression that already
//! slipped through review once on this exact pair of functions.

use std::fs;
use std::path::Path;

fn read_src(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// Both post-meeting call sites must load chat via the shared
/// `load_chat_context` helper. This is the "two sites render chat
/// differently" drift mode — but note it's the WEAKER guard: it only
/// catches a site that reimplements chat-loading, not one that omits
/// it entirely. See the module doc and the per-extractor check below
/// for the drift mode that actually bit this codebase.
#[test]
fn both_post_meeting_sites_load_chat_via_the_shared_helper() {
    for file in ["workers/finalize.rs", "workers/wrap_up.rs"] {
        let text = read_src(file);
        assert!(
            text.contains("load_chat_context"),
            "{file} no longer calls chat_context::load_chat_context — the two \
             post-meeting sites (normal finalize + boot-time wrap_up retry) must \
             load chat identically or they'll silently drift apart on what the \
             extractors see, same bug class as the historical omission where \
             process_retry skipped summarize:: entirely (see wrap_up.rs's \
             process_retry doc comment)."
        );
    }
}

/// Every extractor that consumes chat must call BOTH
/// `extractor_system_prompt` (states chat authority + block grammar)
/// and `compose_extractor_input` (builds the escaped [transcript]/
/// [chat] blocks). This is the drift mode that actually happened:
/// `wrap_up::process_retry` used to skip an entire extractor
/// (summarize) rather than call it with different chat handling. If a
/// future edit rips one of these calls out of an extractor — e.g.
/// reverting to a raw transcript string — this fails instead of
/// silently regressing the wearer's chat corrections out of that
/// artifact.
#[test]
fn every_chat_aware_extractor_calls_both_composition_helpers() {
    for file in [
        "workers/summarize.rs",
        "workers/wrap_up.rs",
        "workers/backfill.rs",
    ] {
        let text = read_src(file);
        assert!(
            text.contains("extractor_system_prompt"),
            "{file} no longer calls chat_context::extractor_system_prompt — this \
             extractor would stop stating the block grammar and (when chat is \
             present) the chat-authority rules, silently regressing to a prompt \
             that doesn't explain the [chat]/[transcript] grammar it receives."
        );
        assert!(
            text.contains("compose_extractor_input"),
            "{file} no longer calls chat_context::compose_extractor_input — this \
             extractor would stop receiving chat at all (or receive it unescaped), \
             regressing to raw-transcript-only extraction. This is the exact bug \
             class that shipped once already: wrap_up::process_retry historically \
             omitted an entire extractor call rather than composing its input \
             differently."
        );
    }
}
