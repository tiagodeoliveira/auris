//! Three-segment picker for `AssistSensitivity`. Used in two
//! places:
//!   1. Compose screen — picks the value carried into the next
//!      `start_meeting` intent. Local state only; the value is
//!      mirrored in `AppState.assistSensitivity` so a reload picks
//!      it back up.
//!   2. Mid-meeting view — fires `Intent.set_assist_sensitivity`
//!      so the server flips the runtime field + persists the
//!      column + broadcasts to other surfaces.
//!
//! The visual matches the existing segmented patterns in this app:
//! pill-shaped container, three buttons, the active one filled
//! with the coral accent. Compact (~140px wide × 30px tall).

import type { Store } from "../store";
import type { AssistSensitivity } from "../types";
import type { CtaActions } from "./cta-region";

const VALUES: AssistSensitivity[] = ["aggressive", "moderate", "minimal"];
const LABELS: Record<AssistSensitivity, string> = {
  aggressive: "AGGRESSIVE",
  moderate: "MODERATE",
  minimal: "MINIMAL",
};

interface MountOptions {
  /// Called whenever the user picks a different value. Compose
  /// callers update `store.assistSensitivity` locally; in-meeting
  /// callers fire `actions.setAssistSensitivity(value)`. The
  /// picker itself doesn't reach into either — it just notifies.
  onPick(value: AssistSensitivity): void;
}

/// Mounts the segmented picker. Returns nothing — the caller wires
/// state via the store subscription.
export function mountSensitivityPicker(
  parent: HTMLElement,
  store: Store,
  opts: MountOptions,
): void {
  const wrap = document.createElement("div");
  wrap.className = "sensitivity-picker";
  wrap.setAttribute("role", "radiogroup");
  wrap.setAttribute("aria-label", "Assist sensitivity");
  parent.appendChild(wrap);

  const buttons = new Map<AssistSensitivity, HTMLButtonElement>();
  for (const v of VALUES) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "sensitivity-segment";
    b.dataset.value = v;
    b.setAttribute("role", "radio");
    b.textContent = LABELS[v];
    b.addEventListener("click", () => opts.onPick(v));
    wrap.appendChild(b);
    buttons.set(v, b);
  }

  function reflect(): void {
    const current = store.get().assistSensitivity;
    for (const [v, b] of buttons) {
      const active = v === current;
      b.classList.toggle("active", active);
      b.setAttribute("aria-checked", active ? "true" : "false");
    }
  }
  reflect();
  store.subscribe((s) => s.assistSensitivity, reflect);
}

/// Compose-screen wrapper. Self-hides outside idle so the picker
/// only shows while the user is staging a meeting. Updates the
/// store's `assistSensitivity` locally; the value rides on the
/// The previous `mountComposeSensitivity` wrapper is gone — the
/// compose-card shell in `ui/index.ts` now provides the title +
/// subtitle + visibility gate, and the caller invokes
/// `mountSensitivityPicker` directly into the card's content slot.

/// Mid-meeting wrapper. Self-hides outside `meetingState === "active"`
/// AND outside `currentMode === "assist"`, so the control only
/// surfaces when the user is on the assist tab during a live
/// meeting. Picking a value fires `Intent.set_assist_sensitivity`
/// via `actions.setAssistSensitivity` — the server is the source
/// of truth, the local state lands via the broadcast event.
export function mountMidMeetingSensitivity(
  parent: HTMLElement,
  store: Store,
  actions: CtaActions,
): void {
  const wrap = document.createElement("section");
  wrap.className = "compose-sensitivity mid-meeting-sensitivity";

  const label = document.createElement("div");
  label.className = "compose-sensitivity-label";
  label.textContent = "ASSIST SENSITIVITY";
  wrap.appendChild(label);

  parent.appendChild(wrap);

  mountSensitivityPicker(wrap, store, {
    onPick: (value) => actions.setAssistSensitivity(value),
  });

  function syncVisibility() {
    const s = store.get();
    const visible = s.meetingState === "active" && s.currentMode === "assist";
    wrap.style.display = visible ? "flex" : "none";
  }
  syncVisibility();
  store.subscribe((s) => `${s.meetingState}|${s.currentMode}`, syncVisibility);
}
