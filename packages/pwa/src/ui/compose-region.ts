//! Idle-state composition surface (input + mic toggle + Extract Tags).
//!
//! Title + multiline description input with embedded mic toggle for the
//! Soniox voice flow + an Extract Tags affordance. The Start button
//! lives in a separate component (mountComposeStart) so it can render
//! below the metadata strip. The metadata strip is rendered separately
//! by mountKvEditor at the top level so it's visible in both idle and
//! active states.

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
  textarea.value = store.get().composeDescription;
  textarea.addEventListener("input", () => {
    store.update({ composeDescription: textarea.value });
  });
  inputArea.appendChild(textarea);

  const mic = document.createElement("button");
  mic.className = "compose-mic";
  mic.setAttribute("aria-label", "Toggle voice input");
  mic.textContent = "●";
  mic.addEventListener("click", () => {
    // Toggle: if currently listening, stop dictation but keep the transcript
    // so the user can edit it. Otherwise start a new dictation session.
    if (store.get().glassesView === "listening") {
      actions.stopListening();
    } else {
      actions.describeMeeting();
    }
  });
  inputArea.appendChild(mic);

  // Extract Tags affordance — runs metadata extraction without starting the
  // meeting so the user can review/edit the chips first. Cmd/Ctrl+Enter on
  // the textarea is the keyboard shortcut.
  const extractRow = document.createElement("div");
  extractRow.className = "compose-extract-row";
  const extractBtn = document.createElement("button");
  extractBtn.type = "button";
  extractBtn.className = "compose-extract";
  extractBtn.textContent = "▸ EXTRACT TAGS";
  function triggerExtract() {
    const s = store.get();
    const desc = s.composeDescription.trim();
    if (!desc || s.extractingMetadata) return;
    actions.extractMetadata(desc);
  }
  extractBtn.addEventListener("click", triggerExtract);
  extractRow.appendChild(extractBtn);
  wrap.appendChild(extractRow);

  textarea.addEventListener("keydown", (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      triggerExtract();
    }
  });

  // Reflect extracting state + empty-description visibility on the
  // button. We *hide* (not just disable) the row when the description
  // is empty — a greyed-out affordance with no clear path to enable
  // it adds visual noise. Reads from composeDescription so dictation
  // updates (which set textarea.value programmatically without firing
  // 'input') still flip the button on.
  function syncExtractBtn() {
    const s = store.get();
    const hasDesc = s.composeDescription.trim().length > 0;
    extractRow.style.display = hasDesc ? "flex" : "none";
    extractBtn.disabled = s.extractingMetadata;
    extractBtn.textContent = s.extractingMetadata ? "▸ EXTRACTING…" : "▸ EXTRACT TAGS";
  }
  store.subscribe((s) => `${s.composeDescription}|${s.extractingMetadata}`, syncExtractBtn);
  syncExtractBtn();

  // When the listening flow runs, populate the textarea with the live
  // transcript. After commit, the listening reducer puts the final text
  // back into listeningTranscript; we copy it into the textarea (and the
  // store-backed description) so Start sends the right thing.
  store.subscribe(
    (s) => `${s.glassesView}|${s.listeningTranscript}|${s.listeningInterim}`,
    () => {
      const s = store.get();
      if (s.glassesView === "listening") {
        const live = s.listeningTranscript + s.listeningInterim;
        textarea.value = live;
        store.update({ composeDescription: live });
        mic.classList.add("active");
      } else {
        mic.classList.remove("active");
        if (s.listeningTranscript && !textarea.value) {
          textarea.value = s.listeningTranscript;
          store.update({ composeDescription: s.listeningTranscript });
        }
      }
    },
  );

  // Reset textarea when description is cleared externally (e.g. after
  // start_meeting commits and we drop back to idle on stop).
  store.subscribe(
    (s) => s.composeDescription,
    () => {
      const s = store.get();
      if (s.composeDescription !== textarea.value) {
        textarea.value = s.composeDescription;
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
