import { describe, expect, test } from "vitest";
import { measureTextWrap } from "@evenrealities/pretext";
import { defaultAppState } from "../types";
import {
  buildHistorySummaryLayout,
  HISTORY_SUMMARY_NAME,
  summaryBodyLines,
  summaryMaxOffset,
} from "./layout-history-summary";
import { wrapToLines } from "./wrap";

const INNER_WIDTH = 452;
const BOX_MAX_LINES = 7;

// How many 27px rows the firmware would stack for a rendered box body:
// each \n-separated segment wraps independently at the inner width.
function renderedRows(content: string): number {
  return content
    .split("\n")
    .reduce(
      (n, seg) => n + (seg.length === 0 ? 1 : measureTextWrap(seg, INNER_WIDTH).lineCount),
      0,
    );
}

function content(state: Parameters<typeof buildHistorySummaryLayout>[0]): string {
  return buildHistorySummaryLayout(state).textObject?.[0]?.content ?? "";
}

// The rendered content is a single scrolling document — the title's wrapped
// row(s) followed by the body's wrapped rows. The title is NOT pinned, so it
// only appears while offset 0 is still within the window.
function documentLines(summary: { title: string; body: string }): string[] {
  return [...wrapToLines(summary.title, INNER_WIDTH), ...summaryBodyLines(summary)];
}
function windowLines(c: string): string[] {
  return c.split("\n");
}

const FOOTER_MARKERS = ["Double-tap", "double-tap", "▲", "▼", "/"];
function expectNoFooter(c: string): void {
  for (const marker of FOOTER_MARKERS) expect(c).not.toContain(marker);
}

