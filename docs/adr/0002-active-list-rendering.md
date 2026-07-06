# ADR-0002: Active-list rendering — TextContainer with formatted lines

**Status:** Accepted
**Date:** 2026-05-02
**Context for:** PWA spec (`docs/specs/pwa.md`), supersedes [`ARCHITECTURE.md` §5 Layout B](../ARCHITECTURE.md#layout-b--active--list) and [`ARCHITECTURE.md` §5 Update strategy](../ARCHITECTURE.md#update-strategy).

## Context

Layout B of the architecture (active meeting, list of items) is the most update-heavy view in the PWA. Items arrive every few seconds during a meeting (3s in the mock generator; production STT will be similar). The architecture describes Layout B as a `ListContainer` body updated via `textContainerUpgrade()`.

The Even Hub [Display & UI System guide](https://hub.evenrealities.com/docs/guides/display) explicitly states two relevant constraints:

1. `textContainerUpgrade` only operates on `TextContainer`, not `ListContainer`. The architecture's "ListContainer + textContainerUpgrade" combination is not a valid SDK call shape.
2. List containers cannot be updated in place at all. Any change to the items requires a full `rebuildPageContainer`, which the same guide flags as causing a brief flicker on hardware.

So we have to choose: rebuild on every update (use the native scroll, eat the flicker) or render the list as a TextContainer (lose the native scroll, get flicker-free updates).

## Decision

Render the active-list body as **a single `TextContainer`** holding all visible items as formatted multi-line text. The PWA tracks the highlight cursor locally and re-emits the formatted text via `textContainerUpgrade` whenever items change or the cursor moves.

Layout shape:

```
┌────────────────────────────────────────┐
│ ⌁ <mode label>            <display_tag>│  TextContainer #1 (header)
├────────────────────────────────────────┤  containerID: 1, no focus
│ ▶ <item text 1>                       │
│   <item text 2>                       │  TextContainer #2 (body)
│   <item text 3>                       │  containerID: 2, isEventCapture: 1
│   ...                                 │
└────────────────────────────────────────┘
```

The `▶` glyph (or another from the [design-guidelines selection set](https://hub.evenrealities.com/docs/guides/design-guidelines)) marks the highlighted line. Scroll events from the ring shift which item gets the cursor. When the cursor would move past the visible window, the PWA scrolls the displayed range so the highlighted item remains in view.

PWA-side state for the body:

- `items: Item[]` — full ordered list received from the server for the current mode.
- `highlightIndex: number` — index of the cursor in `items`.
- `viewportStart: number` — first item rendered in the visible window.
- `linesPerScreen: number` — derived from container height + font measurement (use [`everything-evenhub:font-measurement`](../../README.md) for the exact value).

On each `items_update` event or scroll gesture, the PWA recomputes the formatted body text and calls `textContainerUpgrade(containerID=2, containerName='body', newContent, ...)`.

## Consequences

**Positive:**

- Updates are flicker-free at the visual cadence the meeting summarizer produces them.
- No surprise SDK-call-shape error at Phase 1 hardware bring-up; this matches what `textContainerUpgrade` actually accepts.
- The PWA owns scroll position, so we can implement smarter scrolling later (auto-snap to newest, follow-mode toggle, etc.) without waiting on firmware features.

**Negative:**

- We lose the firmware-native scroll highlight. The PWA must implement scroll math, including handling overflow and edge cases (highlight at top/bottom of list, container width clipping long item text).
- The body's vertical position of items is determined by font metrics, not by the firmware's list layout engine. We have to size and pad consciously.
- Long item text wraps inside the TextContainer; the architecture's expectation of one item per line is enforced by the PWA truncating each item to fit one line of the available width.
- The `update_strategy: "append"` mode strategy still works — we apply the upsert-by-id rule into our local `items` array, then re-render — but we lose the visual signal that "this is a new item appearing at the bottom" that a native list might give.

**Accepted risks:**

- If the list grows past what fits on the 576x288 canvas, the PWA's scroll math has to handle off-screen items correctly. Tests in `tests/active_list.test.ts` need to cover viewport scrolling.
- Font measurement for the LVGL baked-in font isn't trivial; the [`everything-evenhub:font-measurement`](../../README.md) skill provides the only reliable approximation. Build-time validation of the assumed `linesPerScreen` against the skill's measurement is a Phase 0 task.

## Alternatives considered

### (a, chosen) TextContainer with formatted lines

See above.

### (b) ListContainer with full rebuild on every update

Each `items_update` triggers `rebuildPageContainer` with a fresh `ListContainer`. Native scroll, native highlight, but visible flicker on every tick.

Rejected because:

- A flicker every ~3 seconds during an active meeting is user-hostile, especially on glasses where the flicker is highly noticeable.
- The cadence will likely accelerate in Phase 2 (real STT can produce items at 1-2s intervals during dense conversation), making the flicker worse.

Worth revisiting if the firmware ever exposes incremental list updates, OR if we identify a UX pattern where rebuilds are rare (e.g. show only the most recent item, refresh once per minute).

### (c) Hybrid — ListContainer for static views, TextContainer for live updates

A two-mode active view: when items_update arrives slower than some threshold, use ListContainer for the nice native scroll; when it arrives faster, fall back to TextContainer.

Rejected because:

- The dual-mode renderer doubles the implementation surface for the most-used view.
- Mode-switching itself triggers a rebuild (full layout change), so the user gets exactly the flicker we're trying to avoid right at the moment things start happening.

## Follow-ups

- PWA spec: define the exact body-text format (cursor glyph, padding, item-truncation rule, viewport scroll behavior).
- PWA spec: define a Vitest test that runs the formatter against a known item list and asserts the output matches expected lines.
- Phase 1 hardware task: measure actual flicker behavior on G2 to confirm the trade-off was correctly characterized.
