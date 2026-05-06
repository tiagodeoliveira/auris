import type { Store } from "./store";
import { Vad } from "./stt/vad";
import { ServerSttClient } from "./stt/server-stt";

interface BridgeLike {
  audioControl(open: boolean): Promise<boolean>;
}

const VAD_SILENCE_MS = 2500;
const VAD_MIN_SPEECH_MS = 500;
const FORCE_COMMIT_MS = 25_000;

export interface ListeningDeps {
  /// Returns the WS server URL to dial for STT (e.g., `ws://localhost:7331`).
  /// We pull this from the store rather than capturing at construction so
  /// settings changes take effect on the next dictation start without a
  /// full app restart.
  getServerUrl: () => string;
  /// Returns a fresh access token. Same provider used by the main control
  /// WS — Auth0 silent renew handles the refresh under the hood.
  getAccessToken: () => Promise<string>;
}

export class ListeningSession {
  private vad: Vad;
  private stt: ServerSttClient | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private bridge: BridgeLike,
    private store: Store,
    private deps: ListeningDeps,
  ) {
    this.vad = new Vad({
      silenceMs: VAD_SILENCE_MS,
      minSpeechMs: VAD_MIN_SPEECH_MS,
      sampleRateHz: 16000,
    });
  }

  async start(): Promise<void> {
    const serverUrl = this.deps.getServerUrl();
    if (!serverUrl) {
      this.toast("Server URL missing — set it in Settings", "error");
      this.store.update({ glassesView: "idle" });
      return;
    }

    const ok = await this.bridge.audioControl(true);
    if (!ok) {
      this.toast("Microphone access denied", "error");
      this.store.update({ glassesView: "idle" });
      return;
    }

    this.stt = new ServerSttClient({
      serverUrl,
      getAccessToken: this.deps.getAccessToken,
      onTranscript: ({ interim, final }) => {
        this.store.update({ listeningInterim: interim, listeningTranscript: final });
      },
      onError: (err) => {
        this.toast(err, "error");
      },
    });
    void this.stt.start();

    this.store.update({
      listeningStartedAt: Date.now(),
      listeningTranscript: "",
      listeningInterim: "",
    });

    this.timer = setTimeout(() => void this.finish(), FORCE_COMMIT_MS);
  }

  feedAudio(pcm: Uint8Array): void {
    this.vad.feed(pcm, Date.now());
    this.stt?.feed(pcm);
    if (this.vad.shouldCommit()) {
      void this.finish();
    }
  }

  /// Stop dictation and exit the listening view, KEEPING the transcript so
  /// the user can review/edit it in the textarea before pressing Start. This
  /// is what fires on:
  ///   - the user clicking the mic icon a second time (toggle off)
  ///   - VAD detecting sustained silence (auto-pause to save quota)
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

  private toast(text: string, level: "info" | "warn" | "error"): void {
    this.store.update({
      toasts: [
        ...this.store.get().toasts,
        {
          id: `t${Date.now()}`,
          text,
          level,
          expiresAt: Date.now() + 4000,
        },
      ],
    });
  }

  private async cleanup(): Promise<void> {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    this.stt?.stop();
    this.stt = null;
    this.vad.reset();
    await this.bridge.audioControl(false);
  }
}
