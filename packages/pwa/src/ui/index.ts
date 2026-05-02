import type { Store } from "../store";
import { mountStatusBar } from "./status-bar";

export function mountUI(root: HTMLElement, store: Store): void {
  mountStatusBar(root, store, () => store.update({ settingsModalOpen: true }));
  // Subsequent tasks add: mode dropdown, KV editor, CTA region, items mirror,
  // settings modal, toasts, error overlay.
  const placeholder = document.createElement("div");
  placeholder.style.padding = "16px";
  placeholder.textContent = "(UI under construction)";
  root.appendChild(placeholder);
}
