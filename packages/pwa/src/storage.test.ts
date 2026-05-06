import { describe, expect, test, beforeEach } from "vitest";
import { createMockBridge, MockBridge } from "./__test__/mock-bridge";
import { loadSettings, makeStorage } from "./storage";

describe("storage", () => {
  let bridge: MockBridge;

  beforeEach(() => {
    bridge = createMockBridge();
  });

  test("loadSettings returns empty defaults when bridge and env are empty", async () => {
    const s = await loadSettings(bridge, {});
    expect(s.serverToken).toBe("");
    expect(s.lastMetadata).toEqual({});
  });

  test("loadSettings reads stored values", async () => {
    bridge.storage["mc.serverToken"] = "tok";
    bridge.storage["mc.lastMetadata"] = JSON.stringify({ project: "helix" });

    const s = await loadSettings(bridge, {});
    expect(s.serverToken).toBe("tok");
    expect(s.lastMetadata).toEqual({ project: "helix" });
  });

  test("loadSettings seeds from env vars when bridge keys are empty", async () => {
    const s = await loadSettings(bridge, {
      VITE_DEFAULT_SERVER_TOKEN: "seed-tok",
    });
    expect(s.serverToken).toBe("seed-tok");
  });

  test("loadSettings does not overwrite stored values with env seeds", async () => {
    bridge.storage["mc.serverToken"] = "stored-tok";
    const s = await loadSettings(bridge, { VITE_DEFAULT_SERVER_TOKEN: "seed-tok" });
    expect(s.serverToken).toBe("stored-tok");
  });

  test("saveSetting writes to bridge storage", async () => {
    const storage = makeStorage(bridge);
    await storage.set("serverToken", "new-tok");
    expect(bridge.storage["mc.serverToken"]).toBe("new-tok");
  });

  test("saveSetting JSON-encodes lastMetadata", async () => {
    const storage = makeStorage(bridge);
    await storage.set("lastMetadata", { project: "x" });
    expect(bridge.storage["mc.lastMetadata"]).toBe(JSON.stringify({ project: "x" }));
  });

  test("loadSettings tolerates malformed JSON in lastMetadata", async () => {
    bridge.storage["mc.lastMetadata"] = "not valid json";
    const s = await loadSettings(bridge, {});
    expect(s.lastMetadata).toEqual({});
  });
});
