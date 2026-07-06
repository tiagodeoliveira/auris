import { describe, expect, test } from "vitest";
import { nextAssistToShow } from "./assist-queue";
import type { Item } from "../types";

function it(id: string, text = id): Item {
  return { id, text, t: 0, meta: { type: "coach" } };
}

describe("nextAssistToShow", () => {
  test("returns null when there are no items", () => {
    expect(nextAssistToShow([], [])).toBeNull();
  });

  test("returns null when every item has been shown", () => {
    expect(nextAssistToShow([it("a"), it("b")], ["a", "b"])).toBeNull();
  });

  test("returns the first item when nothing has been shown", () => {
    expect(nextAssistToShow([it("a"), it("b")], [])?.id).toBe("a");
  });

  test("skips already-shown items and returns the next", () => {
    // FIFO: the assist list is append-only on the server, so the
    // queue order matches arrival order.
    expect(nextAssistToShow([it("a"), it("b"), it("c")], ["a"])?.id).toBe("b");
    expect(nextAssistToShow([it("a"), it("b"), it("c")], ["a", "b"])?.id).toBe("c");
  });

  test("is stable when new items arrive while older ones are queued", () => {
    // Scenario: while popup of `a` was up, server appended `b`.
    // After dismissing `a`, ledger is [a]; queue should yield `b`.
    expect(nextAssistToShow([it("a"), it("b")], ["a"])?.id).toBe("b");
    // And appending `c` while `b` is showing yields `c` next.
    expect(nextAssistToShow([it("a"), it("b"), it("c")], ["a", "b"])?.id).toBe("c");
  });

  test("does not re-show items even if they are upserted mid-stream", () => {
    // If the server were to amend an item's text (not done today,
    // but cheap insurance), the id-based ledger still prevents a
    // re-popup — the user already decided what to do with it.
    const amended = { ...it("a"), text: "updated text" };
    expect(nextAssistToShow([amended], ["a"])).toBeNull();
  });
});
