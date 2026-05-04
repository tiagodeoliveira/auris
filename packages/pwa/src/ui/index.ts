import type { Store } from "../store";
import type { Intent } from "../types";
import { mountTopBar } from "./top-bar";
import { mountComposeRegion } from "./compose-region";
import { mountComposeStart } from "./compose-start";
import { mountHeaderStrip } from "./header-strip";
import { mountKvEditor } from "./kv-editor";
import { mountModeTabs } from "./mode-tabs";
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
  // Always-visible top status row + settings gear.
  mountTopBar(root, ctx.store, () => ctx.store.update({ settingsModalOpen: true }));

  // Idle-state composition surface (self-hides when meeting is active).
  mountComposeRegion(root, ctx.store, ctx.actions);

  // Active-meeting surface — components self-hide outside active/paused.
  mountHeaderStrip(root, ctx.store);
  mountKvEditor(root, ctx.store, ctx.send); // visible in both idle and active
  // Start button sits below the metadata strip in idle so the visual flow
  // is: input → metadata → start. Self-hides outside idle.
  mountComposeStart(root, ctx.store, ctx.actions);
  mountModeTabs(root, ctx.store, ctx.send);
  mountItemsMirror(root, ctx.store);

  // Sticky bottom action bar (Pause/Stop in active, listening UI when listening).
  mountCtaRegion(root, ctx.store, ctx.send, ctx.actions);

  // Overlays.
  mountSettingsModal(root, ctx.store, ctx.bridge, ctx.reconnect);
  mountToasts(root, ctx.store);
  mountErrorOverlay(root, ctx.store);
}
