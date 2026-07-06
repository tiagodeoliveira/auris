//! Glasses-driven audio source for the active meeting.
//!
//! Counterpart to the Mac's `AudioStreamer`. Owns the lifecycle of
//! the binary WebSocket to `/audio` while we're the bound audio
//! source for the active meeting:
//!
//!   1. open a binary WebSocket to the server's `/audio` endpoint,
//!   2. turn the glasses mic on via `bridge.audioControl(true)`,
//!   3. forward every incoming `audioEvent.audioPcm` frame as a
//!      binary WS message,
//!   4. on unexpected close, reconnect on an exponential ladder
//!      while still bound — the meeting is the user's intent.
//!
//! State machine — published as `audioCaptureState` on the store so
//! the top-bar pill and persistent banner reflect reality:
//!
//!   idle ── start() ──► connecting ── onopen ──► streaming
//!                                                    │
//!                       ┌────────── onclose ◄────────┘ (network)
//!                       ▼
//!                  reconnecting ── retry ──► connecting
//!
//!   any → idle           when stop() is called
//!   any → failed         when getAccessToken throws (auth dead)
//!
//! Why the state machine: the old code published nothing and relied
//! on a main.ts reactor (subscribed to ownDeviceId/audioSourceDeviceId)
//! to restart after a close. The reactor only fires on the *binding*
//! flipping, not on socket close — so a spurious network reset left
//! audio dead with no UI indication and no auto-recovery. This module
//! is now the source of truth for "are frames actually flowing right
//! now," and recovers itself.
//!
//! Reconnect ladder: 500, 1000, 2000, 4000, 8000, 16000 ms, capped
//! at 16s and retried indefinitely while bound. We never give up on
//! network errors; only auth failure (token fetch throws) puts us in
//! `failed`. Frames received during reconnect are dropped — no
//! buffering, since the UI reports the gap honestly via the state.
//!
//! Glasses output is already 16 kHz mono S16LE PCM, which is exactly
//! what the server expects, so we forward the bytes untouched.

import type { Store } from "./store";
import type { AudioCaptureState } from "./types";

interface BridgeLike {
  audioControl(open: boolean): Promise<boolean>;
}

export interface GlassesAudioSourceDeps {
  /// Returns the server WS root, e.g. `ws://localhost:7331`. Pulled
  /// at start() time so a settings change between meetings takes
  /// effect on the next bind without an app restart.
  getServerUrl: () => string;
  /// Returns a fresh access token. Same provider used by the control
  /// WS — auth refresh is handled centrally there.
  getAccessToken: () => Promise<string>;
}

/// Backoff schedule in ms — index = retry attempt (0-based). Beyond
/// the last entry we stay at the final value. Tuned for "spurious
/// network blip" recovery on the order of seconds; longer outages
/// continue retrying at the 16s cadence until the user stops or the
/// network comes back.
const BACKOFF_MS = [500, 1000, 2000, 4000, 8000, 16000];

function backoffFor(attempt: number): number {
  return BACKOFF_MS[Math.min(attempt, BACKOFF_MS.length - 1)];
}

export class GlassesAudioSource {
  /// Open WebSocket while we're connected. Null between attempts.
  private socket: WebSocket | null = null;
  /// Guards re-entrant `connect()` so a double-tick can't open two
  /// sockets in parallel.
  private starting = false;
  /// True between an explicit start() and the matching stop(). The
  /// retry loop is gated on this — a stop() during a pending retry
  /// must not let the timer fire and reopen the socket.
  private active = false;
  /// Pending setTimeout id for the next reconnect attempt, or null.
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  /// Current retry attempt count (0-based). Incremented per retry,
  /// reset to 0 on a successful `onopen`. Drives `backoffFor`.
  private attempt = 0;

  constructor(
    private bridge: BridgeLike,
    private store: Store,
    private deps: GlassesAudioSourceDeps,
  ) {}

  /// True while a frame written via `feed()` would actually be sent.
  /// Read by main.ts when routing incoming PCM — the listening view
  /// has its own consumer and we shouldn't double-feed.
  get isStreaming(): boolean {
    return this.socket?.readyState === WebSocket.OPEN;
  }

  /// Begin streaming. Idempotent — repeat calls while active are
  /// no-ops. State on entry should be `idle`; state on exit is
  /// `connecting` (or `failed` if the URL/token isn't workable).
  async start(): Promise<void> {
    if (this.active) return;
    this.active = true;
    this.attempt = 0;
    await this.connect();
  }

