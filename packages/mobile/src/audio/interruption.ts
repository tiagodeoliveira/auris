// Pure interruption / stall decision logic for the mobile mic
// capture (improvement #196).
//
// Why a separate module from useAudioCapture: the hook lives in
// React-Native land (expo-modules-core, a native-module require())
// and can't be imported by the node-side vitest suite. Everything
// decision-shaped about OS audio-session interruptions lives here
// instead, where it is unit-tested (interruption.test.ts).
//
// Background: @siteed/audio-studio pauses recording when the OS
// interrupts the audio session (incoming call, Siri, audio-focus
// loss) and emits "onRecordingInterrupted" with { reason, isPaused }.
// With `autoResumeAfterInterruption: true` (see
// recording-options.ts) it also resumes ~2 s after the interruption
// ends — but the iOS "ended" event fires BEFORE the delayed resume
// actually succeeds, and the resume can fail outright after long
// calls (AVAudioSession reactivation throws). So:
//   - interruptionStatus() drives a sticky banner from the events
//     (optimistically cleared on the "ended" event), and
//   - isFrameStalled() backs it up: if frames don't actually return,
//     the watchdog in useAudioCapture re-raises a visible error
//     instead of letting the meeting record silence indefinitely.

/// Shape of the lib's "onRecordingInterrupted" emitter payload.
/// Mirrors `RecordingInterruptionEvent` in
/// node_modules/@siteed/audio-studio/src/AudioStudio.types.ts
/// (reason ∈ phoneCall | phoneCallEnded | audioFocusLoss |
/// audioFocusGain | recordingStopped | deviceDisconnected | ...).
/// Typed loosely because it crosses the native bridge and future
/// lib versions may add reasons.
export interface InterruptionEventLike {
  reason?: string;
  isPaused?: boolean;
}

/// Reason-specific banner copy. Anything not listed gets the
/// generic message below — better a vague banner than none.
const PAUSE_MESSAGES: Record<string, string> = {
  phoneCall: "Recording paused by a phone call — it will resume when the call ends.",
  audioFocusLoss:
    "Recording paused — another app took the microphone. It should resume automatically.",
};

const DEFAULT_PAUSE_MESSAGE = "Recording paused by the system — it should resume automatically.";

/// Sticky error raised by the frame-stall watchdog when recording is
/// nominally live but no AudioData frames have arrived for
/// FRAME_STALL_THRESHOLD_MS. Catches auto-resume failures, route
/// loss, and any future "mic silently died" cause.
export const FRAME_STALL_MESSAGE =
  "Mic stalled — audio is not reaching the meeting. Stop and restart capture if this persists.";

/// No frames for this long while recording ⇒ stalled. Generous
/// enough to ride out the lib's 2 s delayed auto-resume plus
/// session-reactivation slack.
export const FRAME_STALL_THRESHOLD_MS = 15_000;

/// How often useAudioCapture's watchdog evaluates isFrameStalled.
export const WATCHDOG_INTERVAL_MS = 5_000;

const STICKY_INTERRUPTION_MESSAGES: ReadonlySet<string> = new Set([
  ...Object.values(PAUSE_MESSAGES),
  DEFAULT_PAUSE_MESSAGE,
]);

/// True iff `msg` is one of the banners interruptionStatus() sets.
/// Used so resume events / the watchdog only ever clear messages
/// this machinery owns — never e.g. a permission error.
export function isInterruptionMessage(msg: string | null): boolean {
  return msg !== null && STICKY_INTERRUPTION_MESSAGES.has(msg);
}

/// State transition for the sticky banner. `prev` is the hook's
/// current `error` value; returns the next one.
///   - paused (isPaused=true)  → reason-specific sticky message
///   - resumed (isPaused=false) → clear, but only our own message
///   - malformed / missing      → no change
export function interruptionStatus(
  prev: string | null,
  event: InterruptionEventLike | null | undefined,
): string | null {
  if (!event || typeof event.isPaused !== "boolean") return prev;
  if (event.isPaused) {
    return PAUSE_MESSAGES[event.reason ?? ""] ?? DEFAULT_PAUSE_MESSAGE;
  }
  return isInterruptionMessage(prev) ? null : prev;
}

/// Stall predicate evaluated by the watchdog. `lastFrameAt` is the
/// epoch-ms timestamp of the most recent AudioData frame (seeded to
/// "now" when recording starts); null means recording never started.
export function isFrameStalled(
  lastFrameAt: number | null,
  now: number,
  isRecording: boolean,
  thresholdMs: number,
): boolean {
  if (!isRecording) return false;
  if (lastFrameAt === null) return false;
  return now - lastFrameAt >= thresholdMs;
}
