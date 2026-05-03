import { describe, expect, test, beforeEach } from "vitest";
import { createMockBridge, MockBridge } from "./__test__/mock-bridge";
import { loadSettings, makeStorage } from "./storage";

describe("storage", () => {
  let bridge: MockBridge;

  beforeEach(() => {
    bridge = createMockBridge();
  });

  test("loadSettings falls back to localhost server URL when bridge and env are empty", async () => {
    const s = await loadSettings(bridge, {});
    expect(s.serverUrl).toBe("ws://localhost:7331");
    expect(s.serverToken).toBe("");
    expect(s.sonioxKey).toBe("");
    expect(s.lastMetadata).toEqual({});
  });

  test("loadSettings reads stored values", async () => {
    bridge.storage["mc.serverUrl"] = "ws://laptop:7331";
    bridge.storage["mc.serverToken"] = "tok";
    bridge.storage["mc.sonioxKey"] = "sk_xxx";
    bridge.storage["mc.lastMetadata"] = JSON.stringify({ project: "helix" });

    const s = await loadSettings(bridge, {});
    expect(s.serverUrl).toBe("ws://laptop:7331");
    expect(s.serverToken).toBe("tok");
    expect(s.sonioxKey).toBe("sk_xxx");
    expect(s.lastMetadata).toEqual({ project: "helix" });
  });

  test("loadSettings seeds from env vars when bridge keys are empty", async () => {
    const s = await loadSettings(bridge, {
      VITE_DEFAULT_SERVER_URL: "ws://seed:7331",
      VITE_DEFAULT_SERVER_TOKEN: "seed-tok",
    });
    expect(s.serverUrl).toBe("ws://seed:7331");
    expect(s.serverToken).toBe("seed-tok");
    expect(s.sonioxKey).toBe(""); // no env var, no stored value
  });

  test("loadSettings does not overwrite stored values with env seeds", async () => {
    bridge.storage["mc.serverUrl"] = "ws://stored:7331";
    const s = await loadSettings(bridge, { VITE_DEFAULT_SERVER_URL: "ws://seed:7331" });
    expect(s.serverUrl).toBe("ws://stored:7331");
  });

  test("saveSetting writes to bridge storage", async () => {
    const storage = makeStorage(bridge);
    await storage.set("serverUrl", "ws://new:7331");
    expect(bridge.storage["mc.serverUrl"]).toBe("ws://new:7331");
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
