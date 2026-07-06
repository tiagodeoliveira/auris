import { afterEach, describe, expect, test, vi } from "vitest";
import { createMockBridge } from "../__test__/mock-bridge";
import { createStore } from "../store";
import { defaultAppState } from "../types";
import { ACTIVITY_NAME, MOMENT_FLASH_MS, MOMENT_NAME } from "./layout-active-list";
import { createGlassesRenderer } from "./render";

describe("glasses renderer", () => {
  test("rebuilds Layout A on idle", async () => {
    const bridge = createMockBridge();
    const store = createStore(defaultAppState());
    const renderer = createGlassesRenderer(bridge as any, store);
    await renderer.applyView("idle");
    expect(bridge.rebuildPageContainer).toHaveBeenCalledOnce();
  });

  test("does not re-render when view does not change", async () => {
    const bridge = createMockBridge();
    const store = createStore(defaultAppState());
    const renderer = createGlassesRenderer(bridge as any, store);
    await renderer.applyView("idle");
    await renderer.applyView("idle");
    expect(bridge.rebuildPageContainer).toHaveBeenCalledOnce();
  });
});

describe("glasses renderer — recording indicator on quick asks", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  test("animates the activity indicator while in quick_asks and listening", async () => {
    vi.useFakeTimers();
    const bridge = createMockBridge();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "idle",
      meetingState: "active",
      glassesCurrentMode: "quick_asks",
      availableModes: [{ id: "quick_asks", label: "Quick Asks", update_strategy: "replace" }],
      status: { listening: true },
    });
    const renderer = createGlassesRenderer(bridge as any, store);
    await renderer.applyView("active_list");
    // startActivityAnim pushes the first frame synchronously (before the
    // interval), targeting the shared activity container by name.
    expect(bridge.textContainerUpgrade).toHaveBeenCalledWith(
      expect.objectContaining({ containerName: ACTIVITY_NAME }),
    );
  });

  test("does not animate the indicator on quick_asks when not listening", async () => {
    vi.useFakeTimers();
    const bridge = createMockBridge();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "idle",
      meetingState: "active",
      glassesCurrentMode: "quick_asks",
      availableModes: [{ id: "quick_asks", label: "Quick Asks", update_strategy: "replace" }],
      status: { listening: false },
    });
    const renderer = createGlassesRenderer(bridge as any, store);
    await renderer.applyView("active_list");
    expect(bridge.textContainerUpgrade).not.toHaveBeenCalledWith(
      expect.objectContaining({ containerName: ACTIVITY_NAME }),
    );
  });

  test("switches the indicator to a warning when the audio WS stalls mid-meeting", async () => {
    vi.useFakeTimers();
    const bridge = createMockBridge();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "idle",
      meetingState: "active",
      glassesCurrentMode: "quick_asks",
      availableModes: [{ id: "quick_asks", label: "Quick Asks", update_strategy: "replace" }],
      status: { listening: true },
      audioCaptureState: { kind: "streaming", since: 0 },
    });
    const renderer = createGlassesRenderer(bridge as any, store);
    await renderer.applyView("active_list");
    // The /audio WS drops — the wearer must see this on the only
    // status surface Quick Asks has.
    store.update({ audioCaptureState: { kind: "reconnecting", attempt: 1, since: 0 } });
    expect(bridge.textContainerUpgrade).toHaveBeenCalledWith(
      expect.objectContaining({ containerName: ACTIVITY_NAME, content: "!!" }),
    );
  });
});

