import { describe, expect, test } from "vitest";

const SIM_BASE = process.env.AURIS_SIM_URL ?? "http://localhost:9898";

async function ping() {
  try {
    const r = await fetch(`${SIM_BASE}/api/ping`);
    return r.ok;
  } catch {
    return false;
  }
}

const SIM_AVAILABLE = await ping();

describe.skipIf(!SIM_AVAILABLE)("simulator integration", () => {
  test("simulator can be reached", async () => {
    expect(await ping()).toBe(true);
  });

  test("glasses screenshot has lit pixels after boot", async () => {
    // Wait for the PWA to render. In a fuller test we'd wait for an
    // app-ready console line via /api/console.
    await new Promise((r) => setTimeout(r, 2000));
    const png = await fetch(`${SIM_BASE}/api/screenshot/glasses`).then((r) => r.arrayBuffer());
    expect(png.byteLength).toBeGreaterThan(1000);
  });
});
