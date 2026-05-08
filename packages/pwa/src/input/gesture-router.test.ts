import { describe, expect, test, vi } from "vitest";
import { OsEventTypeList } from "@evenrealities/even_hub_sdk";
import { handleBridgeEvent } from "./gesture-router";
import { createStore } from "../store";
import { defaultAppState } from "../types";
import { computeNextGlassesView } from "../state-machine";

describe("gesture-router", () => {
  test("ring CLICK while active_list dispatches view -> active_detail", () => {
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      itemsByMode: { transcript: [{ id: "a", text: "x", t: 0 }] },
    });
    handleBridgeEvent({ textEvent: { eventType: OsEventTypeList.CLICK_EVENT } }, store, vi.fn());
    expect(store.get().glassesView).toBe("active_detail");
    expect(store.get().detailItemId).toBe("a");
  });

  test("ring SCROLL_TOP moves highlight up", () => {
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      itemsByMode: {
        transcript: [
          { id: "a", text: "x", t: 0 },
          { id: "b", text: "y", t: 0 },
        ],
      },
      highlightIndex: 1,
    });
    handleBridgeEvent(
      { textEvent: { eventType: OsEventTypeList.SCROLL_TOP_EVENT } },
      store,
      vi.fn(),
    );
    expect(store.get().highlightIndex).toBe(0);
  });

  test("ring SCROLL_BOTTOM moves highlight down", () => {
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      itemsByMode: {
        transcript: [
          { id: "a", text: "x", t: 0 },
          { id: "b", text: "y", t: 0 },
        ],
      },
      highlightIndex: 0,
    });
    handleBridgeEvent(
      { textEvent: { eventType: OsEventTypeList.SCROLL_BOTTOM_EVENT } },
      store,
      vi.fn(),
    );
    expect(store.get().highlightIndex).toBe(1);
  });

  test("ring DOUBLE_CLICK while active dispatches mark_moment intent", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
    });
    handleBridgeEvent(
      { textEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT } },
      store,
      send,
    );
    expect(send).toHaveBeenCalledWith(expect.objectContaining({ type: "mark_moment" }));
  });

  test("CLICK normalized from undefined eventType", () => {
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      itemsByMode: { transcript: [{ id: "a", text: "x", t: 0 }] },
    });
    handleBridgeEvent({ textEvent: { eventType: undefined } }, store, vi.fn());
    expect(store.get().glassesView).toBe("active_detail");
  });

  // The simulator (and likely real glasses for primary tap) emits the
  // click as a sysEvent with eventSource=1 (TOUCH_EVENT_FROM_GLASSES_R)
  // and no eventType (proto3 default-omits zero == CLICK_EVENT).
  test("sysEvent click (no eventType, eventSource=1) -> active_detail", () => {
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      itemsByMode: { transcript: [{ id: "a", text: "x", t: 0 }] },
    });
    handleBridgeEvent({ sysEvent: { eventSource: 1 } }, store, vi.fn());
    expect(store.get().glassesView).toBe("active_detail");
    expect(store.get().detailItemId).toBe("a");
  });

  test("sysEvent double-click (eventType=3, eventSource=1) -> mark_moment", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      meetingState: "active",
    });
    handleBridgeEvent(
      { sysEvent: { eventType: OsEventTypeList.DOUBLE_CLICK_EVENT, eventSource: 1 } },
      store,
      send,
    );
    expect(send).toHaveBeenCalledWith(expect.objectContaining({ type: "mark_moment" }));
  });

  test("sysEvent FOREGROUND_EXIT does NOT trigger a click in gesture-router", () => {
    const send = vi.fn();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "active_list",
      itemsByMode: { transcript: [{ id: "a", text: "x", t: 0 }] },
    });
    handleBridgeEvent(
      { sysEvent: { eventType: OsEventTypeList.FOREGROUND_EXIT_EVENT } },
      store,
      send,
    );
    // Stays in active_list — lifecycle.ts handles foreground events
    // separately; gesture-router must not also treat them as clicks.
    expect(store.get().glassesView).toBe("active_list");
    expect(send).not.toHaveBeenCalled();
  });
});

// suppress unused import warning
void computeNextGlassesView;