describe("glasses renderer — moment-captured flash", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  function activeStore(mode: string) {
    return createStore({
      ...defaultAppState(),
      glassesView: "idle",
      meetingState: "active",
      glassesCurrentMode: mode,
      availableModes: [{ id: mode, label: mode, update_strategy: "replace" }],
      status: { listening: true },
    });
  }

  test("flashes '+1' on a moment-marked edge, then clears after the timer", async () => {
    vi.useFakeTimers();
    const bridge = createMockBridge();
    const store = activeStore("transcript");
    const renderer = createGlassesRenderer(bridge as any, store);
    await renderer.applyView("active_list");

    store.update({ momentMarkedSeq: 1 });
    expect(bridge.textContainerUpgrade).toHaveBeenCalledWith(
      expect.objectContaining({ containerName: MOMENT_NAME, content: "+1" }),
    );

    // Auto-clears back to a blank once the flash window elapses.
    vi.advanceTimersByTime(MOMENT_FLASH_MS);
    expect(bridge.textContainerUpgrade).toHaveBeenCalledWith(
      expect.objectContaining({ containerName: MOMENT_NAME, content: " " }),
    );
  });

  test("does not flash while off the active-list surface", async () => {
    vi.useFakeTimers();
    const bridge = createMockBridge();
    const store = activeStore("transcript");
    createGlassesRenderer(bridge as any, store);
    // Never entered active_list — a phone-CTA mark must not push to a
    // container that isn't on screen.
    store.update({ momentMarkedSeq: 1 });
    expect(bridge.textContainerUpgrade).not.toHaveBeenCalledWith(
      expect.objectContaining({ containerName: MOMENT_NAME }),
    );
  });

  test("does not flash on the quick_asks surface (no moment container there)", async () => {
    vi.useFakeTimers();
    const bridge = createMockBridge();
    const store = activeStore("quick_asks");
    const renderer = createGlassesRenderer(bridge as any, store);
    await renderer.applyView("active_list");

    store.update({ momentMarkedSeq: 1 });
    expect(bridge.textContainerUpgrade).not.toHaveBeenCalledWith(
      expect.objectContaining({ containerName: MOMENT_NAME }),
    );
  });
});

describe("glasses renderer — history surface", () => {
  test("rebuilds the history list on entry", async () => {
    const bridge = createMockBridge();
    const store = createStore(defaultAppState());
    const renderer = createGlassesRenderer(bridge as any, store);
    await renderer.applyView("history_list");
    expect(bridge.rebuildPageContainer).toHaveBeenCalledOnce();
  });

  test("rebuilds the history list when the fetched data arrives", async () => {
    const bridge = createMockBridge();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "history_list",
      glassesHistoryLoading: true,
    });
    const renderer = createGlassesRenderer(bridge as any, store);
    await renderer.applyView("history_list");
    const callsAfterEnter = bridge.rebuildPageContainer.mock.calls.length;
    // Simulate the reactor resolving the fetch.
    store.update({
      glassesHistoryLoading: false,
      glassesHistory: [
        { id: "m-1", description: "First", metadata: {}, started_at: "", ended_at: null },
      ],
    });
    expect(bridge.rebuildPageContainer.mock.calls.length).toBeGreaterThan(callsAfterEnter);
  });

  test("rebuilds the summary popup when the detail arrives", async () => {
    const bridge = createMockBridge();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "history_summary",
      glassesHistorySelectedId: "m-1",
      glassesHistorySummaryLoading: true,
    });
    const renderer = createGlassesRenderer(bridge as any, store);
    await renderer.applyView("history_summary");
    const before = bridge.rebuildPageContainer.mock.calls.length;
    store.update({
      glassesHistorySummaryLoading: false,
      glassesHistorySummary: { title: "First", body: "• point" },
    });
    expect(bridge.rebuildPageContainer.mock.calls.length).toBeGreaterThan(before);
  });

  test("repaints the summary popup when the body window scrolls", async () => {
    // Scrolling changes only glassesHistorySummaryLineOffset — the view
    // stays history_summary, so applyView's dedupe won't fire. The summary
    // subscription's selector must include the offset or the new window is
    // computed in state but never drawn.
    const longBody = Array.from({ length: 30 }, (_, i) => `• Bullet point number ${i}`).join("\n");
    const bridge = createMockBridge();
    const store = createStore({
      ...defaultAppState(),
      glassesView: "history_summary",
      glassesHistorySummary: { title: "Long", body: longBody },
      glassesHistorySummaryLineOffset: 0,
    });
    const renderer = createGlassesRenderer(bridge as any, store);
    await renderer.applyView("history_summary");
    const before = bridge.rebuildPageContainer.mock.calls.length;
    store.update({ glassesHistorySummaryLineOffset: 3 });
    expect(bridge.rebuildPageContainer.mock.calls.length).toBeGreaterThan(before);
  });
});
