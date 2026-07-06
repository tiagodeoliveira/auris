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
- [ADR-0003 — Persistence via bridge](0003-persistence-via-bridge.md) — `bridge.setLocalStorage` plus a `localStorage` fallback; Flutter WebView eats browser storage on its own.
- [ADR-0004 — WebSocket protocol](0004-websocket-protocol.md) — versioned, hand-maintained dual contract files + intent allowlist.
- [ADR-0005 — Multi-provider LLM](0005-multi-provider-llm.md) — `rig` abstracts Bedrock / OpenAI / Anthropic; provider chosen by env.
- [ADR-0006 — Live audio + STT pipeline](0006-live-audio-stt-pipeline.md) — ScreenCaptureKit + in-process mixer + Soniox via swappable adapter trait.
- [ADR-0007 — Summarizer architecture](0007-summarizer-architecture.md) — one task per mode, separate prompts and cadences; mode catalog server-defined. Superseded for highlights/actions/open_questions by [ADR-0011](0011-agentic-summarizer-loop.md).
- [ADR-0008 — mnemo memory integration](0008-mnemo-memory-integration.md) — streaming push per sentence + summary at stop + recall at start; per-mode prior-context toggle.
- [ADR-0009 — PWA UX design system](0009-pwa-ux-design-system.md) — industrial-blueprint tokens; store-driven, self-hiding components; mount-order-as-layout.
- [ADR-0010 — `ExtractMetadata` flow](0010-extract-metadata-flow.md) — separate intent so the user reviews chips before starting; `start_meeting` preserves metadata on omit.
- [ADR-0011 — Agentic summarizer loop](0011-agentic-summarizer-loop.md) — single stateful agent with tool-calling history replaces three per-mode summarizers; default model Opus 4.7 (1M ctx).

## Pending — decisions worth capturing as ADRs

Shipped recently without a formal ADR; the design intent is in
`PLAN.md` and the code, but each is a real "why this and not that"
choice that future readers will benefit from. Roughly in order of
how load-bearing they are:

- **Multi-tenant Auth0** — per-user state shard
  (`HashMap<UserId, UserState>`), JWT validation against Auth0
  JWKS, three Auth0 client roles (server-as-API-audience, PWA-SPA,
  Mac+mobile-Native), `AURIS_AUTH_DISABLED=1` bypass
  for dev/CI. Currently captured ad-hoc in
  [`docs/ARCHITECTURE.md`](../ARCHITECTURE.md) §9.
- **Postgres + boot recovery** — relational state for users +
  meetings + moments + artifacts; partial index on active meetings
  for cheap respawn after a crash; in-memory items per mode by
  design (re-derivable from the transcript JSONL on disk).
- **Mobile native port via Expo + EAS** — Expo SDK 51, expo-router
  file routing, Zustand mirror of PWA store, hand-synced wire
  types as the fourth contract file. Why Expo (not bare RN, not
  PWA-on-mobile), why EAS Build / Update over manual Xcode /
  Gradle, why share Mac's Native Auth0 client_id for personal use.
  Companion plan: [`docs/MOBILE-PLAN.md`](../MOBILE-PLAN.md).
- **Mac auto-update via Sparkle** — EdDSA-signed appcast on
  GitHub Releases, daily polling, Developer-ID signing path vs
  ad-hoc fallback. Why Sparkle (not Mac App Store, not custom).
- **CI/CD topology** — four workflows (server-image →
  GHCR, mac-bundle → GitHub Releases, EAS Build, EAS Update).
  Why a workflow per surface, why GHCR private vs public, why
  EAS Update channels mapped to build profiles. See
  [`.github/workflows/README.md`](../../.github/workflows/README.md).
- **Single-VM deploy** — defined in the kleos repo (Postgres, auris,
  and mnemo behind nginx on a Tailscale-routed VM), vs a managed
  Postgres on Fly.io / Render / Cloud Run. Why the simpler deploy now
  and what would push us toward managed services later.
- **Wire codegen, deferred** — currently four hand-synced contract
  files (Rust + Swift + 2× TS). Plan is `prost` + `swift-protobuf`
  - `ts-proto` migrating all four clients in lockstep. Tracked in
    PLAN.md §4.2; ADR pending the actual cutover.
