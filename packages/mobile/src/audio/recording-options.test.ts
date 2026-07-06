// Pins the @siteed/audio-studio startRecording options. The single
// most important assertion: autoResumeAfterInterruption MUST be
// true. The lib's default is false on BOTH platforms
// (ios/RecordingSettings.swift, android RecordingConfig.kt), and
// without it an incoming phone call permanently silences the rest
// of the meeting (improvement #196).
import { describe, expect, it } from "vitest";

import { buildRecordingOptions } from "./recording-options";

describe("buildRecordingOptions", () => {
  it("enables autoResumeAfterInterruption so phone calls do not kill capture", () => {
    expect(buildRecordingOptions().autoResumeAfterInterruption).toBe(true);
  });

  it("keeps the wire format pinned to the server /audio contract", () => {
    const opts = buildRecordingOptions();
    // Must match Mac AudioStreamer + packages/server/src/audio/remote.rs:
    // PCM 16 kHz mono S16LE.
    expect(opts.sampleRate).toBe(16_000);
    expect(opts.channels).toBe(1);
    expect(opts.encoding).toBe("pcm_16bit");
    expect(opts.keepAwake).toBe(true);
  });
});
