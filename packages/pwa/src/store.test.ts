import { describe, expect, test, vi } from "vitest";
import { createStore } from "./store";
import type { AppState } from "./types";

function defaultState(): AppState {
  return {
    settings: { serverUrl: "", serverToken: "", sonioxKey: "", lastMetadata: {} },
    wsStatus: "closed",
    wsLastEventAt: null,
    protocolVersionMatched: false,
    meetingState: "idle",
    meetingStartedAt: null,
    availableModes: [],
    currentMode: "highlights",
    displayTag: null,
    metadata: {},
    itemsByMode: {},
    composeDescription: "",
    extractingMetadata: false,
    priorContext: null,
    availableDevices: [],
    audioSourceDeviceId: null,
    liveTranscriptInterim: "",
    status: { listening: false, paused: false },
    glassesView: "idle",
    highlightIndex: 0,
    viewportStart: 0,
    detailItemId: null,
    listeningTranscript: "",
    listeningInterim: "",
    listeningStartedAt: null,
    appForegrounded: true,
    bleConnected: false,
    batteryLevel: null,
    wearing: false,
    settingsModalOpen: false,
    toasts: [],
    errorOverlay: null,
  };
}

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
