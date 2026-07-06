//! Glasses display for the unpaired state.
//!
//! Rendered as the *initial* page container at boot time when no
//! tokens are in localStorage. The phone-side webview shows the pair
//! screen; the glasses show this prompt so a user wearing the
//! glasses understands why nothing's happening yet.
//!
//! Layout (576 × 288 canvas):
//!   - Image    (16, 72)..(160, 216) — Auris brand mark, drawn at
//!     boot via `drawAurisMarkPng` and pushed to the SDK with
//!     `updateImageRawData`. Vertically centered on the canvas.
//!   - Heading  (176, 56)..(576, 110) — "Auris" wordmark above the
//!     prompt body, so the right column reads as a small branded
//!     panel rather than just an instruction.
//!   - Prompt   (176, 130)..(576, 270) — the actual pair instruction.
//!     Holds the `isEventCapture: 1` flag (the SDK requires exactly
//!     one container per page have it; image containers don't
//!     support it, so it falls to a text container).
//!
//! After the user pairs, main.ts rebuilds the page to
//! `buildEntryRebuild()` and normal rendering takes over.

import {
  CreateStartUpPageContainer,
  ImageContainerProperty,
  RebuildPageContainer,
  TextContainerProperty,
} from "@evenrealities/even_hub_sdk";

/// Square edge of the brand-mark image, in pixels. 144 sits right at
/// the SDK's image-height ceiling. Exported so the post-create
/// `updateImageRawData` call can match.
export const LOGO_SIZE = 144;

/// Numeric ID used by `updateImageRawData` to target the logo
/// container. The bridge protocol routes on ID and validates name;
/// pass both at call sites for parity with the SDK's example.
export const LOGO_CONTAINER_ID = 1;
const HEADING_CONTAINER_ID = 2;
const PROMPT_CONTAINER_ID = 3;

/// Human-readable name used by `updateImageRawData` to target this
/// container. Must match the `containerName` in the property below.
export const LOGO_CONTAINER_NAME = "logo";
const HEADING_CONTAINER_NAME = "heading";
const PROMPT_CONTAINER_NAME = "prompt";

const LOGO_X = 16;
const LOGO_Y = (288 - LOGO_SIZE) / 2; // 72

const RIGHT_COL_X = 176;
const RIGHT_COL_WIDTH = 576 - RIGHT_COL_X;

const HEADING_Y = 56;
const HEADING_HEIGHT = 54;

const PROMPT_Y = 130;
const PROMPT_HEIGHT = 140;

function unpairedContainers() {
  return {
    containerTotalNum: 3,
    imageObject: [
      new ImageContainerProperty({
        xPosition: LOGO_X,
        yPosition: LOGO_Y,
        width: LOGO_SIZE,
        height: LOGO_SIZE,
        containerID: LOGO_CONTAINER_ID,
        containerName: LOGO_CONTAINER_NAME,
      }),
    ],
    textObject: [
      new TextContainerProperty({
        xPosition: RIGHT_COL_X,
        yPosition: HEADING_Y,
        width: RIGHT_COL_WIDTH,
        height: HEADING_HEIGHT,
        borderWidth: 0,
        paddingLength: 0,
        containerID: HEADING_CONTAINER_ID,
        containerName: HEADING_CONTAINER_NAME,
        content: "Auris",
        isEventCapture: 0,
      }),
      new TextContainerProperty({
        xPosition: RIGHT_COL_X,
        yPosition: PROMPT_Y,
        width: RIGHT_COL_WIDTH,
        height: PROMPT_HEIGHT,
        borderWidth: 0,
        paddingLength: 0,
        containerID: PROMPT_CONTAINER_ID,
        containerName: PROMPT_CONTAINER_NAME,
        content: "Pair your device on the Even app to start.",
        // SDK requires exactly one container per page to have
        // `isEventCapture: 1`. Image containers don't support it;
        // the prompt owns the flag.
        isEventCapture: 1,
      }),
    ],
  };
}

export function buildUnpairedLayout(): CreateStartUpPageContainer {
  return new CreateStartUpPageContainer(unpairedContainers());
}

/// `RebuildPageContainer` variant for the re-pair flow — when a
/// session expires mid-run we need to flip the glasses display from
/// whatever was last rendered back to the unpaired prompt. The
/// caller is responsible for re-uploading the logo image data via
/// `updateImageRawData` (image containers are placeholders after
/// rebuild — they don't retain pixels from the previous page).
export function buildUnpairedRebuild(): RebuildPageContainer {
  return new RebuildPageContainer(unpairedContainers());
}
