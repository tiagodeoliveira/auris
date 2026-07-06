import { describe, expect, test, vi } from "vitest";
import { resolveDeviceLabel } from "./device-label";

describe("resolveDeviceLabel", () => {
  test("formats serial + uppercased model", async () => {
    const bridge = { getDeviceInfo: vi.fn(async () => ({ sn: "A1B2C3D4E5", model: "g2" })) };
    expect(await resolveDeviceLabel(bridge)).toBe("A1B2C3D4E5 (G2)");
  });

  test("defaults model to G2 when omitted", async () => {
    const bridge = { getDeviceInfo: vi.fn(async () => ({ sn: "737373" })) };
    expect(await resolveDeviceLabel(bridge)).toBe("737373 (G2)");
  });

  test("trims whitespace around the serial", async () => {
    const bridge = { getDeviceInfo: vi.fn(async () => ({ sn: "  SN9  ", model: "g2" })) };
    expect(await resolveDeviceLabel(bridge)).toBe("SN9 (G2)");
  });

  test("returns null when there is no serial", async () => {
    const bridge = { getDeviceInfo: vi.fn(async () => ({ sn: "", model: "g2" })) };
    expect(await resolveDeviceLabel(bridge)).toBeNull();
  });

  test("returns null when getDeviceInfo returns null (no glasses)", async () => {
    const bridge = { getDeviceInfo: vi.fn(async () => null) };
    expect(await resolveDeviceLabel(bridge)).toBeNull();
  });

  test("returns null when getDeviceInfo is absent (bare KV bridge)", async () => {
    expect(await resolveDeviceLabel({})).toBeNull();
  });

  test("returns null when getDeviceInfo throws (prototype mode)", async () => {
    const bridge = {
      getDeviceInfo: vi.fn(async () => {
        throw new Error("no bridge");
      }),
    };
    expect(await resolveDeviceLabel(bridge)).toBeNull();
  });
});
