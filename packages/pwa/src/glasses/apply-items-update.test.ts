import { describe, expect, test } from "vitest";
import { applyItemsUpdate } from "./apply-items-update";
import type { Item, ModeOption } from "../types";

const item = (id: string, text: string, detail?: string): Item => ({ id, text, t: 0, detail });
const mode = (id: string, strategy: "replace" | "append"): ModeOption => ({
  id,
  label: id,
  update_strategy: strategy,
});

describe("applyItemsUpdate", () => {
  test("replace strategy replaces entire list", () => {
    const current = [item("a", "old")];
    const incoming = [item("b", "new1"), item("c", "new2")];
    const next = applyItemsUpdate(current, incoming, mode("highlights", "replace"));
    expect(next.map((i) => i.id)).toEqual(["b", "c"]);
  });

  test("append strategy upserts by id; new items at end", () => {
    const current = [item("a", "first"), item("b", "second")];
    const incoming = [item("c", "third")];
    const next = applyItemsUpdate(current, incoming, mode("transcript", "append"));
    expect(next.map((i) => i.id)).toEqual(["a", "b", "c"]);
  });

  test("append strategy replaces existing item by id in place", () => {
    const current = [item("a", "first"), item("b", "second")];
    const incoming = [item("a", "first updated", "detail!")];
    const next = applyItemsUpdate(current, incoming, mode("transcript", "append"));
    expect(next[0].text).toBe("first updated");
    expect(next[0].detail).toBe("detail!");
    expect(next.map((i) => i.id)).toEqual(["a", "b"]);
  });
});
