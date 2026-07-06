import { describe, expect, it } from "vitest";
import { getTextWidth } from "@evenrealities/pretext";
import { wrapToLines } from "./wrap";

// A width generous enough that a short sentence fits on one line.
const WIDE = 452;

describe("wrapToLines", () => {
  it("treats \\n as a hard break", () => {
    expect(wrapToLines("alpha\nbeta", WIDE)).toEqual(["alpha", "beta"]);
  });

  it("renders an empty paragraph as one blank line", () => {
    expect(wrapToLines("alpha\n\nbeta", WIDE)).toEqual(["alpha", "", "beta"]);
  });

  it("returns a single blank line for empty input", () => {
    expect(wrapToLines("", WIDE)).toEqual([""]);
  });

  it("keeps a line that fits on a single row intact", () => {
    expect(wrapToLines("short line", WIDE)).toEqual(["short line"]);
  });

  it("wraps a long paragraph onto multiple rows, none wider than the budget", () => {
    const text =
      "the quick brown fox jumps over the lazy dog and then keeps on running across the meadow";
    const lines = wrapToLines(text, WIDE);
    expect(lines.length).toBeGreaterThan(1);
    for (const line of lines) {
      expect(getTextWidth(line)).toBeLessThanOrEqual(WIDE);
    }
  });

  it("never drops or reorders words when wrapping", () => {
    const text = "one two three four five six seven eight nine ten eleven twelve";
    const lines = wrapToLines(text, 120);
    expect(lines.join(" ").split(/\s+/)).toEqual(text.split(" "));
  });

  it("hard-breaks a single word wider than the budget by character", () => {
    const word = "supercalifragilisticexpialidocious";
    const lines = wrapToLines(word, 80);
    expect(lines.length).toBeGreaterThan(1);
    for (const line of lines) {
      expect(getTextWidth(line)).toBeLessThanOrEqual(80);
    }
    expect(lines.join("")).toBe(word);
  });
});
