import { describe, expect, it } from "vitest";
import type { RawMeetingDetail, RawMeetingSummary } from "./client.js";
import { matchesFilters, paginateTranscript, toBriefing, toSummary } from "./shape.js";

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
    { id: "i1", text: "[Speaker 1] hello", t: 0 },
    { id: "i2", text: "[Speaker 2] hi", t: 1200 },
    { id: "i3", text: "[Speaker 1] bye", t: 2400 },
  ],
  moments: [
    {
      id: "mo1",
      kind: "screenshot",
      t: 500,
      note: "slide",
      summary: "roadmap slide",
      summary_status: "success",
      screenshot_url: "/x.png",
    },
  ],
  items_by_mode: {
    summary: [{ id: "s1", text: "They discussed Axon strategy.", t: 0 }],
    highlights: [{ id: "h1", text: "$900M business", t: 0 }],
    actions: [],
    chat: [{ id: "c1", text: "ignore me", t: 0 }],
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
      { kind: "screenshot", t: 500, note: "slide", summary: "roadmap slide" },
    ]);
    expect(b).not.toHaveProperty("transcript");
    expect(JSON.stringify(b)).not.toContain("ignore me"); // chat mode excluded
  });
});

describe("paginateTranscript", () => {
  it("slices to {id,t,text} and reports the true total", () => {
    const page = paginateTranscript(detail, 1, 2);
    expect(page).toEqual({
      total: 3,
      offset: 1,
      items: [
        { id: "i2", t: 1200, text: "[Speaker 2] hi" },
        { id: "i3", t: 2400, text: "[Speaker 1] bye" },
      ],
    });
  });

  it("clamps an out-of-range offset to an empty page", () => {
    expect(paginateTranscript(detail, 99, 10)).toEqual({ total: 3, offset: 99, items: [] });
  });
});
