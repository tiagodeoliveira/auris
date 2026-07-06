# ADR-0004: WebSocket protocol — versioning, dual contracts, intent allowlist

**Status:** Accepted
**Date:** 2026-05-04
**Context for:** server ↔ PWA wire format; documented in [`PROTOCOL.md`](../PROTOCOL.md).

## Context

The PWA and the Rust server are independently deployable, written in
different languages, and evolve at different rates. Their only contact
surface is a WebSocket on `:7331`. Any wire-format drift becomes a silent
runtime error — the PWA may send an intent the server doesn't understand,
or the server may emit events the PWA can't decode.

Three pressures push on the design:

1. **No shared schema source-of-truth.** A code-generated contract (e.g.
   protobuf) would be the obvious answer, but it adds a build step, a code
   generator, and a hosting decision (where do `.proto` files live?). For a
   solo project iterating fast, the friction outweighed the safety.
2. **Errors that name what went wrong.** When a payload is rejected, the
   client wants to know _what kind_ of error: malformed JSON, unknown
   intent type, or a known type with bad fields. Generic "deserialize
   error" forces a debugger.
3. **Rust's `serde(tag = "type")` is strict.** Unknown variants fail
   deserialization with a generic message. We want a softer, named-error
   path before the strict deserializer.

## Decision

- **Two hand-maintained contract files** kept in sync by code review:
  `packages/server/src/contract.rs` and `packages/pwa/src/contract.ts`.
  Both encode the same `Intent` and `Event` shapes; both are tagged with
  `type: "snake_case"` discriminators.
- **`protocol_version: u32` constant** in both files, currently `1`. Sent
  on every `Snapshot` event; the PWA refuses to operate when it doesn't
  match its compiled-in expectation, surfacing a non-dismissable error
  overlay.
- **Two-stage intent dispatch on the server:**
  1. Parse incoming text as `serde_json::Value`. If it isn't a JSON
     object → `bad_json` error.
  2. Read the `type` field. If it isn't in a hand-maintained allowlist
     in `ws.rs` → `unknown_intent` error (carrying the offending type).
  3. Only then deserialize as `Intent`. Any field-shape problem →
     `bad_payload` error.
- **Versioning rule for breakers:** if a change to either the intent or
  event shape is not backwards-compatible (renamed field, removed variant,
  changed semantics), bump `PROTOCOL_VERSION`. Adding optional fields to
  existing variants does not require a bump.

## Consequences

**Positive:**

- Adding an intent is a small, well-rehearsed change: extend the Rust
  enum, extend the TypeScript union, add the type string to the
  allowlist. Three places, all close to each other in their own files.
- `unknown_intent` errors carry the offending type string, so the PWA
  surfaces a precise toast and the server logs a useful warning.
- Protocol version mismatch produces a hard error overlay, not a silent
  failure. The user is told _exactly_ which side needs updating.
- The wire format is human-readable, debuggable with `wscat`, and free of
  build-time codegen.

**Negative:**

- The intent allowlist is a third source of truth that must be kept in
  sync with the enum. Forgetting to add a new intent here was the
  cause of one user-visible bug (`extract_metadata`); the test suite
  doesn't catch this because it operates on the enum directly.
- Two contract files mean every wire change is a coordinated edit. There
  is no compiler enforcement that the TypeScript and Rust shapes match.
- If a future client (e.g. a CLI for testing) is built, it has to ship
  its own copy of the contract.

**Accepted risks:**

- A stale TypeScript contract relative to a newer server is a silent
  bug. We mitigate via the protocol-version check (so at least the
  _shape_ boundary is hard) and via end-to-end smoke tests that exercise
  every event type. We do not mitigate via codegen.

## Alternatives considered

### (a, chosen) Hand-maintained dual contracts + intent allowlist

See above.

### (b) Protobuf or JSON Schema with codegen

Single source of truth, mechanical sync. Rejected: adds a build dependency
(protoc, protoc-gen-ts, etc.), a generated-files commit policy, and an
extra layer to debug when the wire format itself is the suspect. For a
solo project iterating week to week, the manual edit is faster overall.
Worth revisiting if a third client appears.

### (c) Single-stage `serde(tag)` dispatch with no allowlist

Rust would just fail with "unknown variant" on an unrecognized intent.
Rejected: the error message is generic (no `intent_ref`), and it becomes
hard to distinguish "client sent the wrong type" from "client sent the
right type but malformed payload" — both surface as deserialize errors.
The two-stage dispatch costs one extra `serde_json::Value` parse per
message; trivial.

### (d) `#[serde(other)]` catch-all variant

Tempting — captures unknown intents as `Intent::Unknown { type: String }`.
Rejected: it makes the enum non-exhaustive in a way that disables Rust's
match-completeness check across the codebase. The cost compounds.

## Follow-ups

- Consider deriving the intent allowlist from the enum at compile time
  (e.g. via `strum::IntoStaticStr`) so adding a variant without updating
  the allowlist becomes a compile error. Small, mechanical fix; not
  urgent.
