import type { Store } from "../store";
import type { Intent } from "../types";
import type { AuthBundle } from "../auth";
import { ArtifactsApi } from "../artifacts-api";
import { SERVER_URL } from "../server-url";
import { pickArtifacts } from "./artifact-picker";

const STOP_CONFIRM_WINDOW_MS = 3000;

export interface CtaActions {
  describeMeeting(): void;
  /// Stops dictation but keeps the listeningTranscript intact so the
  /// user can edit it in the textarea before pressing Start Meeting.
  stopListening(): void;
  /// Sends an extract_metadata intent so the user can review/edit chips
  /// before starting the meeting.
  extractMetadata(description: string): void;
  /// `audioSourceDeviceId` binds the meeting's audio source on the
  /// server. `null` means start a silent meeting (no audio source).
  startMeeting(description: string, audioSourceDeviceId: string | null): void;
  /// Stamp a moment at the current meeting offset. No-op outside an
  /// active meeting (the server validates the same).
  markMoment(): void;
  pauseMeeting(): void;
  resumeMeeting(): void;
  stopMeeting(): void;
  cancelListening(): void;
}

export function mountCtaRegion(
  parent: HTMLElement,
  store: Store,
  _send: (i: Intent) => void,
  actions: CtaActions,
  auth: AuthBundle,
): void {
  const wrap = document.createElement("div");
  wrap.className = "cta-region";
  parent.appendChild(wrap);

  let stopArmedUntil = 0;

  function render() {
    const s = store.get();
    wrap.innerHTML = "";

    // Listening view is rendered inline by compose-region (the textarea
    // live-fills with the Soniox transcript and the mic icon shows active
    // state). cta-region intentionally renders nothing during listening
    // so the bottom action bar doesn't compete with the compose surface.
    if (s.glassesView === "listening") {
      wrap.style.display = "none";
      return;
    }

    if (s.meetingState === "active") {
      wrap.append(
        iconButton("◆", "Moment", "btn-ghost", actions.markMoment),
        iconButton("📎", "Attach", "btn-ghost", openAttachPicker),
        iconButton("⏸", "Pause", "btn-ghost", actions.pauseMeeting),
        stopButton(actions.stopMeeting),
      );
      wrap.style.display = "flex";
      return;
    }

    if (s.meetingState === "paused") {
      wrap.append(
        iconButton("▶", "Resume", "btn-primary", actions.resumeMeeting),
        stopButton(actions.stopMeeting),
      );
      wrap.style.display = "flex";
      return;
    }

    // idle state: compose-region handles this; we render nothing.
    wrap.style.display = "none";
  }

  function stopButton(onConfirm: () => void): HTMLButtonElement {
    const btn = iconButton("⏹", "Stop", "btn-danger", () => {
      const now = Date.now();
      if (now < stopArmedUntil) {
        stopArmedUntil = 0;
        onConfirm();
        render();
      } else {
        stopArmedUntil = now + STOP_CONFIRM_WINDOW_MS;
        btn.classList.add("armed");
        btn.innerHTML = `<span class="cta-btn-label">Confirm?</span>`;
        setTimeout(() => {
          if (Date.now() >= stopArmedUntil) {
            stopArmedUntil = 0;
            render();
          }
        }, STOP_CONFIRM_WINDOW_MS + 100);
      }
    });
    return btn;
  }

  /// Build a CTA button with an icon glyph + a label below. Compact
  /// vertical stack so all three (Moment / Pause / Stop) fit in one
  /// row on narrow viewports without wrapping.
  function iconButton(
    icon: string,
    label: string,
    variant: "btn-ghost" | "btn-primary" | "btn-danger",
    onClick: () => void,
  ): HTMLButtonElement {
    const b = document.createElement("button");
    b.className = `${variant} cta-btn`;
    b.innerHTML = `<span class="cta-btn-icon">${icon}</span><span class="cta-btn-label">${label}</span>`;
    b.addEventListener("click", onClick);
    return b;
  }

  /// Mid-meeting attach. Opens the same picker the compose flow
  /// uses, pre-checking whatever's already attached to the running
  /// meeting. On confirm, fires `api.attach` per selected id and
  /// updates `attachedArtifactIds` as each succeeds. Server is
  /// idempotent, so re-confirming the same set is a no-op.
  async function openAttachPicker(): Promise<void> {
    const s = store.get();
    if (!s.currentMeetingId) return;
    const picked = await pickArtifacts({
      alreadySelectedIds: s.attachedArtifactIds,
      auth,
    });
    if (picked === null) return;
    const meetingId = s.currentMeetingId;
    const api = ArtifactsApi.from(SERVER_URL, () => auth.getAccessToken());
    if (!api) return;
    for (const a of picked) {
      try {
        await api.attach(meetingId, a.id);
        store.update({
          attachedArtifactIds: [...store.get().attachedArtifactIds.filter((x) => x !== a.id), a.id],
        });
        console.log(`[artifacts] attached ${a.id} to meeting ${meetingId}`);
      } catch (e) {
        console.warn(`[artifacts] attach ${a.id} failed:`, e);
      }
    }
  }

  render();
  store.subscribe((s) => s.meetingState, render);
  store.subscribe((s) => s.glassesView, render);
  store.subscribe((s) => s.listeningTranscript + s.listeningInterim, render);
}
