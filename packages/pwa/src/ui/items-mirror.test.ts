import { describe, expect, test, vi, beforeEach } from "vitest";
import { mountItemsMirror } from "./items-mirror";
import { createStore } from "../store";
import { defaultAppState, type Item } from "../types";

function makeStore(items: Item[], mode = "highlights") {
  return createStore({
    ...defaultAppState(),
    meetingState: "active",
    currentMode: mode,
    itemsByMode: { [mode]: items },
  });
}

function rowsOf(parent: HTMLElement): HTMLElement[] {
  return Array.from(parent.querySelectorAll<HTMLElement>(".item"));
}

describe("items-mirror DOM diffing", () => {
  let parent: HTMLElement;
  beforeEach(() => {
    parent = document.createElement("div");
    document.body.appendChild(parent);
  });

  test("initial mount creates one .item per store item", () => {
    const store = makeStore([
      { id: "a", text: "first", t: 0 },
      { id: "b", text: "second", t: 0 },
    ]);
    mountItemsMirror(parent, store, vi.fn());
    const rows = rowsOf(parent);
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain("first");
    expect(rows[1].textContent).toContain("second");
  });

  test("appending a new item keeps existing row node identity", () => {
    const store = makeStore([{ id: "a", text: "first", t: 0 }]);
    mountItemsMirror(parent, store, vi.fn());
    const initialFirstRow = rowsOf(parent)[0];

    // Append a new item — existing node MUST be the same instance
    // afterwards (this is the anti-flicker invariant; full innerHTML
    // rebuild fails it).
    store.update({
      itemsByMode: {
        highlights: [
          { id: "a", text: "first", t: 0 },
          { id: "b", text: "second", t: 0 },
        ],
      },
    });

    const after = rowsOf(parent);
    expect(after).toHaveLength(2);
    expect(after[0]).toBe(initialFirstRow);
  });

  test("removing an item leaves untouched siblings as the same nodes", () => {
    const store = makeStore([
      { id: "a", text: "first", t: 0 },
      { id: "b", text: "second", t: 0 },
      { id: "c", text: "third", t: 0 },
    ]);
    mountItemsMirror(parent, store, vi.fn());
    const [, secondRow, thirdRow] = rowsOf(parent);

    store.update({
      itemsByMode: {
        highlights: [
          { id: "b", text: "second", t: 0 },
          { id: "c", text: "third", t: 0 },
        ],
      },
    });

    const after = rowsOf(parent);
    expect(after).toHaveLength(2);
    expect(after[0]).toBe(secondRow);
    expect(after[1]).toBe(thirdRow);
  });

  test("updating an item's detail rerenders only that row", () => {
    const store = makeStore([
      { id: "a", text: "first", t: 0 },
      { id: "b", text: "second", t: 0 },
    ]);
    mountItemsMirror(parent, store, vi.fn());
    const [originalA, originalB] = rowsOf(parent);

    store.update({
      itemsByMode: {
        highlights: [
          { id: "a", text: "first", t: 0, detail: "expanded info" },
          { id: "b", text: "second", t: 0 },
        ],
      },
    });

    const after = rowsOf(parent);
    expect(after).toHaveLength(2);
    // Row A re-rendered (signature changed: detail appeared).
    expect(after[0]).not.toBe(originalA);
    // Row B untouched (no change in its visible state).
    expect(after[1]).toBe(originalB);
  });

  test("mode switch rebuilds all rows", () => {
    const store = makeStore(
      [
        { id: "a", text: "first", t: 0 },
        { id: "b", text: "second", t: 0 },
      ],
      "highlights",
    );
    store.update({
      itemsByMode: {
        highlights: [
          { id: "a", text: "first", t: 0 },
          { id: "b", text: "second", t: 0 },
        ],
        actions: [{ id: "x", text: "action one", t: 0 }],
      },
    });
    mountItemsMirror(parent, store, vi.fn());
    const beforeRows = rowsOf(parent);
    expect(beforeRows).toHaveLength(2);

    store.update({ currentMode: "actions" });

    const after = rowsOf(parent);
    expect(after).toHaveLength(1);
    expect(after[0].textContent).toContain("action one");
    // Different items → different node instances.
    expect(beforeRows.includes(after[0])).toBe(false);
  });

  test("empty state placeholder appears with no items, removed on first item", () => {
    const store = makeStore([]);
    mountItemsMirror(parent, store, vi.fn());
    expect(parent.querySelectorAll(".items-empty")).toHaveLength(1);
    expect(rowsOf(parent)).toHaveLength(0);

    store.update({
      itemsByMode: { highlights: [{ id: "a", text: "first", t: 0 }] },
    });

    expect(parent.querySelectorAll(".items-empty")).toHaveLength(0);
    expect(rowsOf(parent)).toHaveLength(1);
  });
});