describe("buildHistorySummaryLayout", () => {
  test("renders title and body in a bordered, event-capturing box with no footer", () => {
    const state = {
      ...defaultAppState(),
      glassesView: "history_summary" as const,
      glassesHistorySummary: { title: "Roadmap sync", body: "• Shipped v1\n• Planned v2" },
    };
    const box = buildHistorySummaryLayout(state).textObject?.[0];
    expect(box?.containerName).toBe(HISTORY_SUMMARY_NAME);
    expect(box?.isEventCapture).toBe(1);
    expect(box?.borderWidth).toBe(2);
    expect(box?.content).toContain("Roadmap sync");
    expect(box?.content).toContain("• Shipped v1");
    expectNoFooter(box?.content ?? "");
  });

  test("loading variant shows a loading line", () => {
    const state = { ...defaultAppState(), glassesHistorySummaryLoading: true };
    const box = buildHistorySummaryLayout(state).textObject?.[0];
    expect(box?.content).toContain("Loading summary…");
    expect(box?.isEventCapture).toBe(1);
  });

  test("error variant shows the message and no footer", () => {
    const state = { ...defaultAppState(), glassesHistorySummaryError: "Meeting not found (404)." };
    const box = buildHistorySummaryLayout(state).textObject?.[0];
    expect(box?.content).toContain("Meeting not found (404).");
    expectNoFooter(box?.content ?? "");
  });

  test("null-summary variant (resolved but empty) is still event-capturing, no footer", () => {
    const state = { ...defaultAppState(), glassesHistorySummary: null };
    const box = buildHistorySummaryLayout(state).textObject?.[0];
    expect(box?.isEventCapture).toBe(1);
    expectNoFooter(box?.content ?? "");
  });

  test("a document that fits one screen has zero scroll offset available", () => {
    const summary = {
      title: "talk with srinivasan",
      body: ["• Shipped v1", "• Planned v2", "• Hired two"].join("\n"),
    };
    expect(summaryMaxOffset(summary)).toBe(0);
    // The whole document (title + body) is shown, title first.
    const c = content({ ...defaultAppState(), glassesHistorySummary: summary });
    expect(windowLines(c)).toEqual(documentLines(summary));
  });

  test("summaryBodyLines is exactly the body wrapped to display lines", () => {
    const summary = {
      title: "Long meeting",
      body: Array.from({ length: 30 }, (_, i) => `• Bullet point number ${i}`).join("\n"),
    };
    expect(summaryBodyLines(summary)).toEqual(wrapToLines(summary.body, INNER_WIDTH));
  });

  test("at offset 0 the window starts with the title and is the first BOX_MAX_LINES document rows", () => {
    const summary = {
      title: "Long meeting",
      body: Array.from({ length: 30 }, (_, i) => `• Bullet point number ${i}`).join("\n"),
    };
    const doc = documentLines(summary);
    const c = content({
      ...defaultAppState(),
      glassesHistorySummary: summary,
      glassesHistorySummaryLineOffset: 0,
    });
    expect(windowLines(c)[0]).toBe(wrapToLines(summary.title, INNER_WIDTH)[0]);
    expect(windowLines(c)).toEqual(doc.slice(0, BOX_MAX_LINES));
  });

  test("the title scrolls off the top as the wearer scrolls down (NOT pinned)", () => {
    const summary = {
      title: "Long meeting",
      body: Array.from({ length: 30 }, (_, i) => `• Bullet point number ${i}`).join("\n"),
    };
    const doc = documentLines(summary);
    const titleRows = wrapToLines(summary.title, INNER_WIDTH).length;
    const offset = titleRows + 2; // scrolled past the title entirely
    expect(summaryMaxOffset(summary)).toBeGreaterThanOrEqual(offset);
    const c = content({
      ...defaultAppState(),
      glassesHistorySummary: summary,
      glassesHistorySummaryLineOffset: offset,
    });
    // The window is a plain slice of the document at the offset...
    expect(windowLines(c)).toEqual(doc.slice(offset, offset + BOX_MAX_LINES));
    // ...and the title is gone — it scrolled away with the body.
    expect(c).not.toContain(summary.title);
  });

  test("an out-of-range offset is clamped to summaryMaxOffset", () => {
    const summary = {
      title: "Long meeting",
      body: Array.from({ length: 30 }, (_, i) => `• Bullet point number ${i}`).join("\n"),
    };
    const doc = documentLines(summary);
    const max = summaryMaxOffset(summary);
    const c = content({
      ...defaultAppState(),
      glassesHistorySummary: summary,
      glassesHistorySummaryLineOffset: 999,
    });
    expect(windowLines(c)).toEqual(doc.slice(max, max + BOX_MAX_LINES));
  });

  test("a multi-row title scrolls with the body — its rows lead the document, then scroll away", () => {
    const summary = {
      title: "A reasonably long meeting title that definitely wraps onto more than a single row",
      body: Array.from({ length: 30 }, (_, i) => `• Bullet point number ${i}`).join("\n"),
    };
    const titleRows = measureTextWrap(summary.title, INNER_WIDTH).lineCount;
    expect(titleRows).toBeGreaterThan(1);
    const doc = documentLines(summary);
    // The title's wrapped rows are the first rows of the scrollable document.
    expect(doc.slice(0, titleRows)).toEqual(wrapToLines(summary.title, INNER_WIDTH));
    // Scrolled fully past the title, none of its rows remain on screen.
    const c = content({
      ...defaultAppState(),
      glassesHistorySummary: summary,
      glassesHistorySummaryLineOffset: titleRows,
    });
    for (const titleRow of wrapToLines(summary.title, INNER_WIDTH)) {
      expect(windowLines(c)).not.toContain(titleRow);
    }
  });

  test("every rendered window fits the box (≤ 7 rows)", () => {
    const summary = {
      title: "A reasonably long meeting title here",
      body: Array.from({ length: 40 }, (_, i) => `• Bullet point number ${i}`).join("\n"),
    };
    for (let offset = 0; offset <= summaryMaxOffset(summary); offset++) {
      const c = content({
        ...defaultAppState(),
        glassesHistorySummary: summary,
        glassesHistorySummaryLineOffset: offset,
      });
      expect(renderedRows(c)).toBeLessThanOrEqual(BOX_MAX_LINES);
    }
  });
});

describe("summaryMaxOffset", () => {
  test("is zero for an empty body", () => {
    expect(summaryMaxOffset({ title: "t", body: "" })).toBe(0);
  });

  test("equals documentLines.length - BOX_MAX_LINES for a long body", () => {
    const summary = {
      title: "Long meeting",
      body: Array.from({ length: 30 }, (_, i) => `• Bullet point number ${i}`).join("\n"),
    };
    expect(summaryMaxOffset(summary)).toBe(documentLines(summary).length - BOX_MAX_LINES);
  });
});
