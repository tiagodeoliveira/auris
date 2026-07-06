//! Quick Asks mode layout. Three sub-states sharing the same page:
//!
//!   list                                 (default — pick a quick ask)
//!   ┌─────────────────────────────┐
//!   │ > Status report             │
//!   │   Open action items         │
//!   │   AI decisions              │
//!   └─────────────────────────────┘
//!
//!   waiting              (after pick — locked spinner)
//!   ┌─────────────────────────────┐
//!   │                             │
//!   │      Asking…                │
//!   │                             │
//!   │ Tap to return to the list   │
//!   └─────────────────────────────┘
//!
//!   answer                       (server returned a chat answer)
//!   ┌─────────────────────────────┐
//!   │ <wrapped answer text>       │
//!   │                             │
//!   └─────────────────────────────┘
//!
//! Distinct from the standard active-list layout because the items
//! are user-curated (the *label* — not the full prompt — is shown
//! in the list) and the wait/answer states are bespoke to this mode.

import {
  ListContainerProperty,
  ListItemContainerProperty,
  RebuildPageContainer,
  TextContainerProperty,
  TextContainerUpgrade,
} from "@evenrealities/even_hub_sdk";
import type { AppState, Item } from "../types";
import { activeGlassesItems } from "../types";
import { activityIndicator } from "./layout-active-list";

export const QUICK_ASKS_LIST_ID = 1;
export const QUICK_ASKS_LIST_NAME = "quickList";
export const QUICK_ASKS_TEXT_ID = 2;
export const QUICK_ASKS_TEXT_NAME = "quickText";

const FRAMES = ["Asking.", "Asking..", "Asking…"];
export const QUICK_ASKS_SPINNER_FRAME_INTERVAL_MS = 500;

export function quickAsksSpinnerFrame(index: number): string {
  return FRAMES[index % FRAMES.length] ?? FRAMES[0];
}

function listContainer(items: Item[]): ListContainerProperty {
  const labels = items.length > 0 ? items.map((it) => it.text) : ["(no quick asks)"];
  return new ListContainerProperty({
    xPosition: 0,
    yPosition: 0,
    width: 576,
    height: 288,
    borderWidth: 0,
    paddingLength: 12,
    containerID: QUICK_ASKS_LIST_ID,
    containerName: QUICK_ASKS_LIST_NAME,
    isEventCapture: 1,
    itemContainer: new ListItemContainerProperty({
      itemCount: labels.length,
      itemWidth: 0,
      isItemSelectBorderEn: items.length > 0 ? 1 : 0,
      itemName: labels,
    }),
  });
}

function fullScreenText(content: string): TextContainerProperty {
  return new TextContainerProperty({
    xPosition: 0,
    yPosition: 0,
    width: 576,
    height: 288,
    borderWidth: 0,
    paddingLength: 12,
    containerID: QUICK_ASKS_TEXT_ID,
    containerName: QUICK_ASKS_TEXT_NAME,
    content,
    isEventCapture: 1,
  });
}

/// Returns which sub-state should render given the current store.
/// Centralised so render + tests share one truth.
export type QuickAsksSubstate = "list" | "waiting" | "answer";
export function quickAsksSubstate(state: AppState): QuickAsksSubstate {
  if (state.quickAskWaiting) return "waiting";
  if (state.quickAskAnswerText !== null) return "answer";
  return "list";
}

/// Body content for the waiting state — frame 0 initial, animation
/// driver in render.ts ticks via textContainerUpgrade.
function waitingBody(): string {
  return `\n\n      ${quickAsksSpinnerFrame(0)}\n\n  Tap to return to the list`;
}

function answerBody(text: string): string {
  const trimmed = text.trim();
  if (trimmed.length === 0) return " ";
  const MAX = 600;
  if (trimmed.length <= MAX) return trimmed;
  return "…" + trimmed.slice(trimmed.length - MAX + 1);
}

export function buildQuickAsksLayout(state: AppState): RebuildPageContainer {
  // Every sub-state carries the shared top-right recording indicator
  // so the wearer keeps seeing that meeting audio is still flowing
  // while they browse / wait inside Quick Asks. It reuses the same
  // container id/name as the active-list layout, so the renderer's
  // activity-frame animation drives it identically here.
  const sub = quickAsksSubstate(state);
  if (sub === "list") {
    return new RebuildPageContainer({
      containerTotalNum: 2,
      listObject: [listContainer(activeGlassesItems(state))],
      textObject: [activityIndicator(state)],
    });
  }
  if (sub === "waiting") {
    return new RebuildPageContainer({
      containerTotalNum: 2,
      textObject: [fullScreenText(waitingBody()), activityIndicator(state)],
    });
  }
  return new RebuildPageContainer({
    containerTotalNum: 2,
    textObject: [
      fullScreenText(answerBody(state.quickAskAnswerText ?? "")),
      activityIndicator(state),
    ],
  });
}

/// Flicker-free body upgrade for the spinner animation. Only valid
/// while we're in the `waiting` sub-state (caller checks).
export function buildQuickAsksSpinnerUpgrade(frameIndex: number): TextContainerUpgrade {
  const content = `\n\n      ${quickAsksSpinnerFrame(frameIndex)}\n\n  Tap to return to the list`;
  return new TextContainerUpgrade({
    containerID: QUICK_ASKS_TEXT_ID,
    containerName: QUICK_ASKS_TEXT_NAME,
    contentOffset: 0,
    contentLength: content.length,
    content,
  });
}

/// Flicker-free body upgrade for streaming answer text. Only valid
/// while we're already in the `answer` sub-state (caller checks);
/// a fresh entry into that sub-state needs the full
/// `rebuildPageContainer` from `buildQuickAsksLayout` to create
/// the text container in the first place.
export function buildQuickAsksAnswerUpgrade(text: string): TextContainerUpgrade {
  const content = answerBody(text);
  return new TextContainerUpgrade({
    containerID: QUICK_ASKS_TEXT_ID,
    containerName: QUICK_ASKS_TEXT_NAME,
    contentOffset: 0,
    contentLength: content.length,
    content,
  });
}
