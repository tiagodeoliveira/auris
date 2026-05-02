# Architecture Decision Records

Each ADR captures a decision that is non-obvious from the code, has
real alternatives, and would otherwise need to be re-derived from
primary sources every time someone asked "why did we do it this way?"

## Format

Lightweight Michael Nygard style:

```
# ADR-NNNN: Short title

**Status:** Proposed | Accepted | Superseded by ADR-XXXX
**Date:** YYYY-MM-DD
**Context for:** which spec / component / system this constrains

## Context

What's true about the world that forces a decision.

## Decision

What we're committing to.

## Consequences

What changes. What gets harder. What we're explicitly accepting.

## Alternatives considered

Each option, why it was rejected, what would tip us toward it later.
```

## Index

- [ADR-0001 — Gesture map](0001-gesture-map.md) — phone-only lifecycle gestures for Phase 0; SDK has no long-press.
- [ADR-0002 — Active-list rendering](0002-active-list-rendering.md) — TextContainer with formatted lines; ListContainer can't be updated in place.
- [ADR-0003 — Persistence via bridge](0003-persistence-via-bridge.md) — `bridge.setLocalStorage`, not browser `localStorage`; Flutter WebView eats browser storage.
