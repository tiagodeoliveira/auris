import { describe, expect, test } from "vitest";
import { defaultAppState } from "./types";

describe("defaultAppState — history slice", () => {
  test("history fields default to empty/false/null", () => {
    const s = defaultAppState();
    expect(s.glassesHistory).toEqual([]);
    expect(s.glassesHistoryLoading).toBe(false);
    expect(s.glassesHistoryError).toBeNull();
    expect(s.glassesHistorySelectedId).toBeNull();
    expect(s.glassesHistorySummary).toBeNull();
    expect(s.glassesHistorySummaryLoading).toBe(false);
    expect(s.glassesHistorySummaryError).toBeNull();
  });
});
