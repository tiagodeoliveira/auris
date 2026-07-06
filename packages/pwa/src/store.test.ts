import { describe, expect, test, vi } from "vitest";
import { createStore } from "./store";
import { defaultAppState } from "./types";

// Use the exported default-state helper so this test stays
// trivially in sync with new fields on AppState.
const defaultState = defaultAppState;

describe("store", () => {
  test("get returns initial state", () => {
    const store = createStore(defaultState());
    expect(store.get().wsStatus).toBe("closed");
  });

  test("update merges patch", () => {
    const store = createStore(defaultState());
    store.update({ wsStatus: "open" });
    expect(store.get().wsStatus).toBe("open");
    expect(store.get().meetingState).toBe("idle");
  });

  test("subscribe fires on change of selected slice", () => {
    const store = createStore(defaultState());
    const cb = vi.fn();
    const unsubscribe = store.subscribe((s) => s.wsStatus, cb);
    store.update({ wsStatus: "open" });
    expect(cb).toHaveBeenCalledWith("open", "closed");
    unsubscribe();
  });

  test("subscribe does NOT fire when selected slice is unchanged", () => {
    const store = createStore(defaultState());
    const cb = vi.fn();
    store.subscribe((s) => s.wsStatus, cb);
    store.update({ meetingState: "active" }); // different slice
    expect(cb).not.toHaveBeenCalled();
  });

  test("unsubscribe stops further calls", () => {
    const store = createStore(defaultState());
    const cb = vi.fn();
    const unsubscribe = store.subscribe((s) => s.wsStatus, cb);
    unsubscribe();
    store.update({ wsStatus: "open" });
    expect(cb).not.toHaveBeenCalled();
  });

  test("re-entrant update from within subscriber is queued", () => {
    const store = createStore(defaultState());
    const log: string[] = [];

    store.subscribe(
      (s) => s.wsStatus,
      (next) => {
        log.push(`outer:${next}`);
        if (next === "open") {
          store.update({ wsStatus: "reconnecting" });
        }
      },
    );

    store.update({ wsStatus: "open" });

    expect(log).toEqual(["outer:open", "outer:reconnecting"]);
  });
});
