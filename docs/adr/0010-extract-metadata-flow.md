# ADR-0010: `ExtractMetadata` as a separate intent from `start_meeting`

**Status:** Accepted
**Date:** 2026-05-04

## Context

When the user types a meeting description, an LLM extracts structured
metadata (project, title, owner, …) into the metadata pane as chips.
Originally this extraction fired _as a side effect_ of
`start_meeting`: the server flipped state to Active and asynchronously
triggered LLM extraction; the chips arrived a couple seconds later.

This worked but had a UX problem: by the time the chips were visible
and editable, the meeting was already running. The user's choices were
"start a meeting that has wrong metadata, then edit," or "stop the
meeting, edit, and start again." Neither felt good.

The user explicitly asked: "what if we get the metadata as soon as we
fill the description? this way we make that separated from the start
meeting, so on meeting start we already know the tags and can edit them."

That breaks the previous coupling apart. The challenge is to do it
without making `start_meeting` _forget_ metadata that the user has
already extracted and possibly edited.

## Decision

- **New intent: `Intent::ExtractMetadata { description }`.** Triggers
  the same LLM extraction pipeline as `start_meeting`'s side effect,
  but does not change `meeting_state`. Result lands in
  `state.metadata` and emits `Event::MetadataChanged`.
- **`start_meeting` preserves metadata when omitted.** The intent's
  `metadata` field changes semantics: previously "this is the metadata"
  (replace), now "this is an override" (replace if `Some`, keep
  existing if `None`). The PWA stops sending the field on Start so
  the server's already-populated metadata survives.
- **Separate cancellation slot for extractions.** A new
  `extraction_cancel: Arc<Mutex<Option<CancellationToken>>>` on the WS
  handle, parallel to `meeting_cancel`. Each new `ExtractMetadata` (or
  the extraction-on-start path) cancels the previous via this slot;
  `stop_meeting` also cancels any in-flight extraction so a stale result
  can't pollute the next idle's empty state. `meeting_cancel` is kept
  meeting-scoped — an idle extract survives `start_meeting` because the
  two slots are independent.
- **Idle is a valid state for `set_metadata_full`.** The previous guard
  ("don't apply extraction results in idle") existed exactly to defend
  against a stop-mid-extraction race; with `extraction_cancel`, that
  guard is no longer load-bearing and is dropped.
- **PWA-side: `EXTRACT TAGS` button on the compose surface.** Visible
  when the description is non-empty and not currently extracting.
  Cmd/Ctrl+Enter inside the textarea is the keyboard shortcut.
  `extractingMetadata: bool` in the store drives a "▸ EXTRACTING…"
  state on the button.

## Consequences

**Positive:**

- The user's natural flow is: type → extract → review chips → start.
  No more "oops the title is wrong, stop and try again."
- Re-running extraction on the same description merges via
  `merge_manual_wins` — the user's chip edits survive a fresh
  extraction cycle. This is effectively a free undo-buffer for LLM
  mistakes.
- The semantics of `start_meeting`'s `metadata` field are now strictly
  more flexible. A future client (CLI, scripted) can still force an
  explicit metadata set; the PWA just never does.

**Negative:**

- Two intents do the same LLM call. If we want to deprecate the
  extraction-on-start path entirely, we'd remove a backward-compat
  affordance. Not urgent.
- The `extraction_cancel` slot duplicates lifecycle logic that
  `meeting_cancel` also expresses. Conceptually clean (different
  lifetimes); operationally a small footgun if a new contributor
  conflates them.
- Without an extraction, `start_meeting` no longer fills metadata at
  all. If the user hits Start with a description but no extraction,
  the _server falls back_ to the same on-start extraction as before
  (passing the description forward) — preserving the original UX as a
  default. If the user explicitly extracted first, the description
  is still sent on Start (so the server can re-extract and merge), but
  manual edits win via `merge_manual_wins`.

**Accepted risks:**

- The split contract is now more nuanced. A client that doesn't read
  the docs might send `metadata: {}` on Start, intending "no metadata"
  but actually meaning "explicit empty replace." Mitigated by sending
  the field as omitted (not present) when the meaning is "preserve."

## Alternatives considered

### (a, chosen) Separate `ExtractMetadata` intent + `start_meeting` preserves on omit

See above.

### (b) Auto-fire extraction on textarea blur

Same end result but no explicit button. Rejected: feels magical;
fires on every blur even if the user just stepped away briefly. The
explicit button keeps the user in control.

### (c) Inline extraction on every keystroke (debounced)

Real-time chip updates as the user types. Rejected: too expensive
(LLM calls per second), surprising (chips appear before the user is
done thinking), and fragile (incomplete sentences extract poorly).

### (d) Run the extraction on the PWA side and send chips back

Lift LLM extraction into the PWA. Rejected: server already owns the
LLM client and credentials; duplicating to the PWA would mean shipping
keys to a browser context.

## Follow-ups

- If the extraction-on-start fallback proves unused once the explicit
  flow has been the default for a while, simplify `start_meeting` by
  dropping `outcome.start_extraction_for`.
