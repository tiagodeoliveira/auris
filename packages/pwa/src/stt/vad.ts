const SILENCE_AMPLITUDE = 800;

interface Options {
  silenceMs: number;
  minSpeechMs: number;
  sampleRateHz: number;
}

export class Vad {
  private speechMs = 0;
  private silenceMs = 0;

  constructor(private opts: Options) {}

  feed(pcm: Uint8Array, _atMs: number): void {
    const samples = pcm.byteLength / 2;
    const durationMs = (samples / this.opts.sampleRateHz) * 1000;
    const view = new DataView(pcm.buffer, pcm.byteOffset, pcm.byteLength);
    let maxAbs = 0;
    for (let i = 0; i < samples; i++) {
      const v = Math.abs(view.getInt16(i * 2, true));
      if (v > maxAbs) maxAbs = v;
    }
    if (maxAbs >= SILENCE_AMPLITUDE) {
      this.speechMs += durationMs;
      this.silenceMs = 0;
    } else {
      this.silenceMs += durationMs;
    }
  }

  shouldCommit(): boolean {
    return this.speechMs >= this.opts.minSpeechMs && this.silenceMs >= this.opts.silenceMs;
  }

  reset(): void {
    this.speechMs = 0;
    this.silenceMs = 0;
  }
}
