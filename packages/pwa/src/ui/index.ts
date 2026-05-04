import type { Store } from "../store";
import type { Intent } from "../types";
import { mountTopBar } from "./top-bar";
import { mountComposeRegion } from "./compose-region";
import { mountModeDropdown } from "./mode-dropdown";
import { mountKvEditor } from "./kv-editor";
import { mountCtaRegion, type CtaActions } from "./cta-region";
import { mountItemsMirror } from "./items-mirror";
import { mountSettingsModal } from "./settings-modal";
import { mountToasts } from "./toast";
import { mountErrorOverlay } from "./error-overlay";

export interface UiContext {
  store: Store;
  send: (intent: Intent) => void;
  actions: CtaActions;
  bridge: {
    setLocalStorage(k: string, v: string): Promise<boolean>;
    getLocalStorage(k: string): Promise<string>;
  };
  reconnect: () => void;
}

export function mountUI(root: HTMLElement, ctx: UiContext): void {
  mountTopBar(root, ctx.store, () => ctx.store.update({ settingsModalOpen: true }));
  mountComposeRegion(root, ctx.store, ctx.actions); // NEW — idle state surface
  mountModeDropdown(root, ctx.store, ctx.send);
  mountKvEditor(root, ctx.store, ctx.send);
  mountCtaRegion(root, ctx.store, ctx.send, ctx.actions);
  mountItemsMirror(root, ctx.store);
  mountSettingsModal(root, ctx.store, ctx.bridge, ctx.reconnect);
  mountToasts(root, ctx.store);
  mountErrorOverlay(root, ctx.store);
}
