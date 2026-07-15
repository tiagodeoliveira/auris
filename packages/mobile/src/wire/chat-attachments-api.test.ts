import { afterEach, describe, expect, it, vi } from "vitest";

import { ChatAttachmentsApi } from "./chat-attachments-api";
import { MeetingsApiError } from "./meetings-api";

const token = async () => "tok-123";

afterEach(() => {
  vi.restoreAllMocks();
});

describe("ChatAttachmentsApi", () => {
  it("returns null for a non-ws server url", () => {
    expect(ChatAttachmentsApi.from("http://nope", token)).toBeNull();
  });

  it("POSTs raw bytes to the chat_attachments endpoint with auth + mime", async () => {
    const fetchMock = vi.fn(
      async () => new Response(JSON.stringify({ id: "att-1" }), { status: 201 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const api = ChatAttachmentsApi.from("wss://host.example:8443/ws", token)!;
    const body = new Blob([new Uint8Array([1, 2, 3])], { type: "image/jpeg" });
    const id = await api.upload("m-9", body, "image/jpeg");

    expect(id).toBe("att-1");
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("https://host.example:8443/meetings/m-9/chat_attachments");
    expect(init.method).toBe("POST");
    const headers = init.headers as Record<string, string>;
    expect(headers.Authorization).toBe("Bearer tok-123");
    expect(headers["Content-Type"]).toBe("image/jpeg");
    expect(init.body).toBe(body);
  });

  it("throws MeetingsApiError on a non-2xx response", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response("nope", { status: 413 })),
    );
    const api = ChatAttachmentsApi.from("wss://host.example/ws", token)!;
    const body = new Blob([new Uint8Array([1])], { type: "image/jpeg" });
    await expect(api.upload("m-1", body, "image/jpeg")).rejects.toBeInstanceOf(
      MeetingsApiError,
    );
  });
});
