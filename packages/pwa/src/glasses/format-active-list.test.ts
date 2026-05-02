import { describe, expect, test } from "vitest";
import { formatActiveListBody } from "./format-active-list";
import type { Item } from "../types";

const item = (id: string, text: string): Item => ({ id, text, t: 0 });

describe("formatActiveListBody", () => {
  test("empty list returns empty string", () => {
    expect(formatActiveListBody([], 0, 0, 5, 60)).toBe("");
  });

  test("highlights the selected item", () => {
    const items = [item("a", "first"), item("b", "second")];
    const out = formatActiveListBody(items, 0, 0, 5, 60);
    expect(out.split("\n")[0].startsWith("▶ ")).toBe(true);
    expect(out.split("\n")[1].startsWith("  ")).toBe(true);
  });

  test("scrolls viewport when highlight is past visible window", () => {
    const items = Array.from({ length: 10 }, (_, i) => item(`i${i}`, `item ${i}`));
    const out = formatActiveListBody(items, 7, 4, 3, 60);
    const lines = out.split("\n");
    expect(lines).toHaveLength(3);
    expect(lines[0]).toContain("item 4");
    expect(lines[2]).toContain("item 6");
  });

  test("truncates items longer than charsPerLine - 2", () => {
    const long = "x".repeat(100);
    const items = [item("a", long)];
    const out = formatActiveListBody(items, 0, 0, 5, 20);
    expect(out.length).toBeLessThanOrEqual(20);
    expect(out.endsWith("…")).toBe(true);
  });

  test("renders fewer lines when items < linesPerScreen", () => {
    const items = [item("a", "one"), item("b", "two")];
    const out = formatActiveListBody(items, 0, 0, 5, 60);
    expect(out.split("\n")).toHaveLength(2);
  });
});
