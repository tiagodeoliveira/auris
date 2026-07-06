import { describe, expect, test } from "vitest";
import { formatHistorySummaryBody } from "./meeting-format";
import type { Item } from "./contract";

function item(text: string): Item {
  return { id: text, text, t: 0 };
}

describe("formatHistorySummaryBody", () => {
  test("joins summary-mode items as bullets", () => {
    const body = formatHistorySummaryBody({
      items_by_mode: { summary: [item("Discussed Q3 roadmap"), item("Agreed on launch date")] },
      description: "ignored when summary exists",
    });
    expect(body).toBe("• Discussed Q3 roadmap\n• Agreed on launch date");
  });

  test("falls back to description when summary mode is empty", () => {
    const body = formatHistorySummaryBody({
      items_by_mode: { summary: [] },
      description: "  Kickoff with the design team  ",
    });
    expect(body).toBe("Kickoff with the design team");
  });

  test("falls back to description when summary mode is absent", () => {
    const body = formatHistorySummaryBody({ description: "Just a description" });
    expect(body).toBe("Just a description");
  });

  test("uses placeholder when neither summary nor description is present", () => {
    expect(formatHistorySummaryBody({})).toBe("(no summary yet)");
    expect(formatHistorySummaryBody({ description: "   " })).toBe("(no summary yet)");
  });

  test("skips blank summary items", () => {
    const body = formatHistorySummaryBody({
      items_by_mode: { summary: [item("Real point"), item("   ")] },
    });
    expect(body).toBe("• Real point");
  });

  test("renders a narrative summary item as prose, without a bullet", () => {
    // The final-summary worker stores one item tagged
    // meta.kind === "narrative" — it's flowing prose, not a bullet, so
    // it must render verbatim (paragraph breaks preserved, no "• ").
    const narrative = "The team aligned on the roadmap.\n\nBilling was deferred to next quarter.";
    const body = formatHistorySummaryBody({
      items_by_mode: {
        summary: [{ id: "s", text: narrative, t: 0, meta: { kind: "narrative" } }],
      },
      description: "ignored when a summary exists",
    });
    expect(body).toBe(narrative);
  });

  test("returns the full body, uncapped (the glasses layer paginates)", () => {
    const long = "x".repeat(5000);
    const body = formatHistorySummaryBody({ description: long });
    expect(body).toBe(long);
  });
});
