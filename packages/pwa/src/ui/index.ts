import type { Store } from "../store";
import type { Intent } from "../types";
import { mountStatusBar } from "./status-bar";
import { mountModeDropdown } from "./mode-dropdown";
import { mountKvEditor } from "./kv-editor";
import { mountItemsMirror } from "./items-mirror";

export interface UiContext {
  store: Store;
  send: (intent: Intent) => void;
}

export function mountUI(root: HTMLElement, ctx: UiContext): void {
  mountStatusBar(root, ctx.store, () => ctx.store.update({ settingsModalOpen: true }));
  mountModeDropdown(root, ctx.store, ctx.send);
  mountKvEditor(root, ctx.store, ctx.send);
  // CTA region — Task 16
  mountItemsMirror(root, ctx.store);
  // Settings modal + toasts + error overlay — Task 17
}
