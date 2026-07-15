import { describe, expect, it } from "vitest";
import type { RawMeetingDetail, RawMeetingSummary } from "./client.js";
import { matchesFilters, paginateTranscript, speakerOf, toBriefing, toSummary } from "./shape.js";

const raw: RawMeetingSummary = {
  id: "m1",
  description: "Axon 1:1",
  metadata: { title: "Axon strategy", project: "axon" },
  started_at: "2026-07-14T20:35:00Z",
  ended_at: "2026-07-14T21:00:00Z",
};

describe("toSummary", () => {
  it("lifts title/project from metadata and computes duration", () => {
    expect(toSummary(raw)).toEqual({
      id: "m1",
      title: "Axon strategy",
      project: "axon",
      started_at: "2026-07-14T20:35:00Z",
      ended_at: "2026-07-14T21:00:00Z",
      duration_min: 25,
    });
  });

  it("falls back to description, then to (untitled); null project; null duration when ongoing", () => {
    const noTitle = toSummary({ ...raw, metadata: {}, ended_at: null });
    expect(noTitle.title).toBe("Axon 1:1");
    expect(noTitle.project).toBeNull();
    expect(noTitle.duration_min).toBeNull();
    const bare = toSummary({ ...raw, description: null, metadata: null });
    expect(bare.title).toBe("(untitled)");
  });
});

describe("matchesFilters", () => {
  it("matches query against title and description, case-insensitively", () => {
    expect(matchesFilters(raw, { query: "STRATEGY" })).toBe(true);
    expect(matchesFilters(raw, { query: "1:1" })).toBe(true);
    expect(matchesFilters(raw, { query: "nope" })).toBe(false);
  });

  it("filters by exact project and by date range on started_at", () => {
    expect(matchesFilters(raw, { project: "axon" })).toBe(true);
    expect(matchesFilters(raw, { project: "other" })).toBe(false);
    expect(matchesFilters(raw, { since: "2026-07-14", until: "2026-07-14" })).toBe(true);
    expect(matchesFilters(raw, { since: "2026-07-15" })).toBe(false);
    expect(matchesFilters(raw, { until: "2026-07-13" })).toBe(false);
  });

  it("ANDs all provided filters and treats no filters as a match", () => {
    expect(matchesFilters(raw, {})).toBe(true);
    expect(matchesFilters(raw, { query: "strategy", project: "other" })).toBe(false);
  });
});

const detail: RawMeetingDetail = {
  id: "m1",
  description: "Axon 1:1",
  metadata: { title: "Axon strategy", project: "axon" },
  started_at: "2026-07-14T20:35:00Z",
  ended_at: "2026-07-14T21:00:00Z",
  wrap_up_status: "success",
  transcript: [
    { id: "i1", text: "[Speaker 1] hello", t: 0, meta: { speaker: "1" } },
    { id: "i2", text: "[Speaker 2] hi", t: 1200 },
    { id: "i3", text: "[Speaker 1] bye", t: 2400, meta: { speaker: "1" } },
  ],
  moments: [
    {
      id: "mo1",
      kind: "screenshot",
      t: 500,
      note: "slide",
      summary: "roadmap",
      summary_status: "success",
      screenshot_url: "/x.png",
    },
    {
      id: "mo2",
      kind: "note",
      t: 900,
      note: null,
      summary: null,
      summary_status: "none",
      screenshot_url: null,
    },
  ],
  items_by_mode: {
    summary: [{ id: "s1", text: "They discussed Axon strategy.", t: 0 }],
    highlights: [{ id: "h1", text: "$900M business", t: 0 }],
    actions: [],
    chat: [
      { id: "c1", text: "what's going on here?", t: 0, meta: { role: "user" } },
      { id: "c2", text: "It's an interview.", t: 0, meta: { role: "assistant" } },
      { id: "c3", text: "no role here", t: 0 },
    ],
  },
};

describe("toBriefing", () => {
  it("maps mode item texts, includes wrap_up_status, omits transcript, drops screenshot_url", () => {
    const b = toBriefing(detail);
    expect(b.title).toBe("Axon strategy");
    expect(b.wrap_up_status).toBe("success");
    expect(b.summary).toEqual(["They discussed Axon strategy."]);
    expect(b.highlights).toEqual(["$900M business"]);
    expect(b.actions).toEqual([]);
    expect(b.open_questions).toEqual([]);
    expect(b.moments).toEqual([
      {
        id: "mo1",
        kind: "screenshot",
        t: 500,
        note: "slide",
        summary: "roadmap",
        has_screenshot: true,
      },
      { id: "mo2", kind: "note", t: 900, note: null, summary: null, has_screenshot: false },
    ]);
    expect(b).not.toHaveProperty("transcript");
    expect(b.chat).toEqual([
      { role: "user", text: "what's going on here?" },
      { role: "assistant", text: "It's an interview." },
      { role: "unknown", text: "no role here" },
    ]);
  });

  it("returns an empty chat array when items_by_mode has no chat key", () => {
    const noChat = toBriefing({ ...detail, items_by_mode: { summary: [] } });
    expect(noChat.chat).toEqual([]);
  });
});

describe("paginateTranscript", () => {
  it("slices to {id,t,speaker,text} and reports the true total", () => {
    const page = paginateTranscript(detail, 1, 2);
    expect(page).toEqual({
      total: 3,
      offset: 1,
      items: [
        { id: "i2", t: 1200, speaker: null, text: "[Speaker 2] hi" },
        { id: "i3", t: 2400, speaker: "Speaker 1", text: "[Speaker 1] bye" },
      ],
    });
  });

  it("clamps an out-of-range offset to an empty page", () => {
    expect(paginateTranscript(detail, 99, 10)).toEqual({ total: 3, offset: 99, items: [] });
  });
});

describe("speakerOf", () => {
  it("returns 'Speaker N' from meta.speaker (string or number)", () => {
    expect(speakerOf({ id: "i", text: "x", t: 0, meta: { speaker: "1" } })).toBe("Speaker 1");
    expect(speakerOf({ id: "i", text: "x", t: 0, meta: { speaker: 2 } })).toBe("Speaker 2");
  });
  it("returns null when meta is missing, not an object, or has no speaker", () => {
    expect(speakerOf({ id: "i", text: "x", t: 0 })).toBeNull();
    expect(speakerOf({ id: "i", text: "x", t: 0, meta: "nope" })).toBeNull();
    expect(speakerOf({ id: "i", text: "x", t: 0, meta: { other: "1" } })).toBeNull();
    expect(speakerOf({ id: "i", text: "x", t: 0, meta: { speaker: "" } })).toBeNull();
  });
});
