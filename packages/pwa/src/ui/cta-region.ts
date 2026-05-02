import type { Store } from "../store";
import type { Intent } from "../types";

const STOP_CONFIRM_WINDOW_MS = 3000;

export interface CtaActions {
  describeMeeting(): void;
  startMeeting(): void;
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
  wrap.style.cssText =
    "padding:16px;border-bottom:1px solid #25252a;display:flex;flex-direction:column;gap:8px;";
  parent.appendChild(wrap);

  let stopArmedUntil = 0;

  function render() {
    const s = store.get();
    wrap.innerHTML = "";

    if (s.glassesView === "listening") {
      const transcript = document.createElement("div");
      transcript.style.cssText =
        "background:var(--bg-elev);padding:12px;border-radius:var(--radius);max-height:150px;overflow-y:auto;font-size:14px;line-height:1.5;min-height:60px;";
      const finalSpan = document.createElement("span");
      finalSpan.textContent = s.listeningTranscript;
      const interimSpan = document.createElement("span");
      interimSpan.textContent = s.listeningInterim;
      interimSpan.style.color = "var(--fg-dim)";
      transcript.append(finalSpan, interimSpan);
      wrap.appendChild(transcript);

      wrap.appendChild(button("Cancel", "secondary", actions.cancelListening));
      wrap.appendChild(button("Commit", "", actions.commitListening));
      return;
    }

    if (s.meetingState === "idle") {
      wrap.appendChild(button("Describe meeting", "secondary", actions.describeMeeting));
      wrap.appendChild(button("Start meeting", "", actions.startMeeting));
      return;
    }

    if (s.meetingState === "active") {
      wrap.appendChild(button("Pause", "secondary", actions.pauseMeeting));
      wrap.appendChild(stopButton(actions.stopMeeting));
      return;
    }

    if (s.meetingState === "paused") {
      wrap.appendChild(button("Resume", "", actions.resumeMeeting));
      wrap.appendChild(stopButton(actions.stopMeeting));
      return;
    }
  }

  function stopButton(onConfirm: () => void): HTMLButtonElement {
    const btn = button("Stop", "danger", () => {
      const now = Date.now();
      if (now < stopArmedUntil) {
        stopArmedUntil = 0;
        onConfirm();
        render();
      } else {
        stopArmedUntil = now + STOP_CONFIRM_WINDOW_MS;
        btn.textContent = "Tap again to confirm";
        btn.style.animation = "pulse 0.4s ease-in-out 2";
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
    variant: "" | "secondary" | "danger",
    onClick: () => void,
  ): HTMLButtonElement {
    const b = document.createElement("button");
    b.className = "cta" + (variant ? " " + variant : "");
    b.textContent = text;
    b.style.width = "100%";
    b.addEventListener("click", onClick);
    return b;
  }

  render();
  store.subscribe((s) => s.meetingState, render);
  store.subscribe((s) => s.glassesView, render);
  store.subscribe((s) => s.listeningTranscript + s.listeningInterim, render);
}
