import { afterEach, describe, expect, it, vi } from "vitest";
import { AurisClient, AuthError, HttpError, NotFoundError } from "./client.js";

const cfg = { baseUrl: "https://auris.test", token: "tok" };

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
    const out = await new AurisClient(cfg).listMeetings();
    expect(out).toEqual([{ id: "m1" }]);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("https://auris.test/meetings");
    expect((init.headers as Record<string, string>).Authorization).toBe("Bearer tok");
  });

  it("GETs /meetings/:id", async () => {
    const fetchMock = mockFetch(200, { id: "m1", transcript: [] });
    vi.stubGlobal("fetch", fetchMock);
    const out = await new AurisClient(cfg).getMeeting("m1");
    expect(out).toEqual({ id: "m1", transcript: [] });
    expect(fetchMock.mock.calls[0][0]).toBe("https://auris.test/meetings/m1");
  });

  it("maps 401 to AuthError", async () => {
    vi.stubGlobal("fetch", mockFetch(401, "unauthorized"));
    await expect(new AurisClient(cfg).listMeetings()).rejects.toBeInstanceOf(AuthError);
  });

  it("maps 404 to NotFoundError", async () => {
    vi.stubGlobal("fetch", mockFetch(404, "nope"));
    await expect(new AurisClient(cfg).getMeeting("x")).rejects.toBeInstanceOf(NotFoundError);
  });

  it("maps other non-2xx to HttpError with status", async () => {
    vi.stubGlobal("fetch", mockFetch(500, "boom"));
    await expect(new AurisClient(cfg).listMeetings()).rejects.toMatchObject({
      name: "HttpError",
      status: 500,
    });
  });

  it("wraps a network failure in HttpError(status 0)", async () => {
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new Error("ECONNREFUSED")));
    await expect(new AurisClient(cfg).listMeetings()).rejects.toMatchObject({
      name: "HttpError",
      status: 0,
    });
  });
});
