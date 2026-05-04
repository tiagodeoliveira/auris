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
});

// suppress unused import warning
void computeNextGlassesView;
