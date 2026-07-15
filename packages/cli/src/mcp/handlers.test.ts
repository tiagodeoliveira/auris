import { describe, expect, it } from "vitest";
import type { MeetingApi, RawMeetingDetail, RawMeetingSummary } from "../core/client.js";
import { AuthError, NotFoundError } from "../core/client.js";
import { makeTools } from "./handlers.js";

function fakeClient(over: Partial<MeetingApi> = {}): MeetingApi {
  return {
    listMeetings: async () => [],
    getMeeting: async () => {
      throw new Error("not stubbed");
    },
    getMomentScreenshot: async () => ({ bytes: new Uint8Array(), mimeType: "image/png" }),
    ...over,
  };
}

const summaries: RawMeetingSummary[] = [
  {
    id: "m1",
    description: "Axon",
    metadata: { title: "Axon strategy", project: "axon" },
    started_at: "2026-07-14T20:00:00Z",
    ended_at: "2026-07-14T20:30:00Z",
  },
  {
    id: "m2",
    description: "Standup",
    metadata: { project: "helix" },
    started_at: "2026-07-10T09:00:00Z",
    ended_at: null,
  },
];

function tool(name: string, client: MeetingApi) {
  const t = makeTools(client).find((x) => x.name === name);
  if (!t) throw new Error(`no tool ${name}`);
  return t;
}

function parse(res: { content: { text: string }[] }) {
  return JSON.parse(res.content[0].text);
}

describe("makeTools", () => {
  it("exposes exactly the five tools in order", () => {
    expect(makeTools(fakeClient()).map((t) => t.name)).toEqual([
      "list_meetings",
      "search_meetings",
      "get_meeting",
      "get_meeting_transcript",
      "get_moment_screenshot",
    ]);
  });

  it("list_meetings maps summaries and respects limit", async () => {
    const res = await tool(
      "list_meetings",
      fakeClient({ listMeetings: async () => summaries }),
    ).handler({ limit: 1 });
    const out = parse(res);
    expect(out).toHaveLength(1);
    expect(out[0]).toMatchObject({
      id: "m1",
      title: "Axon strategy",
      project: "axon",
      duration_min: 30,
    });
  });

  it("search_meetings filters then maps", async () => {
    const res = await tool(
      "search_meetings",
      fakeClient({ listMeetings: async () => summaries }),
    ).handler({ project: "helix" });
    const out = parse(res);
    expect(out).toHaveLength(1);
    expect(out[0].id).toBe("m2");
  });

  it("get_meeting returns a briefing without transcript", async () => {
    const detail: RawMeetingDetail = {
      id: "m1",
      description: "Axon",
      metadata: { title: "Axon strategy" },
      started_at: "2026-07-14T20:00:00Z",
      ended_at: "2026-07-14T20:30:00Z",
      wrap_up_status: "success",
      transcript: [{ id: "i1", text: "hi", t: 0 }],
      moments: [],
      items_by_mode: { summary: [{ id: "s1", text: "a summary", t: 0 }] },
    };
    const res = await tool("get_meeting", fakeClient({ getMeeting: async () => detail })).handler({
      id: "m1",
    });
    const out = parse(res);
    expect(out.summary).toEqual(["a summary"]);
    expect(out).not.toHaveProperty("transcript");
  });

  it("get_meeting_transcript paginates", async () => {
    const detail: RawMeetingDetail = {
      id: "m1",
      description: null,
      metadata: null,
      started_at: "2026-07-14T20:00:00Z",
      ended_at: null,
      wrap_up_status: null,
      transcript: [
        { id: "i1", text: "a", t: 0 },
        { id: "i2", text: "b", t: 1 },
      ],
      moments: [],
      items_by_mode: {},
    };
    const res = await tool(
      "get_meeting_transcript",
      fakeClient({ getMeeting: async () => detail }),
    ).handler({ id: "m1", offset: 1, limit: 5 });
    const out = parse(res);
    expect(out).toEqual({
      total: 2,
      offset: 1,
      items: [{ id: "i2", t: 1, speaker: null, text: "b" }],
    });
  });

  it("renders AuthError as an isError tool result", async () => {
    const res = await tool(
      "list_meetings",
      fakeClient({
        listMeetings: async () => {
          throw new AuthError();
        },
      }),
    ).handler({});
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toMatch(/not logged in/);
  });

  it("renders NotFoundError as an isError tool result", async () => {
    const res = await tool(
      "get_meeting",
      fakeClient({
        getMeeting: async () => {
          throw new NotFoundError();
        },
      }),
    ).handler({ id: "x" });
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toMatch(/not found/);
  });

  it("exposes get_moment_screenshot as the 5th tool returning an image block", async () => {
    const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47]);
    const t = tool(
      "get_moment_screenshot",
      fakeClient({ getMomentScreenshot: async () => ({ bytes: png, mimeType: "image/png" }) }),
    );
    const res = await t.handler({ meeting_id: "m1", moment_id: "mo1" });
    expect(res.isError).toBeFalsy();
    expect(res.content[0]).toEqual({
      type: "image",
      data: Buffer.from(png).toString("base64"),
      mimeType: "image/png",
    });
  });

  it("renders NotFoundError from get_moment_screenshot as an isError result", async () => {
    const t = tool(
      "get_moment_screenshot",
      fakeClient({
        getMomentScreenshot: async () => {
          throw new NotFoundError();
        },
      }),
    );
    const res = await t.handler({ meeting_id: "m1", moment_id: "x" });
    expect(res.isError).toBe(true);
  });
});
