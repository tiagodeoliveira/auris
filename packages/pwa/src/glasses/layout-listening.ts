import {
  TextContainerProperty,
  RebuildPageContainer,
  TextContainerUpgrade,
} from "@evenrealities/even_hub_sdk";
import type { AppState } from "../types";

const HEADER_ID = 1;
const BODY_ID = 2;
const BODY_NAME = "body";
const MAX_BODY_CHARS = 600;

export function buildListeningLayout(state: AppState) {
  const header = new TextContainerProperty({
    xPosition: 0,
    yPosition: 0,
    width: 576,
    height: 32,
    borderWidth: 0,
    paddingLength: 4,
    containerID: HEADER_ID,
    containerName: "header",
    content: "⌁ Listening…  ●",
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
  return new RebuildPageContainer({ containerTotalNum: 2, textObject: [header, body] });
}

export function buildListeningBodyUpgrade(state: AppState) {
  const content = buildBody(state);
  return new TextContainerUpgrade({
    containerID: BODY_ID,
    containerName: BODY_NAME,
    contentOffset: 0,
    contentLength: content.length,
    content,
  });
}

function buildBody(state: AppState): string {
  const full = state.listeningTranscript + state.listeningInterim;
  if (full.length <= MAX_BODY_CHARS) return full;
  return "…" + full.slice(full.length - MAX_BODY_CHARS + 1);
}
