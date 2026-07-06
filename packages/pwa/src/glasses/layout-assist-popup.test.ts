import { describe, expect, test } from "vitest";
import { assistTypeGlyph, buildAssistPopupLayout } from "./layout-assist-popup";
import type { Item } from "../types";

function itemOf(
  meta: Record<string, unknown> | undefined,
  text = "headline",
  detail?: string,
): Item {
  return { id: "as-1", text, detail, t: 0, meta };
}

describe("layout-assist-popup", () => {
  test("type label covers the four canonical types (ASCII — firmware has no emoji glyphs)", () => {
    expect(assistTypeGlyph(itemOf({ type: "definition" }))).toBe("DEFINITION");
    expect(assistTypeGlyph(itemOf({ type: "question" }))).toBe("QUESTION");
    expect(assistTypeGlyph(itemOf({ type: "memory" }))).toBe("MEMORY");
    expect(assistTypeGlyph(itemOf({ type: "coach" }))).toBe("COACH");
  });

  test("type label defaults to coach when the type tag is missing or unknown", () => {
    // Defensive default so we still render something recognisable
    // if the server schema drifts before we update the client.
    expect(assistTypeGlyph(itemOf(undefined))).toBe("COACH");
    expect(assistTypeGlyph(itemOf({ type: "something-new" }))).toBe("COACH");
  });

  test("layout includes glyph, headline, and the dismiss footer", () => {
    const layout = buildAssistPopupLayout(
      itemOf({ type: "definition" }, "PageRank", "Algorithm by Brin and Page"),
    );
    expect(layout.containerTotalNum).toBe(1);
    expect(layout.textObject).toHaveLength(1);
    const content = layout.textObject?.[0]?.content ?? "";
    expect(content).toContain("DEFINITION");
    expect(content).toContain("PageRank");
    expect(content).toContain("Algorithm by Brin and Page");
    expect(content).toContain("Tap to dismiss");
  });

  test("layout has exactly one event-capture container", () => {
    // Firmware constraint: exactly one container per page must
    // have isEventCapture=1. The popup IS that one container.
    const layout = buildAssistPopupLayout(itemOf({ type: "coach" }, "tip text"));
    expect(layout.textObject?.[0]?.isEventCapture).toBe(1);
  });

  test("body without detail still renders headline + dismiss footer", () => {
    const layout = buildAssistPopupLayout(itemOf({ type: "question" }, "Why is the sky blue?"));
    const content = layout.textObject?.[0]?.content ?? "";
    expect(content).toContain("Why is the sky blue?");
    expect(content).toContain("Tap to dismiss");
  });

  test("very long body truncates with ellipsis before the dismiss footer", () => {
    // Long detail strings would overflow the firmware text container.
    // Slice at ~600 chars so the dismiss footer always lands inside
    // the visible region.
    const longDetail = "x".repeat(2000);
    const layout = buildAssistPopupLayout(itemOf({ type: "memory" }, "h", longDetail));
    const content = layout.textObject?.[0]?.content ?? "";
    expect(content.length).toBeLessThan(700);
    expect(content).toContain("…");
    expect(content.endsWith("Tap to dismiss")).toBe(true);
  });
});
