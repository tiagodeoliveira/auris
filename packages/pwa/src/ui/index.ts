import type { Store } from "../store";
import type { Intent } from "../types";
import { mountStatusBar } from "./status-bar";
import { mountModeDropdown } from "./mode-dropdown";
import { mountKvEditor } from "./kv-editor";
import { mountCtaRegion, type CtaActions } from "./cta-region";
import { mountItemsMirror } from "./items-mirror";

export interface UiContext {
  store: Store;
  send: (intent: Intent) => void;
  actions: CtaActions;
}

export function mountUI(root: HTMLElement, ctx: UiContext): void {
  mountStatusBar(root, ctx.store, () => ctx.store.update({ settingsModalOpen: true }));
  mountModeDropdown(root, ctx.store, ctx.send);
  mountKvEditor(root, ctx.store, ctx.send);
  mountCtaRegion(root, ctx.store, ctx.send, ctx.actions);
  mountItemsMirror(root, ctx.store);
  // Settings modal + toasts + error overlay — Task 17
}
