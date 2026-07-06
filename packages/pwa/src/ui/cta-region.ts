import type { Store } from "../store";
import type { AssistSensitivity, Intent } from "../types";
import type { AuthBundle } from "../auth";
import { ArtifactsApi } from "../artifacts-api";
import { SERVER_URL } from "../server-url";
import { pickArtifacts } from "./artifact-picker";

const STOP_CONFIRM_WINDOW_MS = 3000;
const SVG_NS = "http://www.w3.org/2000/svg";

export interface CtaActions {
  describeMeeting(): void;
  /// Stops dictation but keeps the listeningTranscript intact so the
  /// user can edit it in the textarea before pressing Start Meeting.
  stopListening(): void;
  /// `audioSourceDeviceId` binds the meeting's audio source on the
  /// server. `null` means start a silent meeting (no audio source).
  startMeeting(description: string, audioSourceDeviceId: string | null): void;
  /// Stamp a moment at the current meeting offset. No-op outside an
  /// active meeting (the server validates the same).
  markMoment(): void;
  stopMeeting(): void;
  cancelListening(): void;
  /// Mid-meeting assist sensitivity toggle. Server validates state
  /// (no-op when idle); the compose-screen picker uses local state
  /// only and passes the value to `startMeeting` instead.
  setAssistSensitivity(value: AssistSensitivity): void;
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
    while (wrap.firstChild) wrap.removeChild(wrap.firstChild);

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
        iconButton(momentIcon(), "Moment", "cta-btn-moment", actions.markMoment),
        iconButton(attachIcon(), "Attach", "cta-btn-attach", openAttachPicker),
        stopButton(actions.stopMeeting),
      );
      wrap.style.display = "flex";
      return;
    }

    // idle state: compose-region handles this; we render nothing.
    wrap.style.display = "none";
  }

  function stopButton(onConfirm: () => void): HTMLButtonElement {
    const btn = iconButton(stopIcon(), "Stop", "cta-btn-stop", () => {
      const now = Date.now();
      if (now < stopArmedUntil) {
        stopArmedUntil = 0;
        onConfirm();
        render();
      } else {
        stopArmedUntil = now + STOP_CONFIRM_WINDOW_MS;
        btn.classList.add("armed");
        // Swap the label to "Confirm?" while armed; icon stays in
        // place so the button width doesn't jump. Re-renders on
        // timeout via the setTimeout below.
        const labelEl = btn.querySelector(".cta-btn-label");
        if (labelEl) labelEl.textContent = "Confirm?";
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

  /// Build a CTA button with an icon (SVG element) + a label below.
  /// Compact vertical stack so the row (Moment / Attach / Stop) fits
  /// on narrow viewports without wrapping. Every CTA uses the same
  /// ghost chrome — variant only differentiates via the `cta-btn-*`
  /// modifier class (used by CSS to tint Stop's label red as the
  /// only destructive cue).
  function iconButton(
    icon: SVGSVGElement,
    label: string,
    variant: "cta-btn-moment" | "cta-btn-attach" | "cta-btn-stop",
    onClick: () => void,
  ): HTMLButtonElement {
    const b = document.createElement("button");
    b.className = `cta-btn ${variant}`;
    const iconWrap = document.createElement("span");
    iconWrap.className = "cta-btn-icon";
    iconWrap.appendChild(icon);
    const labelWrap = document.createElement("span");
    labelWrap.className = "cta-btn-label";
    labelWrap.textContent = label;
    b.append(iconWrap, labelWrap);
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

/// === Icon factories ============================================
///
/// Monochrome line-art SVGs that pick up `currentColor` from the
/// surrounding button text. The shapes are deliberately simple —
/// the EvenHub host renders its own plugin icons as low-fi pixel-
/// like glyphs, and using full-color system emoji here (the old
/// behavior) clashed with that aesthetic. All icons render at
/// ~22px in the CSS, drawn on a 22x22 viewBox so the strokes
/// align to integer pixel positions on a standard DPI display.

function svg(): SVGSVGElement {
  const s = document.createElementNS(SVG_NS, "svg");
  s.setAttribute("viewBox", "0 0 22 22");
  s.setAttribute("aria-hidden", "true");
  return s;
}

/// Diamond / moment marker — a rotated square outline. Echoes the
/// coral moment dot used elsewhere in the app without bringing
/// color into the bottom bar.
function momentIcon(): SVGSVGElement {
  const s = svg();
  const p = document.createElementNS(SVG_NS, "polygon");
  p.setAttribute("points", "11,3 19,11 11,19 3,11");
  p.setAttribute("fill", "none");
  p.setAttribute("stroke", "currentColor");
  p.setAttribute("stroke-width", "2");
  p.setAttribute("stroke-linejoin", "round");
  s.appendChild(p);
  return s;
}

/// Paperclip — single-stroke approximation that reads as a clip
/// at small sizes without needing the full curved hook the system
/// emoji draws.
function attachIcon(): SVGSVGElement {
  const s = svg();
  const path = document.createElementNS(SVG_NS, "path");
  path.setAttribute(
    "d",
    "M14.5 6 L7.5 13 a3 3 0 0 0 4.2 4.2 L18 11 a4.5 4.5 0 0 0 -6.4 -6.4 L5 11",
  );
  path.setAttribute("fill", "none");
  path.setAttribute("stroke", "currentColor");
  path.setAttribute("stroke-width", "1.8");
  path.setAttribute("stroke-linecap", "round");
  path.setAttribute("stroke-linejoin", "round");
  s.appendChild(path);
  return s;
}

/// Stop — filled rounded square. The fill (not stroke) is the
/// universal "stop / end recording" signal; keeping it
/// monochrome means destructive intent reads from the LABEL
/// turning red, not from the icon's color.
function stopIcon(): SVGSVGElement {
  const s = svg();
  const r = document.createElementNS(SVG_NS, "rect");
  r.setAttribute("x", "5");
  r.setAttribute("y", "5");
  r.setAttribute("width", "12");
  r.setAttribute("height", "12");
  r.setAttribute("rx", "1.5");
  r.setAttribute("fill", "currentColor");
  s.appendChild(r);
  return s;
}
