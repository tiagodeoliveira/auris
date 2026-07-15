import { describe, expect, it } from "vitest";
import type { MeetingApi, RawMeetingDetail, RawMeetingSummary } from "../../core/client.js";
import { getCmd, listCmd, searchCmd, transcriptCmd } from "./meetings.js";

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
const detail: RawMeetingDetail = {
  id: "m1",
  description: "Axon",
  metadata: { title: "Axon strategy" },
  started_at: "2026-07-14T20:00:00Z",
  ended_at: "2026-07-14T20:30:00Z",
  wrap_up_status: "success",
  transcript: [{ id: "i1", text: "[Speaker 1] hi", t: 0 }],
  moments: [],
  items_by_mode: { summary: [{ id: "s1", text: "a summary", t: 0 }] },
};
const api: MeetingApi = { listMeetings: async () => summaries, getMeeting: async () => detail };

describe("meetings commands", () => {
  it("list --json emits mapped summaries", async () => {
    const out = JSON.parse(await listCmd(api, { json: true }));
    expect(out).toHaveLength(2);
    expect(out[0]).toMatchObject({ id: "m1", title: "Axon strategy", duration_min: 30 });
  });

  it("list (human) includes titles, not raw JSON", async () => {
    const out = await listCmd(api, {});
    expect(out).toContain("Axon strategy");
    expect(out.trim().startsWith("[")).toBe(false);
  });

  it("search filters by project", async () => {
    const out = JSON.parse(await searchCmd(api, { project: "helix", json: true }));
    expect(out).toHaveLength(1);
    expect(out[0].id).toBe("m2");
  });

  it("get returns a briefing without a transcript", async () => {
    const out = JSON.parse(await getCmd(api, "m1", { json: true }));
    expect(out.summary).toEqual(["a summary"]);
    expect(out).not.toHaveProperty("transcript");
  });

  it("transcript paginates", async () => {
    const out = JSON.parse(await transcriptCmd(api, "m1", { json: true }));
    expect(out).toEqual({
      total: 1,
      offset: 0,
      items: [{ id: "i1", t: 0, speaker: null, text: "[Speaker 1] hi" }],
    });
  });
});
