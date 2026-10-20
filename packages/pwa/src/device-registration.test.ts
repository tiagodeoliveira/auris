import { describe, expect, test, vi } from "vitest";
import { wireDeviceRegistration } from "./device-registration";
import { createStore } from "./store";
import { defaultAppState } from "./types";

const flush = () => new Promise((r) => setTimeout(r, 0));

function setup(over: Partial<ReturnType<typeof defaultAppState>> = {}) {
  const store = createStore({ ...defaultAppState(), ...over });
  const send = vi.fn();
  const logs: string[] = [];
  wireDeviceRegistration(store, {
    send,
    resolveIdentity: async () => ["dev-1", "SN123 (G2)"],
    log: (m) => logs.push(m),
  });
  return { store, send, logs };
}

describe("device registration", () => {
  test("registers when the bridge becomes ready after the WS opened", async () => {
    const { store, send, logs } = setup({ wsStatus: "open", glassesBridgeReady: false });
    await flush();
    expect(send).not.toHaveBeenCalled();
    expect(logs.join("\n")).toContain("glasses bridge not ready");
    store.update({ glassesBridgeReady: true });
    await flush();
    expect(send).toHaveBeenCalledTimes(1);
    expect(send.mock.calls[0][0]).toMatchObject({
      type: "register_device",
      hostname: "SN123 (G2)",
      capabilities: ["audio_capture"],
      device_id: "dev-1",
    });
  });

  test("registers on WS open when the bridge is already ready", async () => {
    const { store, send } = setup({ wsStatus: "connecting", glassesBridgeReady: true });
    store.update({ wsStatus: "open" });
    await flush();
    expect(send).toHaveBeenCalledTimes(1);
  });

  test("dedupes within one connection, re-registers after reconnect", async () => {
    const { store, send, logs } = setup({ wsStatus: "open", glassesBridgeReady: true });
    await flush();
    store.update({ glassesBridgeReady: false });
    store.update({ glassesBridgeReady: true });
    await flush();
    expect(send).toHaveBeenCalledTimes(1);
    expect(logs.join("\n")).toContain("already registered on this connection");
    store.update({ wsStatus: "closed" });
    store.update({ wsStatus: "open" });
    await flush();
    expect(send).toHaveBeenCalledTimes(2);
  });

  test("logs the skip reason when the WS is not open", async () => {
    const { send, logs } = setup({ wsStatus: "closed", glassesBridgeReady: true });
    await flush();
    expect(send).not.toHaveBeenCalled();
    expect(logs[0]).toMatch(/\[register\] skipped \(init\): ws not open/);
  });
});
