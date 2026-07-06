import type { Store } from "./store";
import { Vad } from "./stt/vad";
import { ServerSttClient } from "./stt/server-stt";
import { computeNextGlassesView } from "./state-machine";

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
      // Bail back to the entry screen — the user can't proceed
      // without mic access.
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

  /// Stop dictation and advance the glasses flow to the confirm
  /// screen, KEEPING the transcript so the user can choose to
  /// generate tags or re-describe. Fires on:
  ///   - the user single-tapping the body (manual commit)
  ///   - VAD detecting sustained silence (auto-pause)
  ///   - the FORCE_COMMIT_MS timeout (cap streaming session length)
  ///   - the phone-side "stop listening" action
  async finish(): Promise<void> {
    await this.cleanup();
    const cur = this.store.get().glassesView;
    // Only advance the view if we were still capturing — guards
    // against subscriber-triggered finish() calls that arrive after
    // the view has already moved past listening.
    if (cur === "listening") {
      const s = this.store.get();
      // Promote any in-flight interim text into the final transcript
      // in the same update as the view change. The confirm screen
      // reads only `listeningTranscript`, so without this the body
      // preview can be empty for short captures whose latest words
      // never made it out of the interim slot.
      this.store.update({
        listeningTranscript: s.listeningTranscript + s.listeningInterim,
        listeningInterim: "",
        glassesView: computeNextGlassesView(cur, { kind: "commit_listening" }, {}),
      });
    }
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
