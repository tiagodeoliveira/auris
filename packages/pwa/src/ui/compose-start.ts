//! Idle-state Start Meeting button. Lives in its own component so the
//! mount order can place it below the metadata (kv-editor) strip while
//! keeping kv-editor visible in both idle and active states.

import type { Store } from "../store";
import type { CtaActions } from "./cta-region";

export function mountComposeStart(parent: HTMLElement, store: Store, actions: CtaActions): void {
  const wrap = document.createElement("section");
  wrap.className = "compose-start-wrap";
  parent.appendChild(wrap);

  const btn = document.createElement("button");
  btn.className = "btn-primary compose-start";
  btn.textContent = "Start Meeting";
  btn.addEventListener("click", () => {
    const desc = store.get().composeDescription.trim();
    actions.startMeeting(desc);
  });
  wrap.appendChild(btn);

  function syncVisibility() {
    wrap.style.display = store.get().meetingState === "idle" ? "flex" : "none";
  }
  syncVisibility();
  store.subscribe((s) => s.meetingState, syncVisibility);
}
