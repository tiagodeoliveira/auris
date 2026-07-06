import { describe, expect, test } from "vitest";
import { handleServerEvent } from "./ws-handlers";
import { createStore } from "./store";
import { defaultAppState, activeItems } from "./types";
import type { ServerEvent } from "./types";

describe("ws handlers", () => {
  test("snapshot with mismatched protocol_version sets ErrorOverlay", () => {
    const store = createStore(defaultAppState());
    const event: ServerEvent = {
      type: "snapshot",
      protocol_version: 99,
      meeting_state: "idle",
      available_modes: [],
      mode: "highlights",
      metadata: {},
      items: [],
      status: { listening: false },
      devices: [],
    };
    handleServerEvent(event, store);
    expect(store.get().errorOverlay?.title).toMatch(/Incompatible/);
    expect(store.get().protocolVersionMatched).toBe(false);
  });

  test("snapshot with matching protocol_version replaces state", () => {
    const store = createStore(defaultAppState());
    const event: ServerEvent = {
      type: "snapshot",
      protocol_version: 1,
      meeting_state: "active",
      available_modes: [{ id: "highlights", label: "Highlights", update_strategy: "replace" }],
      mode: "highlights",
      metadata: { project: "helix" },
      items: [{ id: "a", text: "first", t: 0 }],
      status: { listening: true },
      devices: [],
    };
    handleServerEvent(event, store);
    expect(store.get().protocolVersionMatched).toBe(true);
    expect(store.get().meetingState).toBe("active");
    // Snapshot ingests items into `itemsByMode["highlights"]` but
    // intentionally does NOT update `currentMode` — view selection
    // is per-surface UI state, not server-driven. Each surface
    // keeps the default `transcript` until the local picker moves.
    expect(store.get().itemsByMode["highlights"]).toHaveLength(1);
    expect(store.get().currentMode).toBe("transcript");
    expect(store.get().glassesView).toBe("active_list");
  });

  test("meeting_state_changed updates state and glassesView", () => {
    const store = createStore({
      ...defaultAppState(),
      meetingState: "idle",
      glassesView: "idle",
      protocolVersionMatched: true,
    });
    handleServerEvent({ type: "meeting_state_changed", meeting_state: "active" }, store);
    expect(store.get().meetingState).toBe("active");
    expect(store.get().glassesView).toBe("active_list");
  });

  test("items_update applies append upsert", () => {
    const store = createStore({
      ...defaultAppState(),
      protocolVersionMatched: true,
      availableModes: [{ id: "transcript", label: "Transcript", update_strategy: "append" }],
      currentMode: "transcript",
      itemsByMode: { transcript: [{ id: "a", text: "first", t: 0 }] },
    });
    handleServerEvent(
      { type: "items_update", mode: "transcript", items: [{ id: "b", text: "second", t: 100 }] },
      store,
    );
    expect(activeItems(store.get()).map((i) => i.id)).toEqual(["a", "b"]);
  });

  test("meeting end clears a primed glasses stop confirmation", () => {
    // If the wearer armed the glasses '> Stop' prompt but the meeting
    // ended by another path (phone Stop, server timeout), the armed
    // flag must reset so the next meeting doesn't open primed to end.
    const store = createStore({
      ...defaultAppState(),
      protocolVersionMatched: true,
      meetingState: "active",
      glassesStopArmed: true,
    });
    handleServerEvent({ type: "meeting_state_changed", meeting_state: "idle" }, store);
    expect(store.get().glassesStopArmed).toBe(false);
  });

  test("error event surfaces as toast", () => {
    const store = createStore(defaultAppState());
    handleServerEvent(
      { type: "error", code: "unknown_mode", message: "bogus", intent_ref: "bogus" },
      store,
    );
    expect(store.get().toasts).toHaveLength(1);
    expect(store.get().toasts[0].text).toContain("unknown_mode");
  });
});
