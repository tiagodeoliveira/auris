//! Assist-mode popup. Bordered box centered on the 576x288 canvas;
//! interrupts any other view when a fresh assist item lands and is
//! dismissed by a single click (any input event, really — see
//! gesture-router). The popup is a separate page topology, so
//! entering and leaving it both go through `rebuildPageContainer`
//! (brief flicker — same cost as quick_asks sub-state transitions).
//!
//! Container budget: a single text container with `isEventCapture: 1`
//! covers display, border chrome, and event capture in one go. No
//! z-order means the popup must be the only thing on its page — the
//! underlying mode's containers are torn down by the rebuild and
//! recreated on dismiss.

import { RebuildPageContainer, TextContainerProperty } from "@evenrealities/even_hub_sdk";
import type { Item } from "../types";

const ASSIST_POPUP_ID = 1;
const ASSIST_POPUP_NAME = "assistPop";

/// Maps the server's `meta.type` tag onto a short header label for the
/// popup. The PWA + mobile use emoji (📖 ❓ 🧠 💡) for the same tags,
/// but the glasses firmware font (LVGL base, Latin + basic punctuation
/// only) has no glyphs for astral-plane emoji — they render as tofu and
/// spam `glyph dsc. not found` warnings. So on this surface we use an
/// ASCII type word instead. Default to the coach label when the tag is
/// missing or unrecognised — assist items always carry one of the four
/// canonical tags today, but staying defensive avoids a blank header if
/// the schema drifts.
export function assistTypeGlyph(item: Item): string {
  const t = (item.meta as { type?: string } | undefined)?.type;
  switch (t) {
    case "definition":
      return "DEFINITION";
    case "question":
      return "QUESTION";
    case "memory":
      return "MEMORY";
    case "coach":
    default:
      return "COACH";
  }
}

/// Compose the popup body from the item's `text` (the headline) and
/// optional `detail` (longer supporting prose). Truncates to ~600
/// chars from the START — the headline + opening of the detail are
/// what matter; truncate the tail. Leaves a "Tap to dismiss" footer
/// on its own line at the bottom.
function popupBody(item: Item): string {
  const glyph = assistTypeGlyph(item);
  const headline = (item.text ?? "").trim();
  const detail = (item.detail ?? "").trim();
  const MAX = 600;
  let body = `${glyph}\n\n${headline}`;
  if (detail.length > 0) {
    body += `\n\n${detail}`;
  }
  if (body.length > MAX) {
    body = body.slice(0, MAX - 1) + "…";
  }
  return `${body}\n\n   Tap to dismiss`;
}

/// Bordered text container centered on the canvas. The numbers are
/// chosen so the box leaves ~48px breathing room left/right and
/// ~34px top/bottom on the 576x288 canvas — enough margin that the
/// border doesn't read as a frame around the whole display.
export function buildAssistPopupLayout(item: Item): RebuildPageContainer {
  const box: TextContainerProperty = new TextContainerProperty({
    xPosition: 48,
    yPosition: 34,
    width: 480,
    height: 220,
    borderWidth: 2,
    borderColor: 15,
    borderRadius: 8,
    paddingLength: 12,
    containerID: ASSIST_POPUP_ID,
    containerName: ASSIST_POPUP_NAME,
    content: popupBody(item),
    isEventCapture: 1,
  });
  return new RebuildPageContainer({
    containerTotalNum: 1,
    textObject: [box],
  });
}
