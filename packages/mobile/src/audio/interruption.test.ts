// Tests for the pure interruption/stall state machine used by
// useAudioCapture. These pin the behavior that fixes improvement
// #196: an OS audio-session interruption (phone call, Siri, audio
// focus loss) must surface a sticky banner instead of silently
// freezing capture, and a frame-stall watchdog must catch every
// other "mic silently died" cause.
import { describe, expect, it } from "vitest";

import {
  FRAME_STALL_MESSAGE,
  FRAME_STALL_THRESHOLD_MS,
  interruptionStatus,
  isFrameStalled,
  isInterruptionMessage,
} from "./interruption";

describe("interruptionStatus", () => {
  it("sets a sticky message naming the phone call when paused", () => {
    const msg = interruptionStatus(null, { reason: "phoneCall", isPaused: true });
    expect(msg).toMatch(/phone call/i);
  });

  it("sets a sticky message for audio-focus loss naming another app", () => {
    const msg = interruptionStatus(null, { reason: "audioFocusLoss", isPaused: true });
    expect(msg).toMatch(/another app/i);
  });

  it("falls back to a generic paused message for unknown reasons", () => {
    const msg = interruptionStatus(null, { reason: "someFutureReason", isPaused: true });
    expect(msg).toMatch(/paused/i);
  });

  it("clears its own message when the interruption ends", () => {
    const paused = interruptionStatus(null, { reason: "phoneCall", isPaused: true });
    expect(paused).not.toBeNull();
    expect(interruptionStatus(paused, { reason: "phoneCallEnded", isPaused: false })).toBeNull();
  });

  it("does not clear unrelated errors on resume events", () => {
    // A resume event must never wipe e.g. a permission error that
    // some other code path set.
    const prev = "Microphone access denied";
    expect(interruptionStatus(prev, { reason: "audioFocusGain", isPaused: false })).toBe(prev);
  });

  it("leaves state unchanged for malformed or missing events", () => {
    const paused = interruptionStatus(null, { reason: "phoneCall", isPaused: true });
    expect(interruptionStatus(paused, {})).toBe(paused); // isPaused missing
    expect(interruptionStatus(paused, undefined)).toBe(paused);
    expect(interruptionStatus(paused, null)).toBe(paused);
    expect(interruptionStatus(null, { reason: "phoneCall" })).toBeNull();
  });

  it("classifies its own messages via isInterruptionMessage", () => {
    const paused = interruptionStatus(null, { reason: "phoneCall", isPaused: true });
    expect(isInterruptionMessage(paused)).toBe(true);
    expect(isInterruptionMessage(FRAME_STALL_MESSAGE)).toBe(false);
    expect(isInterruptionMessage("Microphone access denied")).toBe(false);
    expect(isInterruptionMessage(null)).toBe(false);
  });
});

describe("isFrameStalled", () => {
  const t0 = 1_000_000;

  it("is false while not recording", () => {
    expect(
      isFrameStalled(t0, t0 + FRAME_STALL_THRESHOLD_MS * 2, false, FRAME_STALL_THRESHOLD_MS),
    ).toBe(false);
  });

  it("is false before any frame baseline exists", () => {
    expect(isFrameStalled(null, t0, true, FRAME_STALL_THRESHOLD_MS)).toBe(false);
  });

  it("is true once the threshold elapses with no frames", () => {
    expect(isFrameStalled(t0, t0 + FRAME_STALL_THRESHOLD_MS, true, FRAME_STALL_THRESHOLD_MS)).toBe(
      true,
    );
  });

  it("is false just under the threshold", () => {
    expect(
      isFrameStalled(t0, t0 + FRAME_STALL_THRESHOLD_MS - 1, true, FRAME_STALL_THRESHOLD_MS),
    ).toBe(false);
  });

  it("is false again once frames resume (fresh lastFrameAt)", () => {
    const resumedAt = t0 + FRAME_STALL_THRESHOLD_MS;
    expect(isFrameStalled(resumedAt, resumedAt + 100, true, FRAME_STALL_THRESHOLD_MS)).toBe(false);
  });
});
