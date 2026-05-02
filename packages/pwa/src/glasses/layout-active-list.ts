import {
  TextContainerProperty,
  RebuildPageContainer,
  TextContainerUpgrade,
} from "@evenrealities/even_hub_sdk";
import type { AppState } from "../types";
import { formatActiveListBody } from "./format-active-list";

const HEADER_ID = 1;
const BODY_ID = 2;
const HEADER_NAME = "header";
const BODY_NAME = "body";

export const LINES_PER_SCREEN = 5; // Phase 0 placeholder; recalibrate Phase 1
export const CHARS_PER_LINE = 60;

export function buildActiveListLayout(state: AppState) {
  const header = new TextContainerProperty({
    xPosition: 0,
    yPosition: 0,
    width: 576,
    height: 32,
    borderWidth: 0,
    paddingLength: 4,
    containerID: HEADER_ID,
    containerName: HEADER_NAME,
    content: buildHeader(state),
    isEventCapture: 0,
  });

  const body = new TextContainerProperty({
    xPosition: 0,
    yPosition: 32,
    width: 576,
    height: 256,
    borderWidth: 0,
    paddingLength: 8,
    containerID: BODY_ID,
    containerName: BODY_NAME,
    content: buildBody(state),
    isEventCapture: 1,
  });

  return new RebuildPageContainer({
    containerTotalNum: 2,
    textObject: [header, body],
  });
}

export function buildBodyUpgrade(state: AppState) {
  const content = buildBody(state);
  return new TextContainerUpgrade({
    containerID: BODY_ID,
    containerName: BODY_NAME,
    contentOffset: 0,
    contentLength: content.length,
    content,
  });
}

export function buildHeaderUpgrade(state: AppState) {
  const content = buildHeader(state);
  return new TextContainerUpgrade({
    containerID: HEADER_ID,
    containerName: HEADER_NAME,
    contentOffset: 0,
    contentLength: content.length,
    content,
  });
}

function buildHeader(state: AppState): string {
  const mode = state.availableModes.find((m) => m.id === state.currentMode);
  const label = mode?.label ?? state.currentMode;
  const tag = state.displayTag ? `  ${state.displayTag}` : "";
  return `⌁ ${label}${tag}`;
}

function buildBody(state: AppState): string {
  return formatActiveListBody(
    state.items,
    state.highlightIndex,
    state.viewportStart,
    LINES_PER_SCREEN,
    CHARS_PER_LINE,
  );
}
