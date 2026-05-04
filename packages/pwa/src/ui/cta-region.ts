import type { Store } from "../store";
import type { Intent } from "../types";

const STOP_CONFIRM_WINDOW_MS = 3000;

export interface CtaActions {
  describeMeeting(): void;
  startMeeting(description: string): void;
  pauseMeeting(): void;
  resumeMeeting(): void;
  stopMeeting(): void;
  commitListening(): void;
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

    if (s.glassesView === "listening") {
      // Listening UI — task 5 will polish styling. For now, keep the existing
      // shape with class names instead of inline styles.
      const transcript = document.createElement("div");
      transcript.className = "listening-transcript";
      const finalSpan = document.createElement("span");
      finalSpan.textContent = s.listeningTranscript;
      const interimSpan = document.createElement("span");
      interimSpan.className = "interim";
      interimSpan.textContent = s.listeningInterim;
      transcript.append(finalSpan, interimSpan);
      wrap.appendChild(transcript);

      const cancel = button("Cancel", "btn-ghost", actions.cancelListening);
      const commit = button("Commit", "btn-primary", actions.commitListening);
      wrap.append(cancel, commit);
      wrap.style.display = "flex";
      return;
    }

    if (s.meetingState === "active") {
      wrap.append(
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
