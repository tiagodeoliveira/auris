import { describe, expect, test, vi } from "vitest";
import { createMockBridge } from "./__test__/mock-bridge";
import { createStore } from "./store";
import { boot } from "./boot";
import { defaultAppState } from "./types";

describe("boot", () => {
  test("loads settings and stores them", async () => {
    const bridge = createMockBridge();
    bridge.storage["mc.serverToken"] = "tok";
    const store = createStore(defaultAppState());
    await boot({ bridge, store, env: {}, isPaired: true });
    expect(store.get().settings.serverToken).toBe("tok");
  });

  test("creates startup page container", async () => {
    const bridge = createMockBridge();
    const store = createStore(defaultAppState());
    await boot({ bridge, store, env: {}, isPaired: true });
    expect(bridge.createStartUpPageContainer).toHaveBeenCalledOnce();
  });

  test("subscribes to device status changed", async () => {
    const bridge = createMockBridge();
    const store = createStore(defaultAppState());
    await boot({ bridge, store, env: {}, isPaired: true });
    // onEvenHubEvent is wired in main.ts (not boot); boot only registers device status.
    expect(bridge.onDeviceStatusChanged).toHaveBeenCalledOnce();
  });

  test("triggers ErrorOverlay if createStartUpPageContainer fails", async () => {
    const bridge = createMockBridge();
    bridge.createStartUpPageContainer = vi.fn(async () => 1); // 1 = invalid
    const store = createStore(defaultAppState());
    await boot({ bridge, store, env: {}, isPaired: true });
    expect(store.get().errorOverlay).not.toBeNull();
    expect(store.get().errorOverlay?.title).toMatch(/Failed to initialize/i);
  });

  test("isPaired=false picks the unpaired layout, isPaired=true picks idle", async () => {
    // Distinguish the two layouts by their container shape:
    //   - unpaired: 1 image (logo) + 1 text (prompt), and a follow-up
    //     `updateImageRawData` to paint the brand mark into the image
    //     container.
    //   - paired (idle): 2 text containers (header + body), no images.
    const unpairedBridge = createMockBridge();
    await boot({
      bridge: unpairedBridge,
      store: createStore(defaultAppState()),
      env: {},
      isPaired: false,
    });
    const unpairedArg = (
      unpairedBridge.createStartUpPageContainer.mock.calls as unknown[][]
    )[0][0] as {
      imageObject?: { containerName: string }[];
      textObject: { containerName: string }[];
    };
    expect(unpairedArg.imageObject?.map((i) => i.containerName)).toEqual(["logo"]);
    expect(unpairedArg.textObject.map((t) => t.containerName)).toEqual(["heading", "prompt"]);
    // We don't assert updateImageRawData here: jsdom's Canvas2D is a
    // stub, so the brand-mark draw helper throws, which boot.ts
    // (correctly) swallows with a warn. Real-glasses verification of
    // the image bytes happens manually.

    const pairedBridge = createMockBridge();
    await boot({
      bridge: pairedBridge,
      store: createStore(defaultAppState()),
      env: {},
      isPaired: true,
    });
    const pairedArg = (pairedBridge.createStartUpPageContainer.mock.calls as unknown[][])[0][0] as {
      imageObject?: { containerName: string }[];
      textObject?: { containerName: string }[];
      listObject?: { containerName: string }[];
    };
    // Entry layout: a single ListContainer with "Start meeting" /
    // "List meetings" items, full-screen. No text containers, no
    // image containers — the brand mark stays on the unpaired splash.
    expect(pairedArg.imageObject).toBeUndefined();
    expect(pairedArg.textObject).toBeUndefined();
    expect(pairedArg.listObject?.map((l) => l.containerName)).toEqual(["entry"]);
    expect(pairedBridge.updateImageRawData).not.toHaveBeenCalled();
  });

  test("boot does not gate on the legacy server token", async () => {
    // Pre-OAuth, boot would pop the settings modal when the
    // shared-secret token was missing. Auth0 owns first-run gating
    // now (login screen rendered by main.ts before mountUI runs),
    // so boot itself stays silent regardless of token presence.
    const bridge = createMockBridge();
    const store = createStore(defaultAppState());
    await boot({ bridge, store, env: {}, isPaired: true });
    expect(store.get().settingsModalOpen).toBe(false);
  });
});
