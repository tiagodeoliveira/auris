//! Glasses "List meetings" history list. A firmware ListContainer of
//! meeting names (the firmware draws the `>` cursor + selection border
//! and owns touchpad scroll); single-tap selects a row. Loading, empty,
//! and error states render as a plain text container instead — every
//! variant is `isEventCapture: 1` so a double-tap-back is captured on
//! all of them (one event-capturing container per page is the firmware
//! limit). Container id/name are shared across all variants so the
//! gesture router can recognise list taps by `containerID`.

import {
  ListContainerProperty,
  ListItemContainerProperty,
  RebuildPageContainer,
  TextContainerProperty,
} from "@evenrealities/even_hub_sdk";
import type { AppState } from "../types";
import { pickDetailTitle } from "../meeting-format";

// Shares the value 1 with ENTRY_LIST_CONTAINER_ID — safe because only
// one view is active at a time and the gesture router always checks
// `glassesView` before `containerID`. Keep that disambiguation if a
// third id-1 list is ever added.
export const HISTORY_LIST_CONTAINER_ID = 1;
export const HISTORY_LIST_CONTAINER_NAME = "historyList";

// Firmware rejects a RebuildPageContainer whose list item text exceeds
// 63 *bytes* (UTF-8), not characters — `pickDetailTitle`'s 80-char cap
// is a web-modal budget and can overflow this. Cap each row here, on a
// codepoint boundary, so we never split a multi-byte sequence into tofu.
const LIST_ITEM_MAX_BYTES = 63;

const encoder = new TextEncoder();

function truncateToBytes(text: string, maxBytes: number): string {
  if (encoder.encode(text).length <= maxBytes) return text;
  const ellipsis = "…"; // U+2026, 3 bytes, renders on the firmware base font
  const budget = maxBytes - encoder.encode(ellipsis).length;
  let used = 0;
  let out = "";
  for (const ch of text) {
    const chBytes = encoder.encode(ch).length;
    if (used + chBytes > budget) break;
    out += ch;
    used += chBytes;
  }
  return out.trimEnd() + ellipsis;
}

function fullScreenText(content: string): TextContainerProperty {
  return new TextContainerProperty({
    xPosition: 0,
    yPosition: 0,
    width: 576,
    height: 288,
    borderWidth: 0,
    paddingLength: 12,
    containerID: HISTORY_LIST_CONTAINER_ID,
    containerName: HISTORY_LIST_CONTAINER_NAME,
    content,
    isEventCapture: 1,
  });
}

export function buildHistoryListLayout(state: AppState): RebuildPageContainer {
  if (state.glassesHistoryLoading) {
    return new RebuildPageContainer({
      containerTotalNum: 1,
      textObject: [fullScreenText("\n\n  Loading meetings…")],
    });
  }
  if (state.glassesHistoryError) {
    return new RebuildPageContainer({
      containerTotalNum: 1,
      textObject: [fullScreenText(`\n\n  ${state.glassesHistoryError}\n\n  Double-tap to go back`)],
    });
  }
  const meetings = state.glassesHistory;
  if (meetings.length === 0) {
    return new RebuildPageContainer({
      containerTotalNum: 1,
      textObject: [fullScreenText("\n\n  No meetings yet\n\n  Double-tap to go back")],
    });
  }
  const labels = meetings.map((m) => truncateToBytes(pickDetailTitle(m), LIST_ITEM_MAX_BYTES));
  const list = new ListContainerProperty({
    xPosition: 0,
    yPosition: 0,
    width: 576,
    height: 288,
    borderWidth: 0,
    paddingLength: 12,
    containerID: HISTORY_LIST_CONTAINER_ID,
    containerName: HISTORY_LIST_CONTAINER_NAME,
    isEventCapture: 1,
    itemContainer: new ListItemContainerProperty({
      itemCount: labels.length,
      itemWidth: 0,
      isItemSelectBorderEn: 1,
      itemName: labels,
    }),
  });
  return new RebuildPageContainer({ containerTotalNum: 1, listObject: [list] });
}
