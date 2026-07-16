import { describe, expect, it, vi, afterEach } from "vitest";
import { MomentScreenshotApi } from "./moment-screenshot-api";

const token = async () => "tok";
afterEach(() => vi.restoreAllMocks());

describe("MomentScreenshotApi", () => {
  it("POSTs raw bytes to the moment screenshot endpoint with the mime + bearer", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as Response);
    vi.stubGlobal("fetch", fetchMock);
    const api = new MomentScreenshotApi("https://auris.test", token);
    await api.upload("m1", "mo1", new Blob([new Uint8Array([1, 2])]), "image/jpeg");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("https://auris.test/meetings/m1/moments/mo1/screenshot");
    expect(init.method).toBe("POST");
    expect(init.headers["Content-Type"]).toBe("image/jpeg");
    expect(init.headers.Authorization).toBe("Bearer tok");
  });

  it("throws with the status on a non-ok response", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: false, status: 400 } as Response));
    const api = new MomentScreenshotApi("https://auris.test", token);
    await expect(
      api.upload("m1", "mo1", new Blob([new Uint8Array([1])]), "image/jpeg"),
    ).rejects.toThrow(/400/);
  });
});