  /// Stop streaming. Cancels any pending retry, closes the socket,
  /// turns the mic off, and publishes `idle`. Safe from any state.
  async stop(): Promise<void> {
    this.active = false;
    if (this.retryTimer !== null) {
      clearTimeout(this.retryTimer);
      this.retryTimer = null;
    }
    const ws = this.socket;
    this.socket = null;
    if (ws) {
      // Detach handlers BEFORE close() so the onclose-driven
      // reconnect path can't fire on a socket we're tearing down
      // intentionally.
      ws.onopen = null;
      ws.onerror = null;
      ws.onclose = null;
      try {
        ws.close();
      } catch {
        // close() throws if already closed — harmless.
      }
    }
    this.publish({ kind: "idle" });
    await this.bridge.audioControl(false);
  }

  /// Forward one PCM frame to the audio socket. Caller is `main.ts`,
  /// which sees every `audioEvent.audioPcm` and routes here when
  /// we're streaming. Dropped if the socket isn't OPEN — covers
  /// both the pre-handshake race AND any reconnect window. We
  /// deliberately do NOT buffer dropped frames; the gap is reported
  /// honestly via `audioCaptureState`.
  feed(pcm: Uint8Array): void {
    const ws = this.socket;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    // Copy into a tight ArrayBuffer — Uint8Array views over a shared
    // pool can carry trailing bytes the WS impl might ship unchanged.
    ws.send(pcm.buffer.slice(pcm.byteOffset, pcm.byteOffset + pcm.byteLength));
  }

  /// Open the WS and wire its lifecycle handlers. Called from
  /// `start()` and from the retry timer. State on entry is one of
  /// `idle | reconnecting`; state on synchronous exit is
  /// `connecting` or `failed`.
  private async connect(): Promise<void> {
    if (!this.active) return;
    if (this.socket || this.starting) return;
    this.starting = true;
    this.publish({ kind: "connecting" });
    try {
      const serverUrl = this.deps.getServerUrl();
      if (!serverUrl) {
        this.publish({ kind: "failed", reason: "Server URL missing" });
        this.active = false;
        return;
      }
      let token: string;
      try {
        token = await this.deps.getAccessToken();
      } catch {
        // A token fetch failure is almost always the network being
        // down — which is exactly when we're mid-reconnect (wifi→5G,
        // tunnel, sleep/wake). Treating it as terminal here was a bug:
        // it stranded the ladder in `failed` precisely when it should
        // keep trying. Schedule another attempt on the backoff ladder;
        // the token provider's own re-pair routing handles the truly
        // unrecoverable case (refresh token rejected). The `finally`
        // below resets `starting`; scheduleRetry queues the next try.
        this.scheduleRetry();
        return;
      }
      // start() might have been cancelled while we were awaiting the
      // token — bail before opening a socket nobody wants.
      if (!this.active) return;

      // Dial /audio with the same `?token=` shape the control WS
      // uses; the server's auth layer is identical for both routes.
      const url = `${serverUrl}/audio?token=${encodeURIComponent(token)}`;
      const ws = new WebSocket(url);
      ws.binaryType = "arraybuffer";
      this.socket = ws;
      ws.onopen = () => {
        // Reset backoff: any subsequent close starts the ladder
        // over from 500ms. Mic comes ON only after the socket is up;
        // otherwise the first frames would be dropped on the floor
        // and the user would see an empty transcript opening.
        this.attempt = 0;
        this.publish({ kind: "streaming", since: Date.now() });
        void this.bridge.audioControl(true);
      };
      ws.onerror = () => {
        // Browser WebSocket errors are opaque (no detail). The
        // `onclose` that follows drives the state transition; there's
        // nothing useful to do here on its own.
      };
      ws.onclose = () => {
        this.socket = null;
        void this.bridge.audioControl(false);
        if (!this.active) {
          // stop() already published `idle` and detached handlers
          // before closing — but defensive against races.
          return;
        }
        this.scheduleRetry();
      };
    } finally {
      this.starting = false;
    }
  }

  /// Schedule the next reconnect attempt per the backoff ladder.
  /// Publishes `reconnecting` immediately so the UI shows the
  /// degraded state during the wait, not just during the connect
  /// attempt itself.
  private scheduleRetry(): void {
    if (!this.active) return;
    if (this.retryTimer !== null) return; // already scheduled
    const delay = backoffFor(this.attempt);
    this.publish({
      kind: "reconnecting",
      // 1-indexed for display ("attempt 1, attempt 2, ...")
      attempt: this.attempt + 1,
      since: Date.now(),
    });
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      this.attempt += 1;
      void this.connect();
    }, delay);
  }

  private publish(next: AudioCaptureState): void {
    this.store.update({ audioCaptureState: next });
  }
}
