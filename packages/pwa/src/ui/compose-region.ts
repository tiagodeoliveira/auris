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
import type { AuthBundle } from "../auth";
import type { Artifact } from "../artifacts-api";
import { pickArtifacts } from "./artifact-picker";

export function mountComposeRegion(
  parent: HTMLElement,
  store: Store,
  actions: CtaActions,
  auth: AuthBundle,
): void {
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

  // Artifact chip strip — staged attachments for the next meeting.
  // Sits below the extract row so the visual flow is description →
  // tags → artifacts → start. Local closure state because nothing
  // else cares about the staged list until Start fires; we hand
  // the ids to the store at submit time.
  const artifactRow = document.createElement("div");
  artifactRow.className = "compose-artifacts-row";
  wrap.appendChild(artifactRow);
  let stagedArtifacts: Artifact[] = [];

  function renderArtifactStrip(): void {
    artifactRow.innerHTML = "";
    for (const a of stagedArtifacts) {
      const chip = document.createElement("span");
      chip.className = "compose-artifact-chip";
      const name = document.createElement("span");
      name.className = "compose-artifact-chip-name";
      name.textContent = a.name;
      chip.appendChild(name);
      const x = document.createElement("button");
      x.className = "compose-artifact-chip-x";
      x.setAttribute("aria-label", "Remove");
      x.title = "Remove";
      x.textContent = "×";
      x.addEventListener("click", () => {
        stagedArtifacts = stagedArtifacts.filter((y) => y.id !== a.id);
        store.update({ pendingArtifactAttachments: stagedArtifacts.map((y) => y.id) });
        renderArtifactStrip();
      });
      chip.append(x);
      artifactRow.appendChild(chip);
    }
    const addBtn = document.createElement("button");
    addBtn.className = "compose-artifact-add";
    addBtn.type = "button";
    addBtn.textContent = stagedArtifacts.length === 0 ? "+ Attach artifact" : "+ Add";
    addBtn.addEventListener("click", () => {
      void (async () => {
        const picked = await pickArtifacts({
          alreadySelectedIds: stagedArtifacts.map((a) => a.id),
          auth,
        });
        if (picked === null) return;
        stagedArtifacts = picked;
        store.update({ pendingArtifactAttachments: picked.map((p) => p.id) });
        renderArtifactStrip();
      })();
    });
    artifactRow.appendChild(addBtn);
  }
  renderArtifactStrip();

  // Reset on meeting transitions to idle (e.g., after Stop) — the
  // ws-handler clears `pendingArtifactAttachments`, but the local
  // `stagedArtifacts` (which carries names for the chips) needs
  // its own reset hook.
  store.subscribe(
    (s) => s.meetingState,
    () => {
      const s = store.get();
      if (s.meetingState !== "idle" && stagedArtifacts.length > 0) {
        // Meeting started — keep the chips visible during the
        // brief moment before the compose-region self-hides; the
        // next idle resets the staged list.
      } else if (s.meetingState === "idle" && s.pendingArtifactAttachments.length === 0) {
        if (stagedArtifacts.length > 0) {
          stagedArtifacts = [];
          renderArtifactStrip();
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
