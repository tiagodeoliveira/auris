//! Idle-state composition surface.
//! See `docs/specs/pwa-ux-redesign.md` §3.2.
//!
//! Title + multiline description input (with embedded mic toggle for the
//! Soniox voice flow) + rust-gradient Start button. The metadata strip is
//! rendered separately by mountKvEditor at the top level so it's visible
//! in both idle and active states.

import type { Store } from "../store";
import type { CtaActions } from "./cta-region";

export function mountComposeRegion(parent: HTMLElement, store: Store, actions: CtaActions): void {
  const wrap = document.createElement("section");
  wrap.className = "compose";
  parent.appendChild(wrap);

  const title = document.createElement("h1");
  title.className = "compose-title";
  title.textContent = "New Meeting";
  wrap.appendChild(title);

  const inputArea = document.createElement("div");
  inputArea.className = "compose-input-area";
  wrap.appendChild(inputArea);

  const textarea = document.createElement("textarea");
  textarea.placeholder = "What's this meeting about?";
  textarea.rows = 3;
  inputArea.appendChild(textarea);

  const mic = document.createElement("button");
  mic.className = "compose-mic";
  mic.setAttribute("aria-label", "Toggle voice input");
  mic.innerHTML = "🎤";
  mic.addEventListener("click", () => actions.describeMeeting());
  inputArea.appendChild(mic);

  const startBtn = document.createElement("button");
  startBtn.className = "btn-primary compose-start";
  startBtn.textContent = "Start Meeting";
  startBtn.addEventListener("click", () => actions.startMeeting(textarea.value.trim()));
  wrap.appendChild(startBtn);

  // When the listening flow runs, populate the textarea with the live
  // transcript. After commit, the listening reducer puts the final text
  // back into listeningTranscript; we copy it into the textarea so the
  // user can review/edit before pressing Start.
  store.subscribe(
    (s) => `${s.glassesView}|${s.listeningTranscript}|${s.listeningInterim}`,
    () => {
      const s = store.get();
      if (s.glassesView === "listening") {
        textarea.value = s.listeningTranscript + s.listeningInterim;
        mic.classList.add("active");
      } else {
        mic.classList.remove("active");
        // After listening commits, the listeningTranscript holds the final
        // text; sync it into the textarea so Start button sends the right thing.
        if (s.listeningTranscript && !textarea.value) {
          textarea.value = s.listeningTranscript;
        }
      }
    },
  );

  // Self-hide in non-idle states.
  function syncVisibility() {
    wrap.style.display = store.get().meetingState === "idle" ? "flex" : "none";
  }
  syncVisibility();
  store.subscribe((s) => s.meetingState, syncVisibility);
}
