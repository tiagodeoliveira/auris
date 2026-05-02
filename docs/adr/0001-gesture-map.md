# ADR-0001: Gesture map — phone-only lifecycle for Phase 0

**Status:** Accepted
**Date:** 2026-05-02
**Context for:** PWA spec (`docs/specs/pwa.md`), supersedes parts of [`ARCHITECTURE.md` §4 Gesture map](../ARCHITECTURE.md#gesture-map) and §1 Input-surface convention.

## Context

The architecture doc binds every meeting-lifecycle gesture (start, stop, cancel listening, mode cycle) to **long-press** on the G2 left temple or the R1 ring. Verifying this against the [Even Hub Input & Events guide](https://hub.evenrealities.com/docs/guides/input-events) and the `@evenrealities/even_hub_sdk` TypeScript definitions reveals that no long-press event is exposed by the SDK. The full enumerated input surface is:

| Source             | Available events                                                                               |
| ------------------ | ---------------------------------------------------------------------------------------------- |
| G2 temple touchpad | `CLICK_EVENT (0)`, `DOUBLE_CLICK_EVENT (3)`, `SCROLL_TOP_EVENT (1)`, `SCROLL_BOTTOM_EVENT (2)` |
| R1 ring touchpad   | Same set, distinguishable by source (field name undocumented)                                  |
| Phone screen       | Standard DOM input — anything we want                                                          |
| Simulator          | Same four events as the glasses (no long-press)                                                |

The simulator's HTTP automation API also confirms this: only Up / Down / Click / Double Click can be injected. Designing lifecycle gestures around long-press would mean we couldn't validate them in the simulator at all.

## Decision

For Phase 0:

- **Lifecycle gestures live entirely on the phone screen.** Start, Pause, Stop, Cancel-listening, Confirm-stop are buttons in the PWA UI.
- **Glasses-side gestures are reserved for in-flow controls only**: Press = expand item, Double Press = mark moment, Swipe Up/Down = scroll highlight, mode cycle = swipe (left/right rebinding to be confirmed once source distinction is empirically resolved).
- **G2 vs R1 source distinction is deferred** to a Phase 1 hardware task — log raw events, determine the field name, then decide whether to remap any of the in-flow controls per source (e.g. ring tap vs temple tap).
- **Promotion of `DOUBLE_CLICK_EVENT` to a lifecycle role is left open.** A double-press to confirm-stop would be defensible UX once we have hardware to test deliberate-vs-accidental activation. Re-evaluate after Phase 1.

This decision applies through the end of Phase 0 (simulator + first hardware sideload). Revisit before Phase 2 if the phone-only path proves user-hostile.

## Consequences

**Positive:**

- The Phase 0 gesture map is fully exercisable in the simulator. No "we need real hardware to know if this works" footnotes.
- The phone is already specified as a "parallel deliberate path" in [`ARCHITECTURE.md` §2 Design principles](../ARCHITECTURE.md#design-principles); making it the only lifecycle path is a smaller departure than it sounds.
- Stop and other destructive actions are deliberate by construction — no accidental long-press to worry about.
- One fewer SDK-quirk to discover at Phase 1 hardware bring-up.

**Negative:**

- The user must reach for the phone to start a meeting. The design intent of "long-press your temple to start a meeting" was hands-free; we lose that ergonomic.
- The original §4 description of the listening flow ("left temple long-press in idle → enter listening view") doesn't apply. The PWA spec must define a phone-screen flow that achieves the same end.
- Mode cycling via gesture is reduced to swipe (or deferred entirely to the phone dropdown).

**Accepted risks:**

- We may discover at Phase 1 that double-press on a temple feels safe enough for lifecycle and want to add it back. That's a future ADR superseding this one.
- The PWA spec gets slightly more UI work (Cancel/Confirm/Stop buttons) than the architecture doc implied.

## Alternatives considered

### (b, chosen) Phone-only lifecycle

See above.

### (a) Promote `DOUBLE_CLICK_EVENT` to lifecycle

Temple double-press = enter listening (idle) / confirm-stop (active). Ring single-tap = expand item. Ring double-tap = mark moment.

Rejected for now because:

- Double-press on a temple touchpad has a non-trivial accidental-trigger rate that we can only characterize on hardware. Stop is a destructive action; getting it wrong loses meeting state.
- We don't yet know how the SDK distinguishes G2 temple double-press from R1 ring double-press; assigning different meanings to "double-press" depending on source needs the field discovery to land first.

Re-evaluate after Phase 1 hardware testing. If accidental triggers are <1% in real use, this becomes the preferred Phase 2 design.

### (c) Combination gesture (e.g. double-press + swipe)

Temple double-press followed by a swipe within 1s confirms a destructive action. Adds discoverability cost ("how do I stop the meeting?") and implementation complexity (state machine inside the gesture handler).

Rejected because the same destructive-confirm UX is achievable on the phone with a single visible button — no learning curve, no discoverability cost.

## Follow-ups

- Phase 1 hardware task: log raw `event.textEvent` / `event.listEvent` payloads on temple and ring presses, identify the source-distinction field, document the exact field name and value set in [`docs/specs/pwa.md`](../specs/pwa.md).
- Phase 1 usability check: time how long it takes to start / stop a meeting with phone-only flow vs hypothetical double-press lifecycle on hardware; revisit this ADR with data.
