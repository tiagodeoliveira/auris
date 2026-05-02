import { describe, expect, test } from "vitest";
import { createMockBridge } from "../__test__/mock-bridge";
import { createStore } from "../store";
import { defaultAppState } from "../types";
import { createGlassesRenderer } from "./render";

describe("glasses renderer", () => {
  test("rebuilds Layout A on idle", async () => {
    const bridge = createMockBridge();
    const store = createStore(defaultAppState());
    const renderer = createGlassesRenderer(bridge as any, store);
    await renderer.applyView("idle");
    expect(bridge.rebuildPageContainer).toHaveBeenCalledOnce();
  });

  test("does not re-render when view does not change", async () => {
    const bridge = createMockBridge();
    const store = createStore(defaultAppState());
    const renderer = createGlassesRenderer(bridge as any, store);
    await renderer.applyView("idle");
    await renderer.applyView("idle");
    expect(bridge.rebuildPageContainer).toHaveBeenCalledOnce();
  });
});
