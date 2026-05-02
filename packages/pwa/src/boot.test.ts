import { describe, expect, test, vi } from "vitest";
import { createMockBridge } from "./__test__/mock-bridge";
import { createStore } from "./store";
import { boot } from "./boot";
import { defaultAppState } from "./types";

describe("boot", () => {
  test("loads settings and stores them", async () => {
    const bridge = createMockBridge();
    bridge.storage["mc.serverUrl"] = "ws://test:7331";
    bridge.storage["mc.serverToken"] = "tok";
    const store = createStore(defaultAppState());
    await boot({ bridge, store, env: {} });
    expect(store.get().settings.serverUrl).toBe("ws://test:7331");
    expect(store.get().settings.serverToken).toBe("tok");
  });

  test("creates startup page container", async () => {
    const bridge = createMockBridge();
    const store = createStore(defaultAppState());
    await boot({ bridge, store, env: {} });
    expect(bridge.createStartUpPageContainer).toHaveBeenCalledOnce();
  });

  test("subscribes to device status changed", async () => {
    const bridge = createMockBridge();
    const store = createStore(defaultAppState());
    await boot({ bridge, store, env: {} });
    // onEvenHubEvent is wired in main.ts (not boot); boot only registers device status.
    expect(bridge.onDeviceStatusChanged).toHaveBeenCalledOnce();
  });

  test("triggers ErrorOverlay if createStartUpPageContainer fails", async () => {
    const bridge = createMockBridge();
    bridge.createStartUpPageContainer = vi.fn(async () => 1); // 1 = invalid
    const store = createStore(defaultAppState());
    await boot({ bridge, store, env: {} });
    expect(store.get().errorOverlay).not.toBeNull();
    expect(store.get().errorOverlay?.title).toMatch(/Failed to initialize/i);
  });

  test("opens settings modal if serverUrl is empty", async () => {
    const bridge = createMockBridge();
    const store = createStore(defaultAppState());
    await boot({ bridge, store, env: {} });
    expect(store.get().settingsModalOpen).toBe(true);
  });

  test("does not open settings modal if serverUrl is in storage", async () => {
    const bridge = createMockBridge();
    bridge.storage["mc.serverUrl"] = "ws://laptop:7331";
    bridge.storage["mc.serverToken"] = "tok";
    const store = createStore(defaultAppState());
    await boot({ bridge, store, env: {} });
    expect(store.get().settingsModalOpen).toBe(false);
  });
});
