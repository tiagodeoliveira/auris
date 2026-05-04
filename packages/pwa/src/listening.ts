import type { Store } from "./store";
import { Vad } from "./stt/vad";
import { SonioxClient } from "./stt/soniox";

interface BridgeLike {
  audioControl(open: boolean): Promise<boolean>;
}

const VAD_SILENCE_MS = 2500;
const VAD_MIN_SPEECH_MS = 500;
const FORCE_COMMIT_MS = 25_000;

export class ListeningSession {
  private vad: Vad;
  private soniox: SonioxClient | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private bridge: BridgeLike,
    private store: Store,
  ) {
    this.vad = new Vad({
      silenceMs: VAD_SILENCE_MS,
      minSpeechMs: VAD_MIN_SPEECH_MS,
      sampleRateHz: 16000,
    });
  }

  async start(): Promise<void> {
    const apiKey = this.store.get().settings.sonioxKey;
    if (!apiKey) {
      this.store.update({
        toasts: [
          ...this.store.get().toasts,
          {
            id: `t${Date.now()}`,
            text: "Soniox API key missing — set it in Settings",
            level: "error",
            expiresAt: Date.now() + 4000,
          },
        ],
        glassesView: "idle",
      });
      return;
    }

    const ok = await this.bridge.audioControl(true);
    if (!ok) {
      this.store.update({
        toasts: [
          ...this.store.get().toasts,
          {
            id: `t${Date.now()}`,
            text: "Microphone access denied",
            level: "error",
            expiresAt: Date.now() + 4000,
          },
        ],
        glassesView: "idle",
      });
      return;
    }

    this.soniox = new SonioxClient({
      apiKey,
      onTranscript: ({ interim, final }) => {
        this.store.update({ listeningInterim: interim, listeningTranscript: final });
      },
      onError: (err) => {
        this.store.update({
          toasts: [
            ...this.store.get().toasts,
            {
              id: `t${Date.now()}`,
              text: err,
              level: "error",
              expiresAt: Date.now() + 4000,
            },
          ],
        });
      },
    });
    this.soniox.start();

    this.store.update({
      listeningStartedAt: Date.now(),
      listeningTranscript: "",
      listeningInterim: "",
    });

    this.timer = setTimeout(() => void this.finish(), FORCE_COMMIT_MS);
  }

  feedAudio(pcm: Uint8Array): void {
    this.vad.feed(pcm, Date.now());
    this.soniox?.feed(pcm);
    if (this.vad.shouldCommit()) {
      void this.finish();
    }
  }

  /// Stop dictation and exit the listening view, KEEPING the transcript so
  /// the user can review/edit it in the textarea before pressing Start. This
  /// is what fires on:
  ///   - the user clicking the mic icon a second time (toggle off)
  ///   - VAD detecting sustained silence (auto-pause to save Soniox quota)
  ///   - the FORCE_COMMIT_MS timeout (cap the streaming session length)
  async finish(): Promise<void> {
    await this.cleanup();
    this.store.update({ glassesView: "idle" });
    // listeningTranscript intentionally preserved.
  }

  async cancel(): Promise<void> {
    await this.cleanup();
    this.store.update({
      glassesView: "idle",
      listeningTranscript: "",
      listeningInterim: "",
      listeningStartedAt: null,
    });
  }

  private async cleanup(): Promise<void> {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    this.soniox?.stop();
    this.soniox = null;
    this.vad.reset();
    await this.bridge.audioControl(false);
  }
}
