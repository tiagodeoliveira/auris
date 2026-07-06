import { afterEach, describe, expect, test, vi } from "vitest";
import { MeetingsApi, MeetingsApiError } from "./meetings-api";

function api() {
  return new MeetingsApi("https://api.example.test", async () => "tok-123");
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("MeetingsApi.rename", () => {
  test("PATCHes /meetings/:id with the title and bearer auth", async () => {
    const fetchMock = vi.fn(
      async (_url: string, _init: RequestInit) => new Response(null, { status: 204 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await api().rename("m 1/2", "Renamed meeting");

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("https://api.example.test/meetings/m%201%2F2");
    expect(init.method).toBe("PATCH");
    expect((init.headers as Record<string, string>).Authorization).toBe("Bearer tok-123");
    expect(JSON.parse(init.body as string)).toEqual({ title: "Renamed meeting" });
  });

  test("throws MeetingsApiError on a non-2xx response", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response("bad", { status: 400 })),
    );
    await expect(api().rename("m1", "x")).rejects.toBeInstanceOf(MeetingsApiError);
  });

  test("wraps a network error as MeetingsApiError(status 0)", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new Error("offline");
      }),
    );
    await expect(api().rename("m1", "x")).rejects.toMatchObject({ status: 0 });
  });
});
