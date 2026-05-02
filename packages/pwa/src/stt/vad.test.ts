import { describe, expect, test } from "vitest";
import { Vad } from "./vad";

function silentFrame(samples: number): Uint8Array {
  return new Uint8Array(samples * 2); // 16-bit samples = 2 bytes each
}

function loudFrame(samples: number): Uint8Array {
  const buf = new Uint8Array(samples * 2);
  const view = new DataView(buf.buffer);
  for (let i = 0; i < samples; i++) {
    view.setInt16(i * 2, 5000, true); // signed 16-bit LE, well above silence
  }
  return buf;
}

describe("Vad", () => {
  test("does not commit before min-speech threshold", () => {
    const vad = new Vad({ silenceMs: 100, minSpeechMs: 500, sampleRateHz: 16000 });
    vad.feed(silentFrame(1600), Date.now()); // 100ms of silence
    expect(vad.shouldCommit()).toBe(false);
  });

  test("commits after silence following min speech", () => {
    const vad = new Vad({ silenceMs: 100, minSpeechMs: 500, sampleRateHz: 16000 });
    let t = Date.now();
    // 600ms of speech
    for (let i = 0; i < 6; i++) {
      vad.feed(loudFrame(1600), t);
      t += 100;
    }
    // 100ms of silence
    vad.feed(silentFrame(1600), t);
    expect(vad.shouldCommit()).toBe(true);
  });

  test("does not commit on transient silence shorter than threshold", () => {
    const vad = new Vad({ silenceMs: 200, minSpeechMs: 500, sampleRateHz: 16000 });
    let t = Date.now();
    for (let i = 0; i < 6; i++) {
      vad.feed(loudFrame(1600), t);
      t += 100;
    }
    vad.feed(silentFrame(1600), t); // 100ms silence — under 200ms threshold
    expect(vad.shouldCommit()).toBe(false);
  });
});
