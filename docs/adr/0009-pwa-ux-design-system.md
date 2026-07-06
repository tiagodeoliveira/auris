# ADR-0009: PWA UX — industrial-blueprint design system, store-driven UI

**Status:** Accepted
**Date:** 2026-05-04
**Context for:** PWA presentation layer; full reference in
[`UX.md`](../UX.md).

## Context

The PWA's first iteration was a functional but visually rough scaffold
built during Phase 0. By Phase 2, the surface had grown — listening
flow, mode tabs, items mirror, active-meeting header, settings modal,
toasts, error overlays — and the styling debt was load-bearing on the
overall feel of the product.

Two things needed fixing:

1. **A coherent visual language.** The author's portfolio site at
   `tiago.sh` is an industrial-blueprint aesthetic (deep slate base,
   rust-orange accents, monospace technical labels alongside display
   typography). The PWA should feel like part of the same world.
2. **An interaction model that fits "control surface for a real-time
   system."** Components must self-hide based on meeting state without
   the parent component knowing about every child. Settings must persist
   reliably. Live state changes (transcript appending, metadata
   extracting, prior context loading) must be visible without being
   loud.

Over Phase 2 the redesign also absorbed several behavior changes that
were initially separate items: live in-flight transcript row, chip-style
metadata editor, extract-tags-before-start flow, memory-context badge.

## Decision

- **Industrial-blueprint design tokens** in `style.css`:
  - Palette: `--bg-primary: #1a1e23`, `--text-light: #e4e9ec`,
    `--rust-warm: #d4602c`, `--spark-orange: #e87a3d`.
  - Typography: Bebas Neue for display, Space Grotesk for body,
    JetBrains Mono for technical labels.
  - Border / radius / spacing tokens for consistent geometry.
- **Store-driven UI.** A single typed `Store<AppState>` is the source
  of truth. Each UI component subscribes to a slice via a string
  selector and re-renders on change. The DOM is a projection, not a
  cache. This means: dictation-in-progress, metadata edits,
  extract-tags clicks, mnemo recall arrival — all flow through the
  store, all UI reacts uniformly.
- **Self-hiding components.** Each top-level component (compose-region,
  compose-start, header-strip, kv-editor, mode-tabs, items-mirror,
  cta-region) subscribes to `meetingState` and toggles its own
  `display: none` outside its valid states. The parent index.ts knows
  only mount order, not visibility logic.
- **Mount order as layout.** Components are appended to `#app` in
  reading order; visibility self-decides. Two visible side-effects:
  - The `kv-editor` (metadata) is mounted _between_ compose-region
    (idle-only) and compose-start (idle-only), so the kv-editor renders
    in a stable visual slot whether the meeting is idle or active.
  - This avoids layout jumps at state transitions: kv-editor doesn't
    move when a meeting starts.
- **Inline-edit chips for metadata.** No modal, no separate form. Each
  metadata key is a small pill; the value input is the chip body. Edit,
  press Enter or blur to commit. Add new entries via a `+ ADD` chip
  that turns into an inline editor.
- **Two-step description flow.** The user types (or dictates) a meeting
  description; clicking _Extract Tags_ runs LLM extraction without
  starting the meeting. Chips appear, can be edited, then _Start
  Meeting_ uses them as a base. The server preserves metadata when
  `start_meeting` omits the field, so editing-after-extracting is
  load-bearing.
- **Two persistence layers.** Settings (server URL, tokens) write to
  both `bridge.setLocalStorage` (for the EvenHub Flutter WebView
  context) and browser `localStorage` (fallback for dev / simulator).
  On read, prefer bridge; fall back to browser.
- **Memory badge in the active-meeting header strip.** When the server
  emits `prior_context_changed` with non-empty counts, a small rust pill
  reads `★ memory · N recalled`; tooltip gives the per-dimension
  breakdown. Hidden otherwise.

## Consequences

**Positive:**

- Adding a new screen state (e.g., a "post-meeting recap" view) is
  mostly: write a self-hiding component, mount it in the right place.
  The parent never grows.
- Layout doesn't jump on state transitions — a recurring complaint
  with the previous flexbox-stretching approach.
- The store-driven model means tests at the boundary (store →
  component) are straightforward; we don't need to integration-test
  every interaction path.
- The visual language is consistent with the author's broader work,
  which matters for a portfolio piece.

**Negative:**

- Subscribing to a string-selector slice (e.g.,
  `${s.composeDescription}|${s.extractingMetadata}`) is a string-glue
  abstraction; if a value contains the separator, equality checks lie.
  None of our slices do today.
- Self-hiding components mean the DOM always contains every screen,
  just with `display: none`. That's fine for our scale (≤ 10 top-level
  components) but wouldn't scale to a richer app.
- The `bridge.setLocalStorage` + `localStorage` dual write means a
  conflict is possible if the user runs both in real glasses and a
  browser at once with the same settings keys. We accept this — the
  user runs one at a time.
- The mount order _is_ the layout. A future contributor needs to know
  this; it's documented but not enforced.

## Alternatives considered

### (a, chosen) Store-driven, self-hiding components, mount order = layout

See above.

### (b) Component framework (Vue, Svelte, React)

A framework gives reactivity and templating out of the box. Rejected:
the PWA runs inside a Flutter WebView with constrained CPU and bundle
budgets; vanilla TS + a tiny store keeps the runtime to ~37 KB gzip.
The framework's "self-hiding" semantics would also be different
(conditional rendering vs. CSS display) — not strictly better.

### (c) CSS-grid–based layout with explicit state classes

`#app` carries a class like `state-active` / `state-listening`, child
components position themselves via grid-area. Rejected: more rigid;
adding a state means touching every child's grid placement. Mount-order
flow is more change-resilient.

### (d) Modal dialogs for metadata editing

Click metadata → modal opens with full form. Rejected: modal feels
heavyweight for "edit one value"; chip inline-edit is faster and stays
in the user's visual context. Settings modal is justified differently
(it's truly a separate concern, not part of the meeting flow).

## Follow-ups

- The dual-persistence layer for settings could be cleaned up if /
  when the EvenHub bridge proves reliable across app restarts on real
  hardware.
- The mount-order-as-layout convention deserves a one-line comment in
  `ui/index.ts` so a future reader doesn't reorder mounts thinking
  it's cosmetic.
