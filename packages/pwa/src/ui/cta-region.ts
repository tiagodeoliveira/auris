import type { Store } from "../store";
import type { Intent } from "../types";

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
        button("📍 Moment", "btn-ghost", actions.markMoment),
        button("Pause", "btn-ghost", actions.pauseMeeting),
        stopButton(actions.stopMeeting),
      );
      wrap.style.display = "flex";
      return;
    }

    if (s.meetingState === "paused") {
      wrap.append(
        button("Resume", "btn-primary", actions.resumeMeeting),
        stopButton(actions.stopMeeting),
      );
      wrap.style.display = "flex";
      return;
    }

    // idle state: compose-region handles this; we render nothing.
    wrap.style.display = "none";
  }

  function stopButton(onConfirm: () => void): HTMLButtonElement {
    const btn = button("Stop", "btn-danger", () => {
      const now = Date.now();
      if (now < stopArmedUntil) {
        stopArmedUntil = 0;
        onConfirm();
        render();
      } else {
        stopArmedUntil = now + STOP_CONFIRM_WINDOW_MS;
        btn.textContent = "Tap again to confirm";
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

  function button(
    text: string,
    variant: "btn-ghost" | "btn-primary" | "btn-danger",
    onClick: () => void,
  ): HTMLButtonElement {
    const b = document.createElement("button");
    b.className = variant;
    b.textContent = text;
    b.addEventListener("click", onClick);
    return b;
  }

  render();
  store.subscribe((s) => s.meetingState, render);
  store.subscribe((s) => s.glassesView, render);
  store.subscribe((s) => s.listeningTranscript + s.listeningInterim, render);
}
