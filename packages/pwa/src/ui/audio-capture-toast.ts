//! Persistent banner toast when audio capture is unhealthy during
//! an active meeting. Subscribes to `audioCaptureState` + `meetingState`
//! and pushes a persistent toast (no auto-dismiss) once the
//! unhealthy state has lasted longer than the grace window — a quick
//! blip during normal reconnect isn't worth interrupting the user,
//! but a sustained outage absolutely is.
//!
//! The original bug: `glasses-audio-source.ts` published a single
//! 4-second toast on disconnect and walked away. Easy to miss while
//! the user was looking at the transcript. The replacement is a
//! state-driven persistent banner that clears itself when (and only
//! when) capture returns to streaming.

import type { Store } from "../store";
import type { Toast } from "../types";

const TOAST_ID = "audio-capture-banner";
/// Grace window before the banner appears. Short enough that a real
/// outage is reported promptly, long enough that a routine reconnect
/// (single backoff cycle) doesn't flash a scary message.
const GRACE_MS = 5_000;

export function mountAudioCaptureToast(store: Store): void {
  /// Timestamp at which the current unhealthy run started. Reset
  /// only when capture returns to streaming or the meeting ends —
  /// state flapping between connecting/reconnecting doesn't reset
  /// the clock, so a flapping connection still surfaces after the
  /// grace window.
  let unhealthySince: number | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  function clearToast(): void {
    const cur = store.get().toasts;
    const next = cur.filter((t) => t.id !== TOAST_ID);
    if (next.length !== cur.length) {
      store.update({ toasts: next });
    }
  }

  function showToast(text: string, level: Toast["level"]): void {
    const cur = store.get().toasts;
    const existing = cur.find((t) => t.id === TOAST_ID);
    if (existing && existing.text === text && existing.level === level) return;
    const next = cur.filter((t) => t.id !== TOAST_ID);
    next.push({ id: TOAST_ID, text, level, expiresAt: null });
    store.update({ toasts: next });
  }

  function messageFor(
    state: ReturnType<typeof store.get>["audioCaptureState"],
  ): { text: string; level: Toast["level"] } | null {
    if (state.kind === "reconnecting") {
      return {
        text: `Audio reconnecting (attempt ${state.attempt}) — meeting is not being recorded right now.`,
        level: "warn",
      };
    }
    if (state.kind === "connecting") {
      return {
        text: "Audio reconnecting — meeting is not being recorded right now.",
        level: "warn",
      };
    }
    if (state.kind === "failed") {
      return {
        text: `Audio disconnected — ${state.reason}. Stop the meeting and re-pick the source to recover.`,
        level: "error",
      };
    }
    return null;
  }

  function evaluate(): void {
    const s = store.get();
    const active = s.meetingState === "active";
    const kind = s.audioCaptureState.kind;
    const unhealthy =
      active && (kind === "connecting" || kind === "reconnecting" || kind === "failed");

    if (!unhealthy) {
      unhealthySince = null;
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
      clearToast();
      return;
    }

    if (unhealthySince === null) {
      unhealthySince = Date.now();
    }
    const elapsed = Date.now() - unhealthySince;

    if (elapsed >= GRACE_MS) {
      const msg = messageFor(s.audioCaptureState);
      if (msg) showToast(msg.text, msg.level);
      return;
    }

    // Wait the remainder of the grace window before showing the
    // banner. Re-evaluate then in case the state changed (e.g.
    // recovered) — `evaluate()` is the single arbiter.
    if (timer === null) {
      timer = setTimeout(() => {
        timer = null;
        evaluate();
      }, GRACE_MS - elapsed);
    }
  }

  store.subscribe(
    (s) =>
      `${s.meetingState}|${s.audioCaptureState.kind}|${
        s.audioCaptureState.kind === "reconnecting" ? s.audioCaptureState.attempt : 0
      }`,
    evaluate,
  );
  evaluate();
}
