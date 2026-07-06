//! Segmented control for switching between modes.

import type { Store } from "../store";

const SHORT_LABELS: Record<string, string> = {
  assist: "ASSIST",
  transcript: "TRANSCRIPT",
  highlights: "HIGHLIGHTS",
  actions: "ACTIONS",
  open_questions: "QUESTIONS",
  summary: "SUMMARY",
};

export function mountModeTabs(parent: HTMLElement, store: Store): void {
  const wrap = document.createElement("nav");
  wrap.className = "mode-tabs";
  parent.appendChild(wrap);

  function render() {
    const s = store.get();
    if (s.meetingState !== "active") {
      wrap.style.display = "none";
      return;
    }
    wrap.style.display = "flex";
    wrap.innerHTML = "";
    // Quick asks are a glasses-only mode — on the browser PWA + Mac
    // + mobile, the same prompts surface as a chip row above the
    // chat input instead of as a separate tab. The mode still
    // exists in `availableModes` (so the chip row + glasses cycle
    // can find it), just hidden from the tab picker here.
    const tabsModes = s.availableModes.filter((m) => m.id !== "quick_asks");
    for (const m of tabsModes) {
      const tab = document.createElement("button");
      tab.className = "mode-tab" + (s.currentMode === m.id ? " active" : "");
      tab.textContent = SHORT_LABELS[m.id] ?? m.label.toUpperCase();
      tab.addEventListener("click", () => {
        if (store.get().currentMode !== m.id) {
          // `currentMode` is per-surface UI state — purely local.
          // No `set_mode` intent fires; the server-side intent is a
          // legacy no-op now that each surface tracks its own view
          // independently.
          store.update({ currentMode: m.id });
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
