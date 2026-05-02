import {
  TextContainerProperty,
  CreateStartUpPageContainer,
  RebuildPageContainer,
} from "@evenrealities/even_hub_sdk";

const HEADER_ID = 1;
const BODY_ID = 2;

function buildContainers(): TextContainerProperty[] {
  const header = new TextContainerProperty({
    xPosition: 0,
    yPosition: 0,
    width: 576,
    height: 32,
    borderWidth: 0,
    paddingLength: 4,
    containerID: HEADER_ID,
    containerName: "header",
    content: "⌁ Ready",
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
    containerName: "body",
    content: "Tap 'Describe meeting' or 'Start meeting' on phone",
    isEventCapture: 1,
  });

  return [header, body];
}

export function buildIdleLayout(): CreateStartUpPageContainer {
  return new CreateStartUpPageContainer({
    containerTotalNum: 2,
    textObject: buildContainers(),
  });
}

export function buildIdleRebuild(): RebuildPageContainer {
  return new RebuildPageContainer({
    containerTotalNum: 2,
    textObject: buildContainers(),
  });
}
