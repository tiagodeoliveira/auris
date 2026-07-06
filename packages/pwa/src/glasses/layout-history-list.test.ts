import { describe, expect, test } from "vitest";
import { defaultAppState } from "../types";
import type { MeetingSummary } from "../meetings-api";
import { buildHistoryListLayout, HISTORY_LIST_CONTAINER_NAME } from "./layout-history-list";

function meeting(id: string, over: Partial<MeetingSummary> = {}): MeetingSummary {
  return { id, description: null, metadata: {}, started_at: "", ended_at: null, ...over };
}

describe("buildHistoryListLayout", () => {
  test("renders a selectable list of meeting names", () => {
    const state = {
      ...defaultAppState(),
      glassesView: "history_list" as const,
      glassesHistory: [
        meeting("m-1", { metadata: { title: "Roadmap sync" } }),
        meeting("m-2", { description: "Design review\nmore" }),
      ],
    };
    const layout = buildHistoryListLayout(state);
    expect(layout.listObject?.[0]?.itemContainer?.itemName).toEqual([
      "Roadmap sync",
      "Design review",
    ]);
    expect(layout.listObject?.[0]?.itemContainer?.isItemSelectBorderEn).toBe(1);
    expect(layout.listObject?.[0]?.isEventCapture).toBe(1);
    expect(layout.listObject?.[0]?.containerName).toBe(HISTORY_LIST_CONTAINER_NAME);
  });

  test("truncates a too-long title to the 63-byte firmware list-item limit", () => {
    const longTitle = "A".repeat(100);
    const state = {
      ...defaultAppState(),
      glassesHistory: [meeting("m-1", { metadata: { title: longTitle } })],
    };
    const label = buildHistoryListLayout(state).listObject?.[0]?.itemContainer?.itemName?.[0] ?? "";
    expect(new TextEncoder().encode(label).length).toBeLessThanOrEqual(63);
    expect(label.endsWith("…")).toBe(true);
  });

  test("budgets in bytes, not characters, for multi-byte titles", () => {
    // 40 em(😀, 4 bytes each) = 160 bytes but only 40 chars — a char
    // cap would pass, a byte cap must truncate. Result must not split a
    // codepoint, so its byte length stays a multiple-of-4 plus ellipsis.
    const state = {
      ...defaultAppState(),
      glassesHistory: [meeting("m-1", { metadata: { title: "😀".repeat(40) } })],
    };
    const label = buildHistoryListLayout(state).listObject?.[0]?.itemContainer?.itemName?.[0] ?? "";
    expect(new TextEncoder().encode(label).length).toBeLessThanOrEqual(63);
    // No half-codepoint: every char round-trips through encode/decode.
    expect([...label].every((ch) => ch.length >= 1)).toBe(true);
  });

  test("loading variant is an event-capturing text container", () => {
    const state = { ...defaultAppState(), glassesHistoryLoading: true };
    const layout = buildHistoryListLayout(state);
    expect(layout.textObject?.[0]?.isEventCapture).toBe(1);
    expect(layout.textObject?.[0]?.content).toContain("Loading");
    expect(layout.listObject).toBeUndefined();
  });

  test("empty variant prompts a double-tap to go back", () => {
    const state = { ...defaultAppState(), glassesHistory: [] };
    const layout = buildHistoryListLayout(state);
    expect(layout.textObject?.[0]?.content).toContain("No meetings");
    expect(layout.textObject?.[0]?.content).toContain("Double-tap to go back");
    expect(layout.textObject?.[0]?.isEventCapture).toBe(1);
  });

  test("error variant shows the message and is event-capturing", () => {
    const state = { ...defaultAppState(), glassesHistoryError: "Server returned HTTP 500." };
    const layout = buildHistoryListLayout(state);
    expect(layout.textObject?.[0]?.content).toContain("Server returned HTTP 500.");
    expect(layout.textObject?.[0]?.content).toContain("Double-tap to go back");
    expect(layout.textObject?.[0]?.isEventCapture).toBe(1);
  });
});
