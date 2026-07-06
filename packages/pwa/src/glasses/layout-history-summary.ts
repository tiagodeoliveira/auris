//! Glasses meeting-summary popup. A bordered text container centered on
//! the 576x288 canvas (same geometry as the assist popup) showing the
//! meeting title and summary body as ONE continuous, line-by-line scroll
//! window (no pages, no footer). The title is the first row(s) of that
//! window — it scrolls off the top with the body, it is NOT pinned.
//!
//! The firmware can't pixel-scroll, so "scrolling" means re-pushing the
//! container with the window shifted by whole display rows. We wrap the title
//! and body ourselves into exact display lines (see `./wrap`) so we own the
//! precise line array and can slide a fixed-height window across it. Single
//! event-capturing container; single-tap is a no-op, scroll up/down moves the
//! window, double-tap returns to the list (all handled in the gesture router).
//!
//! `summaryDocumentLines` + `summaryMaxOffset` are the single source of truth:
//! the layout renders `lines.slice(offset, offset + BOX_MAX_LINES)` and the
//! gesture router clamps the offset against the same `summaryMaxOffset`, so
//! they can never disagree about how far the wearer can scroll.

import { RebuildPageContainer, TextContainerProperty } from "@evenrealities/even_hub_sdk";
import type { AppState, HistorySummary } from "../types";
import { wrapToLines } from "./wrap";

export const HISTORY_SUMMARY_ID = 1;
export const HISTORY_SUMMARY_NAME = "historySum";

// Box geometry (see `box()` below). Inner text area subtracts padding
// and border from both sides; the firmware renders fixed-height rows.
const BOX_WIDTH = 480;
const BOX_HEIGHT = 220;
const BOX_PADDING = 12;
const BOX_BORDER = 2;
const LINE_HEIGHT = 27; // px per text row (firmware constant)
const INNER_WIDTH = BOX_WIDTH - 2 * (BOX_PADDING + BOX_BORDER); // 452
const INNER_HEIGHT = BOX_HEIGHT - 2 * (BOX_PADDING + BOX_BORDER); // 192
const BOX_MAX_LINES = Math.floor(INNER_HEIGHT / LINE_HEIGHT); // 7

/// The summary body wrapped to exact display lines.
export function summaryBodyLines(summary: HistorySummary): string[] {
  return wrapToLines(summary.body, INNER_WIDTH);
}

/// The full scrollable document: the title's wrapped row(s) followed by the
/// body's. The title is NOT pinned — it's just the first rows of this single
/// line set, so it scrolls off the top like any other content. Single source
/// of truth for the line set the window slides across.
export function summaryDocumentLines(summary: HistorySummary): string[] {
  return [...wrapToLines(summary.title, INNER_WIDTH), ...summaryBodyLines(summary)];
}

/// The largest line offset the wearer can scroll to — the last window that
/// still fills (or under-fills, for a short tail) the box. Zero when the whole
/// document (title + body) already fits on screen.
export function summaryMaxOffset(summary: HistorySummary): number {
  return Math.max(0, summaryDocumentLines(summary).length - BOX_MAX_LINES);
}

function box(content: string): RebuildPageContainer {
  const c = new TextContainerProperty({
    xPosition: 48,
    yPosition: 34,
    width: BOX_WIDTH,
    height: BOX_HEIGHT,
    borderWidth: BOX_BORDER,
    borderColor: 15,
    borderRadius: 8,
    paddingLength: BOX_PADDING,
    containerID: HISTORY_SUMMARY_ID,
    containerName: HISTORY_SUMMARY_NAME,
    content,
    isEventCapture: 1,
  });
  return new RebuildPageContainer({ containerTotalNum: 1, textObject: [c] });
}

export function buildHistorySummaryLayout(state: AppState): RebuildPageContainer {
  if (state.glassesHistorySummaryLoading) return box("\n\n  Loading summary…");
  if (state.glassesHistorySummaryError) return box(`\n\n  ${state.glassesHistorySummaryError}`);
  const s = state.glassesHistorySummary;
  if (!s) return box("\n\n  (no summary)");

  const lines = summaryDocumentLines(s);
  const max = summaryMaxOffset(s);
  const offset = Math.min(Math.max(state.glassesHistorySummaryLineOffset, 0), max);
  // One continuous window over [title…, body…]; the title scrolls off the
  // top with the body once the wearer scrolls past it (it is not pinned).
  const window = lines.slice(offset, offset + BOX_MAX_LINES);
  return box(window.join("\n"));
}
