//! Describe-meeting flow — three sub-screens that share the transcript
//! preview box so transitions can update content with flicker-free
//! `textContainerUpgrade` calls where possible:
//!
//!   describe_idle      "Describe meeting"          (tap to start)
//!                      ┌─────────────────────────┐
//!                      │ Tap to start description!│
//!                      └─────────────────────────┘
//!
//!   listening          "Describing…"               (live capture)
//!                      ┌─────────────────────────┐
//!                      │ <live transcript>        │
//!                      └─────────────────────────┘
//!
//!   describe_confirm   ▸ Start the meeting          (select to act)
//!                        Describe (to try again)
//!                      ┌─────────────────────────┐
//!                      │ <captured transcript>    │
//!                      └─────────────────────────┘
//!
//! The first two states share a text header at the top and a bordered
//! body box below — the body owns `isEventCapture` so a single tap
//! advances the flow. The confirm state swaps the header out for a
//! 2-item list (which natively handles scroll-up/scroll-down + select)
//! and the body becomes a read-only transcript preview.

import {
  ListContainerProperty,
  ListItemContainerProperty,
  RebuildPageContainer,
  TextContainerProperty,
  TextContainerUpgrade,
} from "@evenrealities/even_hub_sdk";
import type { AppState } from "../types";

export const DESCRIBE_HEADER_ID = 1;
export const DESCRIBE_HEADER_NAME = "describeHdr";
export const DESCRIBE_BODY_ID = 2;
export const DESCRIBE_BODY_NAME = "describeBody";
export const DESCRIBE_LIST_ID = 3;
export const DESCRIBE_LIST_NAME = "confirmList";

/// Indices for the confirm screen's list, reported back via
/// `list_event.currentSelectItemIndex`. Mirrored on the
/// gesture-router side.
export const CONFIRM_ITEM_GENERATE = 0;
export const CONFIRM_ITEM_TRY_AGAIN = 1;

const CONFIRM_ITEMS = ["Start the meeting", "Describe (to try again)"];

const HEADER_X = 0;
const HEADER_Y = 0;
const HEADER_WIDTH = 576;
const HEADER_HEIGHT = 48;

const BODY_X = 0;
const BODY_Y_TEXT_HEADER = 56;
const BODY_Y_LIST_HEADER = 104;
const BODY_WIDTH = 576;
const BODY_HEIGHT_TEXT_HEADER = 224;
const BODY_HEIGHT_LIST_HEADER = 176;

/// Hint shown in the empty body box on the initial describe screen.
const DESCRIBE_HINT = "Tap to start description!";
/// Max characters held in the body container at once. The firmware
/// clips beyond this anyway; trimming on our side keeps payloads
/// predictable for `textContainerUpgrade`.
const MAX_BODY_CHARS = 600;

function headerText(content: string): TextContainerProperty {
  return new TextContainerProperty({
    xPosition: HEADER_X,
    yPosition: HEADER_Y,
    width: HEADER_WIDTH,
    height: HEADER_HEIGHT,
    borderWidth: 0,
    paddingLength: 8,
    containerID: DESCRIBE_HEADER_ID,
    containerName: DESCRIBE_HEADER_NAME,
    content,
    isEventCapture: 0,
  });
}

function bodyText(
  content: string,
  yPosition: number,
  height: number,
  isEventCapture: 0 | 1,
): TextContainerProperty {
  return new TextContainerProperty({
    xPosition: BODY_X,
    yPosition,
    width: BODY_WIDTH,
    height,
    borderWidth: 1,
    borderColor: 8,
    borderRadius: 4,
    paddingLength: 10,
    containerID: DESCRIBE_BODY_ID,
    containerName: DESCRIBE_BODY_NAME,
    content,
    isEventCapture,
  });
}

function confirmList(): ListContainerProperty {
  return new ListContainerProperty({
    xPosition: 0,
    yPosition: 0,
    width: 576,
    height: 96,
    borderWidth: 0,
    paddingLength: 8,
    containerID: DESCRIBE_LIST_ID,
    containerName: DESCRIBE_LIST_NAME,
    isEventCapture: 1,
    itemContainer: new ListItemContainerProperty({
      itemCount: CONFIRM_ITEMS.length,
      itemWidth: 0,
      isItemSelectBorderEn: 1,
      itemName: CONFIRM_ITEMS,
    }),
  });
}

/// Read the body content for the live-transcript states. Reused by
/// the initial rebuild and by `textContainerUpgrade` deltas during
/// the listening session.
export function describeBodyContent(state: AppState): string {
  const full = state.listeningTranscript + state.listeningInterim;
  if (full.length === 0) return " "; // forces firmware clear when empty
  if (full.length <= MAX_BODY_CHARS) return full;
  return "…" + full.slice(full.length - MAX_BODY_CHARS + 1);
}

/// Initial describe screen — header prompt + empty bordered body
/// with the hint. Body owns event capture so a single tap kicks the
/// flow into listening.
export function buildDescribeIdleLayout(): RebuildPageContainer {
  return new RebuildPageContainer({
    containerTotalNum: 2,
    textObject: [
      headerText("Describe meeting"),
      bodyText(DESCRIBE_HINT, BODY_Y_TEXT_HEADER, BODY_HEIGHT_TEXT_HEADER, 1),
    ],
  });
}

/// Listening (Describing…) screen — same shape as describe_idle, but
/// the header reads "Describing…" and the body box fills with the
/// live transcript. Body keeps event capture so a single tap commits
/// the description (VAD silence also commits).
export function buildListeningLayout(state: AppState): RebuildPageContainer {
  return new RebuildPageContainer({
    containerTotalNum: 2,
    textObject: [
      headerText("Describing…"),
      bodyText(describeBodyContent(state), BODY_Y_TEXT_HEADER, BODY_HEIGHT_TEXT_HEADER, 1),
    ],
  });
}

/// Confirm screen — 2-item list at top (event capture for scroll +
/// select), bordered transcript preview below.
export function buildDescribeConfirmLayout(state: AppState): RebuildPageContainer {
  return new RebuildPageContainer({
    containerTotalNum: 2,
    listObject: [confirmList()],
    textObject: [
      bodyText(transcriptPreview(state), BODY_Y_LIST_HEADER, BODY_HEIGHT_LIST_HEADER, 0),
    ],
  });
}

/// Live-update payload for the body box during the listening state —
/// fires on every transcript / interim delta so the user sees words
/// stream in without a full rebuild.
export function buildListeningBodyUpgrade(state: AppState): TextContainerUpgrade {
  const content = describeBodyContent(state);
  return new TextContainerUpgrade({
    containerID: DESCRIBE_BODY_ID,
    containerName: DESCRIBE_BODY_NAME,
    contentOffset: 0,
    contentLength: content.length,
    content,
  });
}

function transcriptPreview(state: AppState): string {
  const text = state.listeningTranscript.trim();
  if (text.length === 0) return " ";
  if (text.length <= MAX_BODY_CHARS) return text;
  return "…" + text.slice(text.length - MAX_BODY_CHARS + 1);
}
