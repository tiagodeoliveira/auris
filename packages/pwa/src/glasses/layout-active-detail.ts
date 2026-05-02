import {
  TextContainerProperty,
  RebuildPageContainer,
  TextContainerUpgrade,
} from "@evenrealities/even_hub_sdk";
import type { AppState } from "../types";

const HEADER_ID = 1;
const BODY_ID = 2;
const BODY_NAME = "body";

export function buildActiveDetailLayout(state: AppState) {
  const item = findDetailItem(state);
  const header = new TextContainerProperty({
    xPosition: 0,
    yPosition: 0,
    width: 576,
    height: 32,
    borderWidth: 0,
    paddingLength: 4,
    containerID: HEADER_ID,
    containerName: "header",
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
    content: buildBody(item),
    isEventCapture: 1,
  });

  return new RebuildPageContainer({ containerTotalNum: 2, textObject: [header, body] });
}

export function buildDetailBodyUpgrade(state: AppState) {
  const content = buildBody(findDetailItem(state));
  return new TextContainerUpgrade({
    containerID: BODY_ID,
    containerName: BODY_NAME,
    contentOffset: 0,
    contentLength: content.length,
    content,
  });
}

function findDetailItem(state: AppState) {
  return state.detailItemId ? state.items.find((i) => i.id === state.detailItemId) : undefined;
}

function buildHeader(state: AppState): string {
  const mode = state.availableModes.find((m) => m.id === state.currentMode);
  return `⌁ ${mode?.label ?? state.currentMode}`;
}

function buildBody(item: ReturnType<typeof findDetailItem>): string {
  if (!item) return "Loading…";
  if (!item.detail) return `${item.text}\n──────────\nLoading…`;
  return `${item.text}\n──────────\n${item.detail}`;
}
