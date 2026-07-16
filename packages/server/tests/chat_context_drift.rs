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
//!
//! COVERAGE NOTE (read this before trusting the two tests below to mean
//! more than they do): a first pass at this ratchet added
//! `every_chat_aware_extractor_calls_both_composition_helpers`, which
//! greps the whole `summarize.rs` / `wrap_up.rs` / `backfill.rs` files
//! for `extractor_system_prompt` / `compose_extractor_input`. That
//! catches an extractor's OWN implementation reverting to raw-transcript
//! handling, but it does NOT catch the bug that actually shipped:
//! deleting the `summarize::run(...)` CALL from `process_retry` leaves
//! `summarize.rs` itself completely untouched (it still contains both
//! helper calls inside `summarize::run`'s body), so that test keeps
//! passing while the historical bug is reproduced verbatim. This file
//! was caught making exactly that mistake in review — see
//! `each_post_meeting_site_invokes_every_extractor` below, which is the
//! test that actually closes the gap, and was verified to FAIL when the
//! historical bug is re-injected (delete the `summarize::run(...)` call
//! from `process_retry`).

use std::fs;
use std::path::Path;

fn read_src(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// Strip `//` comment lines so a deleted CALL can't be "found" just
/// because a doc comment still mentions the function name in prose.
/// `wrap_up.rs` alone mentions `summarize::run` in at least four
/// comments (module doc, `extract`'s replace-strategy note,
/// `process_retry`'s doc comment, and the inline note above the
/// `tokio::join!`) — none of those are a call.
fn strip_comment_lines(src: &str) -> String {
    src.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Source text of one function's body, comments stripped. The function
/// is located by its signature and assumed to end at the first
/// flush-left `}` line after it (Rust's standard top-level item
/// closing brace at 0 indent) — good enough for the handful of
/// well-formatted worker functions this test scopes into.
fn fn_body(src: &str, sig: &str) -> String {
    let start = src
        .find(sig)
        .unwrap_or_else(|| panic!("{sig} not found — did it get renamed or moved?"));
    let rest = &src[start..];
    let end = rest.find("\n}\n").map(|i| i + 3).unwrap_or(rest.len());
    strip_comment_lines(&rest[..end])
}

/// The gap that actually bit this codebase: `process_retry` silently
/// omitting an entire extractor call. Assert each post-meeting call
/// site's function BODY (not the whole file — `wrap_up.rs` both
/// DEFINES `extract` and CALLS it unqualified from `process_retry`, so
/// a whole-file `contains("extract(")` would match the definition and
/// prove nothing about whether it's actually called) contains a
/// call-shaped pattern (with the opening paren, so prose mentioning the
/// bare name doesn't count) for `load_chat_context`, `summarize::run`,
/// the actions/open-questions extractor, and `backfill::run`.
///
/// This is intentionally a source-text check, not a runtime one: the
/// point is to catch the call being physically deleted from the
/// function body, the same way it was deleted once before.
#[test]
fn each_post_meeting_site_invokes_every_extractor() {
    let finalize_body = strip_comment_lines(&read_src("workers/finalize.rs"));
    for call in [
        "load_chat_context(",
        "summarize::run(",
        "wrap_up::extract(",
        "backfill::run(",
    ] {
        assert!(
            finalize_body.contains(call),
            "workers/finalize.rs no longer calls `{call}` — the normal \
             post-meeting finalize path must invoke ALL THREE post-meeting \
             extractors (summarize, wrap_up::extract, backfill) on chat-aware \
             input, or an artifact silently stops absorbing the wearer's chat \
             corrections. This is the bug class that shipped once already: \
             process_retry omitted an entire extractor call rather than \
             composing its input differently."
        );
    }

    let retry_body = fn_body(&read_src("workers/wrap_up.rs"), "async fn process_retry(");
    for call in [
        "load_chat_context(",
        "summarize::run(",
        "extract(",
        "backfill::run(",
    ] {
        assert!(
            retry_body.contains(call),
            "workers/wrap_up.rs's process_retry no longer calls `{call}` — \
             this is scoped to process_retry's OWN function body (comments \
             stripped), not the whole file, specifically so a deleted call \
             can't hide behind wrap_up.rs still DEFINING `extract` or behind \
             a doc comment mentioning `summarize::run` in prose. The exact \
             historical bug this guards: process_retry (the boot-time retry \
             for a wrap-up interrupted by a redeploy) used to skip \
             summarize::run entirely, so a retried meeting recovered its \
             actions/open_questions but silently lost its summary/highlights \
             — and with them, any wearer chat correction that only the \
             summary/highlights extractor would have applied. Restore the \
             call inside process_retry; don't delete this test."
        );
    }
}

/// Weaker guard, kept because it still catches a REAL (if less severe)
/// drift mode: a call site reimplementing chat-loading instead of
/// using the shared `load_chat_context` helper, so the two post-meeting
/// paths (normal finalize vs. boot-time wrap_up retry) could render
/// chat differently even though both still call every extractor. It
/// does NOT catch an extractor call being omitted entirely — that's
/// what `each_post_meeting_site_invokes_every_extractor` above is for.
#[test]
fn both_post_meeting_sites_load_chat_via_the_shared_helper() {
    for file in ["workers/finalize.rs", "workers/wrap_up.rs"] {
        let text = read_src(file);
        assert!(
            text.contains("load_chat_context"),
            "{file} no longer calls chat_context::load_chat_context — the two \
             post-meeting sites (normal finalize + boot-time wrap_up retry) must \
             load chat identically or they'll silently drift apart on what the \
             extractors see."
        );
    }
}

/// Weaker guard, kept because it still catches a REAL drift mode: an
/// extractor's OWN implementation reverting to raw-transcript handling
/// (dropping the block-grammar prompt or the escaped [chat]/[transcript]
/// composition) internally. It does NOT catch a call site omitting the
/// extractor call altogether — a deleted `summarize::run(...)` call in
/// `process_retry` leaves `summarize.rs` itself, and therefore this
/// test, completely unaffected. See the module doc's COVERAGE NOTE and
/// `each_post_meeting_site_invokes_every_extractor` above for the check
/// that actually closes that gap.
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
             regressing to raw-transcript-only extraction."
        );
    }
}
