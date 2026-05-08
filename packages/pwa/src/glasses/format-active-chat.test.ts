import { describe, expect, test } from "vitest";
import { formatActiveChatBody, wrapText } from "./format-active-chat";
import type { Item } from "../types";

const item = (id: string, text: string, role: string): Item => ({
  id,
  text,
  t: 0,
  meta: { role },
});

describe("formatActiveChatBody", () => {
  test("empty thread returns empty string", () => {
    expect(formatActiveChatBody([], 60, 5)).toBe("");
  });

  test("user turn gets cursor prefix, assistant turn gets indent", () => {
    const items = [item("q1", "What is the weather?", "user"), item("a1", "Sunny.", "assistant")];
    const out = formatActiveChatBody(items, 60, 5);
    const lines = out.split("\n");
    expect(lines[0]).toBe("▶ What is the weather?");
    // Blank separator between turns.
    expect(lines[1]).toBe("");
    expect(lines[2]).toBe("  Sunny.");
  });

  test("wraps long assistant answers across multiple lines", () => {
    const long =
      "Per the bio, Hopper helped develop UNIVAC I, pioneered COBOL, and wrote the first compiler.";
    const items = [item("q1", "Tell me about Hopper.", "user"), item("a1", long, "assistant")];
    const out = formatActiveChatBody(items, 30, 10);
    const lines = out.split("\n");
    // User question on line 0, blank on line 1, assistant answer
    // wraps across 2-3 lines.
    expect(lines[0]).toBe("▶ Tell me about Hopper.");
    expect(lines.length).toBeGreaterThan(3);
    // Subsequent assistant lines are indented (no cursor).
    for (let i = 2; i < lines.length; i++) {
      expect(lines[i].startsWith("  ")).toBe(true);
    }
  });

  test("bottom-pins to the most recent exchange", () => {
    const items = [
      item("q1", "first question", "user"),
      item("a1", "first answer", "assistant"),
      item("q2", "second question", "user"),
      item("a2", "second answer", "assistant"),
    ];
    const out = formatActiveChatBody(items, 60, 3);
    const lines = out.split("\n");
    expect(lines).toHaveLength(3);
    // The last 3 lines should be the most recent content. With
    // blank separators, the slice catches the second exchange's
    // tail (user turn + blank + assistant turn or part thereof).
    expect(lines[lines.length - 1]).toContain("second answer");
  });

  test("strips markdown bold markers", () => {
    const items = [item("a1", "**UNIVAC I** was an early computer.", "assistant")];
    const out = formatActiveChatBody(items, 60, 5);
    expect(out).not.toContain("**");
    expect(out).toContain("UNIVAC I was an early computer.");
  });

  test("default role (no meta) is treated as assistant", () => {
    const noMeta: Item = { id: "x", text: "hello", t: 0 };
    const out = formatActiveChatBody([noMeta], 60, 5);
    // Indented (no cursor).
    expect(out.startsWith("  ")).toBe(true);
  });
});

describe("wrapText", () => {
  test("preserves explicit newlines as paragraph breaks", () => {
    const out = wrapText("first line\nsecond line", 60);
    expect(out).toEqual(["first line", "second line"]);
  });

  test("hard-splits words longer than width", () => {
    const out = wrapText("xxxxxxxxxxxxxxxxxxxxxxxxxxxx", 10);
    // 28 chars / 10 = 2 full lines + 8-char remainder.
    expect(out).toEqual(["xxxxxxxxxx", "xxxxxxxxxx", "xxxxxxxx"]);
  });

  test("greedy fill across word boundaries", () => {
    const out = wrapText("the quick brown fox jumps over the lazy dog", 15);
    // Each line at most 15 chars.
    for (const line of out) {
      expect(line.length).toBeLessThanOrEqual(15);
    }
    expect(out.join(" ")).toBe("the quick brown fox jumps over the lazy dog");
  });
});
