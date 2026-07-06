//! Entry layout — the post-pair landing screen. Two-item menu:
//!   > Start meeting
//!     List meetings
//!
//! The firmware draws the `>` cursor + selection border automatically
//! when `isItemSelectBorderEn: 1` (single-item highlight, scroll to
//! move). Single tap fires `list_event` with the highlighted index;
//! the gesture router dispatches to the matching action.
//!
//! `List meetings` is a stub for now — selecting it is a no-op until
//! the glasses-side meeting history surface lands.

import {
  CreateStartUpPageContainer,
  ListContainerProperty,
  ListItemContainerProperty,
  RebuildPageContainer,
} from "@evenrealities/even_hub_sdk";

export const ENTRY_LIST_CONTAINER_ID = 1;
export const ENTRY_LIST_CONTAINER_NAME = "entry";

/// Item indices reported back via `list_event.currentSelectItemIndex`.
/// Mirrored on the gesture-router side.
export const ENTRY_ITEM_START = 0;
export const ENTRY_ITEM_LIST_MEETINGS = 1;

const ITEMS = ["Start meeting", "List meetings"];

function entryListContainer(): ListContainerProperty {
  return new ListContainerProperty({
    xPosition: 0,
    yPosition: 0,
    width: 576,
    height: 288,
    borderWidth: 0,
    paddingLength: 12,
    containerID: ENTRY_LIST_CONTAINER_ID,
    containerName: ENTRY_LIST_CONTAINER_NAME,
    isEventCapture: 1,
    itemContainer: new ListItemContainerProperty({
      itemCount: ITEMS.length,
      itemWidth: 0,
      isItemSelectBorderEn: 1,
      itemName: ITEMS,
    }),
  });
}

export function buildEntryLayout(): CreateStartUpPageContainer {
  return new CreateStartUpPageContainer({
    containerTotalNum: 1,
    listObject: [entryListContainer()],
  });
}

export function buildEntryRebuild(): RebuildPageContainer {
  return new RebuildPageContainer({
    containerTotalNum: 1,
    listObject: [entryListContainer()],
  });
}
