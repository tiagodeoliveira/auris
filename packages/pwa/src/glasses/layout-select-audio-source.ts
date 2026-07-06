//! Pre-meeting audio-source picker.
//!
//! Sits between "Start meeting" on the idle screen and the actual
//! meeting active view. Renders the user's registered audio-capable
//! devices as a list; clicking one fires `start_meeting` with the
//! chosen `audio_source_device_id`. A "Silent (no audio)" option
//! pinned at the end lets the user start a meeting without binding
//! a capture source (the server treats `audio_source_device_id =
//! undefined` as "no source bound" — see the Intent doc).
//!
//! Layout (576 × 288 canvas):
//!   - Title (0, 0)..(576, 28)         — "Select audio source"
//!   - List  (0, 32)..(576, 280)       — device hostnames + the
//!     trailing "Silent" option. Owns `isEventCapture: 1`; click
//!     routes by `currentSelectItemIndex`.

import {
  ListContainerProperty,
  ListItemContainerProperty,
  RebuildPageContainer,
  TextContainerProperty,
} from "@evenrealities/even_hub_sdk";
import type { Device } from "../contract";
import type { AppState } from "../types";

export const AUDIO_LIST_CONTAINER_ID = 2;
export const AUDIO_LIST_CONTAINER_NAME = "audio";
const TITLE_CONTAINER_ID = 1;
const TITLE_CONTAINER_NAME = "title";

/// Sentinel index for the "Silent" choice — appended after the
/// real devices. The gesture router treats this index as
/// "start_meeting with audio_source_device_id omitted".
export const AUDIO_ITEM_SILENT = "__silent__";

/// Returns the ordered list of audio-source options the user can
/// pick from on the glasses. Devices come first (filtered to
/// audio_capture capability, the same filter the phone-side
/// `compose-audio-source.ts` uses), followed by the "Silent" entry.
///
/// Each entry pairs a `key` (used to resolve the click target —
/// either a real `device.id` or the silent sentinel) with a `label`
/// for the firmware list.
export interface AudioSourceOption {
  key: string;
  label: string;
}

export function audioSourceOptions(state: AppState): AudioSourceOption[] {
  const devices: Device[] = state.availableDevices.filter((d) =>
    d.capabilities.includes("audio_capture"),
  );
  return [
    ...devices.map((d) => ({ key: d.id, label: d.hostname })),
    { key: AUDIO_ITEM_SILENT, label: "Silent (no audio)" },
  ];
}

export function buildSelectAudioSourceLayout(state: AppState): RebuildPageContainer {
  const options = audioSourceOptions(state);
  const title = new TextContainerProperty({
    xPosition: 0,
    yPosition: 0,
    width: 576,
    height: 28,
    borderWidth: 0,
    paddingLength: 4,
    containerID: TITLE_CONTAINER_ID,
    containerName: TITLE_CONTAINER_NAME,
    content: "Select audio source",
    isEventCapture: 0,
  });
  const list = new ListContainerProperty({
    xPosition: 0,
    yPosition: 32,
    width: 576,
    height: 248,
    borderWidth: 0,
    paddingLength: 8,
    containerID: AUDIO_LIST_CONTAINER_ID,
    containerName: AUDIO_LIST_CONTAINER_NAME,
    isEventCapture: 1,
    itemContainer: new ListItemContainerProperty({
      itemCount: options.length,
      itemWidth: 0,
      isItemSelectBorderEn: 1,
      itemName: options.map((o) => o.label),
    }),
  });
  return new RebuildPageContainer({
    containerTotalNum: 2,
    textObject: [title],
    listObject: [list],
  });
}
