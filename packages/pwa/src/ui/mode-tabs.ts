//! Segmented control for switching between modes.

import type { Store } from "../store";
import type { Intent } from "../types";

const SHORT_LABELS: Record<string, string> = {
  transcript: "TRANSCRIPT",
  highlights: "HIGHLIGHTS",
  actions: "ACTIONS",
  open_questions: "QUESTIONS",
};

export function mountModeTabs(parent: HTMLElement, store: Store, send: (i: Intent) => void): void {
  const wrap = document.createElement("nav");
  wrap.className = "mode-tabs";
  parent.appendChild(wrap);

  function render() {
    const s = store.get();
    if (s.meetingState !== "active" && s.meetingState !== "paused") {
      wrap.style.display = "none";
      return;
    }
    wrap.style.display = "flex";
    wrap.innerHTML = "";
    for (const m of s.availableModes) {
      const tab = document.createElement("button");
      tab.className = "mode-tab" + (s.currentMode === m.id ? " active" : "");
      tab.textContent = SHORT_LABELS[m.id] ?? m.label.toUpperCase();
      tab.addEventListener("click", () => {
        if (store.get().currentMode !== m.id) {
          store.update({ currentMode: m.id });
          send({ type: "set_mode", mode: m.id });
        }
      });
      wrap.appendChild(tab);
    }
  }

  render();
  store.subscribe(
    (s) => `${s.meetingState}|${s.currentMode}|${s.availableModes.map((m) => m.id).join(",")}`,
    render,
  );
}
