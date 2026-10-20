//! Native long-press contextual menu (Even Hub SDK 0.0.14+) for an
//! active meeting. Replaces the old in-cycle "> Stop" screen: the
//! wearer long-presses, scrolls to "End meeting" and selects it. That
//! long-press + deliberate select is the confirmation, so no second
//! tap is needed. The menu must be re-sent on every rebuild that
//! should keep it (omitting `menuObject` clears it).
import { MenuContainerProperty, MenuItemProperty } from "@evenrealities/even_hub_sdk";
import type { AppState } from "../types";

export const MENU_END_MEETING_ID = 1;

export function meetingMenu(state: AppState): MenuContainerProperty | undefined {
  if (state.meetingState !== "active") return undefined;
  return new MenuContainerProperty({
    menuItems: [new MenuItemProperty({ itemName: "End meeting", itemID: MENU_END_MEETING_ID })],
  });
}
