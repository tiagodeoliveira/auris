//! Idle-state composition surface — description card content +
//! attachments card content. Each export populates a content slot
//! the parent (a `compose-card` shell) owns. The card itself
//! provides the title / subtitle / underline; we only render the
//! controls inside.
//!
//! Previously this file owned a single `mountComposeRegion` that
//! emitted title, description, and the two attach strips as flat
//! siblings. The new card-based layout mirrors mobile's
//! `app/(tabs)/index.tsx`: one card per section.

import type { Store } from "../store";
import type { CtaActions } from "./cta-region";
import type { AuthBundle } from "../auth";
import type { Artifact } from "../artifacts-api";
import type { MeetingSummary } from "../meetings-api";
import { pickArtifacts } from "./artifact-picker";
import { pickMeetings } from "./meeting-picker";

function emptyChildren(el: HTMLElement): void {
  while (el.firstChild) el.removeChild(el.firstChild);
}

/// Description textarea + embedded mic toggle. The mic kicks off /
/// stops the Soniox dictation flow; live transcript fills the
/// textarea while the flow runs. Two-way bound to
/// `store.composeDescription` so external resets (after start /
/// stop) flow back into the field.
export function mountComposeDescription(
  parent: HTMLElement,
  store: Store,
  actions: CtaActions,
): void {
  const inputArea = document.createElement("div");
  inputArea.className = "compose-input-area";
  parent.appendChild(inputArea);

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
    // Toggle: if currently listening, stop dictation but keep the
    // transcript so the user can edit it. Otherwise start a fresh
    // dictation session.
    if (store.get().glassesView === "listening") {
      actions.stopListening();
    } else {
      actions.describeMeeting();
    }
  });
  inputArea.appendChild(mic);

  // Bind the live transcript into the textarea while dictation runs.
  // After commit, mirror the final value into the store-backed
  // description so Start sends the right thing.
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

  // External reset (e.g. start_meeting commits → idle on stop)
  // flows back into the textarea.
  store.subscribe(
    (s) => s.composeDescription,
    () => {
      const s = store.get();
      if (s.composeDescription !== textarea.value) {
        textarea.value = s.composeDescription;
      }
    },
  );
}

/// Attachments card content — MEETINGS sub-row + ARTIFACTS sub-row.
/// Each sub-row shows currently-staged chips + a `+ Add` affordance
/// that opens the matching picker modal. Staged ids land in
/// `pendingArtifactAttachments` / `pendingAttachedMeetings`; the
/// `ws-handlers.ts` start_meeting drain turns them into real
/// attaches once the meeting becomes active.
export function mountComposeAttachments(parent: HTMLElement, store: Store, auth: AuthBundle): void {
  // MEETINGS sub-section.
  const meetingsSub = document.createElement("div");
  meetingsSub.className = "compose-subsection";
  const meetingsLabel = document.createElement("p");
  meetingsLabel.className = "compose-subsection-label";
  meetingsLabel.textContent = "Meetings";
  const meetingRow = document.createElement("div");
  meetingRow.className = "compose-artifacts-row";
  meetingsSub.append(meetingsLabel, meetingRow);
  parent.appendChild(meetingsSub);

  let stagedMeetings: MeetingSummary[] = [];

  function meetingChipLabel(m: MeetingSummary): string {
    const desc = (m.description ?? "").trim();
    if (desc) return desc.length > 40 ? desc.slice(0, 37) + "…" : desc;
    const t = m.metadata?.title;
    return t && t.trim() ? t.trim() : "Meeting";
  }

  function renderMeetingStrip(): void {
    emptyChildren(meetingRow);
    for (const m of stagedMeetings) {
      const chip = document.createElement("span");
      chip.className = "compose-artifact-chip";
      const name = document.createElement("span");
      name.className = "compose-artifact-chip-name";
      name.textContent = meetingChipLabel(m);
      chip.appendChild(name);
      const x = document.createElement("button");
      x.className = "compose-artifact-chip-x";
      x.setAttribute("aria-label", "Remove");
      x.title = "Remove";
      x.textContent = "×";
      x.addEventListener("click", () => {
        stagedMeetings = stagedMeetings.filter((y) => y.id !== m.id);
        store.update({ pendingAttachedMeetings: stagedMeetings.map((y) => y.id) });
        renderMeetingStrip();
      });
      chip.append(x);
      meetingRow.appendChild(chip);
    }
    const addBtn = document.createElement("button");
    addBtn.className = "compose-artifact-add";
    addBtn.type = "button";
    addBtn.textContent = stagedMeetings.length === 0 ? "+ Attach meeting" : "+ Add";
    addBtn.addEventListener("click", () => {
      void (async () => {
        const picked = await pickMeetings({
          alreadySelectedIds: stagedMeetings.map((m) => m.id),
          auth,
        });
        if (picked === null) return;
        stagedMeetings = picked;
        store.update({ pendingAttachedMeetings: picked.map((p) => p.id) });
        renderMeetingStrip();
      })();
    });
    meetingRow.appendChild(addBtn);
  }
  renderMeetingStrip();

  // ARTIFACTS sub-section.
  const artifactsSub = document.createElement("div");
  artifactsSub.className = "compose-subsection";
  const artifactsLabel = document.createElement("p");
  artifactsLabel.className = "compose-subsection-label";
  artifactsLabel.textContent = "Artifacts";
  const artifactRow = document.createElement("div");
  artifactRow.className = "compose-artifacts-row";
  artifactsSub.append(artifactsLabel, artifactRow);
  parent.appendChild(artifactsSub);

  let stagedArtifacts: Artifact[] = [];

  function renderArtifactStrip(): void {
    emptyChildren(artifactRow);
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

  // Reset on meeting transitions to idle (ws-handler clears
  // pending* arrays; the local chip caches need their own reset).
  store.subscribe(
    (s) => s.meetingState,
    () => {
      const s = store.get();
      if (s.meetingState === "idle" && s.pendingArtifactAttachments.length === 0) {
        if (stagedArtifacts.length > 0) {
          stagedArtifacts = [];
          renderArtifactStrip();
        }
      }
      if (s.meetingState === "idle" && s.pendingAttachedMeetings.length === 0) {
        if (stagedMeetings.length > 0) {
          stagedMeetings = [];
          renderMeetingStrip();
        }
      }
    },
  );
}
