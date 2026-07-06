import { describe, expect, test } from "vitest";
import {
  activeListDocLines,
  activeListMaxOffset,
  formatActiveListBody,
  formatActiveListWindow,
} from "./format-active-list";
import type { Item } from "../types";

const item = (id: string, text: string): Item => ({ id, text, t: 0 });

describe("formatActiveListBody (tail-only)", () => {
  test("empty list returns a single space (firmware-clear sentinel)", () => {
    // `textContainerUpgrade("")` is a no-op on the firmware — the
    // prior content stays on screen. We send " " instead so an
    // empty mode actually blanks out.
    expect(formatActiveListBody([], 14)).toBe(" ");
  });

  test("pins the newest item to the bottom", () => {
    const items = Array.from({ length: 5 }, (_, i) => item(`i${i}`, `item ${i}`));
    const out = formatActiveListBody(items, 14);
    const lines = out.split("\n");
    expect(lines[lines.length - 1]).toBe("item 4");
  });

  test("budget caps how many items we send — newer beats older", () => {
    // Five 120-char items: ~3 wrapped lines each → at most ~4 fit
    // in a 14-line budget. The dropped item must be the OLDEST,
    // not the newest.
    const big = "x".repeat(120);
    const items = Array.from({ length: 5 }, (_, i) => item(`i${i}`, big));
    const out = formatActiveListBody(items, 14);
    const lines = out.split("\n");
    expect(lines.length).toBeLessThan(5);
    expect(lines.length).toBeGreaterThan(0);
  });

  test("always includes at least the newest item, even if oversize", () => {
    // A single 1000-char item is more than the 14-line budget can
    // hold, but we still render it (firmware clips the overflow)
    // rather than going blank — otherwise a long live-transcript
    // segment would show nothing.
    const huge = "y".repeat(1000);
    const items = [item("a", huge)];
    const out = formatActiveListBody(items, 14);
    expect(out).toBe(huge);
  });

  test("does NOT truncate item text — firmware wraps it", () => {
    const long = "x".repeat(120);
    const items = [item("a", long)];
    const out = formatActiveListBody(items, 14);
    expect(out).toBe(long);
    expect(out).not.toContain("…");
  });

  test("renders fewer lines when items fit comfortably", () => {
    const items = [item("a", "one"), item("b", "two")];
    const out = formatActiveListBody(items, 14);
    expect(out.split("\n")).toEqual(["one", "two"]);
  });

  test("interim line takes the last slot when present", () => {
    const items = Array.from({ length: 5 }, (_, i) => item(`i${i}`, `item ${i}`));
    const out = formatActiveListBody(items, 14, "live words");
    const lines = out.split("\n");
    // Interim sits at the bottom of the rendered content.
    expect(lines[lines.length - 1]).toBe("live words");
    // The most-recent committed item is the row just above it.
    expect(lines[lines.length - 2]).toBe("item 4");
  });
});

describe("formatActiveListWindow (scrollable: summary/highlights)", () => {
  // Short, single-line items so each wraps to exactly one display row —
  // the document line set is predictable (["item 0", … , "item 9"]).
  const tenItems = Array.from({ length: 10 }, (_, i) => item(`i${i}`, `item ${i}`));

  test("activeListDocLines is every item wrapped to display rows, oldest first", () => {
    expect(activeListDocLines(tenItems)).toEqual(tenItems.map((it) => it.text));
  });

  test("activeListMaxOffset is docLines.length - the visible-line budget", () => {
    expect(activeListMaxOffset(tenItems, 4)).toBe(6);
    // Everything fits → nothing to scroll.
    expect(activeListMaxOffset(tenItems, 20)).toBe(0);
  });

  test("offset 0 is the tail — the newest rows, newest at the bottom", () => {
    const out = formatActiveListWindow(tenItems, 4, 0);
    expect(out.split("\n")).toEqual(["item 6", "item 7", "item 8", "item 9"]);
  });

  test("a positive offset scrolls up toward older content, hiding the newest", () => {
    const out = formatActiveListWindow(tenItems, 4, 2);
    expect(out.split("\n")).toEqual(["item 4", "item 5", "item 6", "item 7"]);
  });

  test("at max offset the oldest rows are shown", () => {
    const out = formatActiveListWindow(tenItems, 4, activeListMaxOffset(tenItems, 4));
    expect(out.split("\n")).toEqual(["item 0", "item 1", "item 2", "item 3"]);
  });

  test("an out-of-range offset is clamped to the oldest window", () => {
    const out = formatActiveListWindow(tenItems, 4, 999);
    expect(out.split("\n")).toEqual(["item 0", "item 1", "item 2", "item 3"]);
  });

  test("when everything fits, the whole list shows regardless of offset", () => {
    expect(formatActiveListWindow(tenItems, 20, 0).split("\n")).toEqual(
      tenItems.map((it) => it.text),
    );
    expect(formatActiveListWindow(tenItems, 20, 5).split("\n")).toEqual(
      tenItems.map((it) => it.text),
    );
  });

  test("empty list returns the single-space firmware-clear sentinel", () => {
    expect(formatActiveListWindow([], 4, 0)).toBe(" ");
  });
});
