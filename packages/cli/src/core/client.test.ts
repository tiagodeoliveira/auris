import { afterEach, describe, expect, it, vi } from "vitest";
import { AurisClient, AuthError, HttpError, NotFoundError } from "./client.js";

const BASE = "https://auris.test";
const provider = async () => "tok";

function mockFetch(status: number, body: unknown) {
  return vi.fn().mockResolvedValue({
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
    text: async () => (typeof body === "string" ? body : JSON.stringify(body)),
  } as Response);
}

afterEach(() => vi.restoreAllMocks());

describe("AurisClient", () => {
  it("GETs /meetings with a bearer header and returns the list", async () => {
    const fetchMock = mockFetch(200, [{ id: "m1" }]);
    vi.stubGlobal("fetch", fetchMock);
    const out = await new AurisClient(BASE, provider).listMeetings();
    expect(out).toEqual([{ id: "m1" }]);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("https://auris.test/meetings");
    expect((init.headers as Record<string, string>).Authorization).toBe("Bearer tok");
  });

  it("throws AuthError when the token provider returns null", async () => {
    vi.stubGlobal("fetch", mockFetch(200, []));
    await expect(new AurisClient(BASE, async () => null).listMeetings()).rejects.toBeInstanceOf(
      AuthError,
    );
  });

  it("GETs /meetings/:id", async () => {
    const fetchMock = mockFetch(200, { id: "m1", transcript: [] });
    vi.stubGlobal("fetch", fetchMock);
    const out = await new AurisClient(BASE, provider).getMeeting("m1");
    expect(out).toEqual({ id: "m1", transcript: [] });
    expect(fetchMock.mock.calls[0][0]).toBe("https://auris.test/meetings/m1");
  });

  it("maps 401 to AuthError", async () => {
    vi.stubGlobal("fetch", mockFetch(401, "unauthorized"));
    await expect(new AurisClient(BASE, provider).listMeetings()).rejects.toBeInstanceOf(AuthError);
  });

  it("maps 404 to NotFoundError", async () => {
    vi.stubGlobal("fetch", mockFetch(404, "nope"));
    await expect(new AurisClient(BASE, provider).getMeeting("x")).rejects.toBeInstanceOf(
      NotFoundError,
    );
  });

  it("maps other non-2xx to HttpError with status", async () => {
    vi.stubGlobal("fetch", mockFetch(500, "boom"));
    await expect(new AurisClient(BASE, provider).listMeetings()).rejects.toMatchObject({
      name: "HttpError",
      status: 500,
    });
  });

  it("wraps a network failure in HttpError(status 0)", async () => {
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new Error("ECONNREFUSED")));
    await expect(new AurisClient(BASE, provider).listMeetings()).rejects.toMatchObject({
      name: "HttpError",
      status: 0,
    });
  });
});

function bytesResponse(status: number, body: Uint8Array, contentType = "image/png") {
  return vi.fn().mockResolvedValue({
    ok: status >= 200 && status < 300,
    status,
    headers: { get: (h: string) => (h.toLowerCase() === "content-type" ? contentType : null) },
    arrayBuffer: async () => body.buffer.slice(body.byteOffset, body.byteOffset + body.byteLength),
    text: async () => "",
  } as unknown as Response);
}

describe("AurisClient.getMomentScreenshot", () => {
  it("GETs the screenshot endpoint and returns bytes + mimeType", async () => {
    const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47]);
    const fetchMock = bytesResponse(200, png);
    vi.stubGlobal("fetch", fetchMock);
    const out = await new AurisClient(BASE, provider).getMomentScreenshot("m1", "mo1");
    expect(out.mimeType).toBe("image/png");
    expect(Array.from(out.bytes)).toEqual([0x89, 0x50, 0x4e, 0x47]);
    expect(fetchMock.mock.calls[0][0]).toBe(
      "https://auris.test/meetings/m1/moments/mo1/screenshot",
    );
    expect((fetchMock.mock.calls[0][1].headers as Record<string, string>).Authorization).toBe(
      "Bearer tok",
    );
  });
  it("maps 404 to NotFoundError", async () => {
    vi.stubGlobal("fetch", bytesResponse(404, new Uint8Array()));
    await expect(
      new AurisClient(BASE, provider).getMomentScreenshot("m1", "x"),
    ).rejects.toBeInstanceOf(NotFoundError);
  });
  it("throws AuthError when the token provider returns null", async () => {
    vi.stubGlobal("fetch", bytesResponse(200, new Uint8Array()));
    await expect(
      new AurisClient(BASE, async () => null).getMomentScreenshot("m1", "mo1"),
    ).rejects.toBeInstanceOf(AuthError);
  });
});
