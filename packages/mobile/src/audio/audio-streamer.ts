// AudioStreamer — opens a WebSocket to `<serverURL>/audio?token=<jwt>`
// and forwards binary PCM frames (16 kHz mono S16LE, ~640 bytes / 20 ms
// each) into it. Mirrors the Mac client's AudioStreamer.swift wire
// contract; the server-side endpoint is documented in
// packages/server/src/audio/remote.rs.
//
// Why a separate module from useAudioCapture:
//   - capture (mic → PCM frames) and transport (PCM → /audio WS) are
//     independent concerns. The hook can be reused later for, e.g.,
//     local-only VAD without the network side.
//   - reconnect logic stays out of React render path; the hook only
//     calls connect/disconnect/feed.
//
// Reconnect: lazy. We open the WS on first `feed()` after `start()`,
// and re-open on close with simple exponential backoff. We do NOT
// queue frames during the gap — dropping ~1 s of audio on a network
// blip is preferable to a backlogged catch-up that desyncs the live
// transcript.

const INITIAL_BACKOFF_MS = 500;
const MAX_BACKOFF_MS = 10_000;
const BACKOFF_FACTOR = 2;

export interface AudioStreamerOptions {
  /// The control WS server URL, e.g. `wss://api.auris.dev`. The
  /// `/audio` path is appended; query is replaced with `?token=`.
  serverUrl: string;
  /// Fresh Auth0 access token per (re)connect. The audio WS validates
  /// it the same way the control WS does.
  getAccessToken: () => Promise<string>;
  /// Optional hook for status changes — currently consumed only by
  /// logs / tests. The UI reads `isRecording` from the capture hook.
  onStatusChange?: (status: AudioStreamerStatus) => void;
}

export type AudioStreamerStatus = "idle" | "connecting" | "open" | "reconnecting" | "error";

export class AudioStreamer {
  private ws: WebSocket | null = null;
  private status: AudioStreamerStatus = "idle";
  private backoff = INITIAL_BACKOFF_MS;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private stopped = true;

  constructor(private opts: AudioStreamerOptions) {}

  /// Begin streaming. Idempotent. Opens the WS asynchronously; frames
  /// fed before the handshake completes are dropped (see file header).
  start(): void {
    if (!this.stopped) return;
    this.stopped = false;
    this.backoff = INITIAL_BACKOFF_MS;
    void this.connect();
  }

  /// Stop streaming and close the underlying WS. Idempotent.
  stop(): void {
    this.stopped = true;
    this.clearReconnect();
    this.detachWs();
    this.setStatus("idle");
  }

  /// Forward one PCM frame. Silently drops if the WS isn't open —
  /// network blips shouldn't bubble up to the capture loop.
  feed(pcm: Uint8Array): void {
    const ws = this.ws;
    if (!ws || ws.readyState !== 1 /* OPEN */) return;
    try {
      ws.send(pcm);
    } catch {
      // The WS will fire `onclose` on real failures; ignore here so
      // the capture loop isn't blocked by transient send errors.
    }
  }

  private async connect(): Promise<void> {
    if (this.stopped) return;
    this.setStatus("connecting");
    let token: string;
    try {
      token = await this.opts.getAccessToken();
    } catch (e) {
      console.warn("[audio-streamer] token fetch failed", e);
      this.setStatus("error");
      this.scheduleReconnect();
      return;
    }
    const url = buildAudioUrl(this.opts.serverUrl, token);
    if (!url) {
      console.warn("[audio-streamer] invalid server URL", this.opts.serverUrl);
      this.setStatus("error");
      // No retry: the URL is not going to fix itself.
      return;
    }
    let ws: WebSocket;
    try {
      ws = new WebSocket(url);
    } catch (e) {
      console.warn("[audio-streamer] WS open failed", e);
      this.setStatus("error");
      this.scheduleReconnect();
      return;
    }
    ws.binaryType = "arraybuffer";
    this.ws = ws;

    ws.onopen = () => {
      this.backoff = INITIAL_BACKOFF_MS;
      this.setStatus("open");
    };
    ws.onerror = () => {
      this.setStatus("error");
    };
    ws.onclose = () => {
      this.ws = null;
      if (this.stopped) return;
      this.setStatus("reconnecting");
      this.scheduleReconnect();
    };
    // We don't expect inbound messages on /audio; ignore if any.
    ws.onmessage = () => {};
  }

  private scheduleReconnect(): void {
    this.clearReconnect();
    if (this.stopped) return;
    const delay = this.backoff;
    this.reconnectTimer = setTimeout(() => {
      this.backoff = Math.min(MAX_BACKOFF_MS, this.backoff * BACKOFF_FACTOR);
      void this.connect();
    }, delay);
  }

  private clearReconnect(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }

  private detachWs(): void {
    const ws = this.ws;
    if (!ws) return;
    ws.onopen = null;
    ws.onmessage = null;
    ws.onerror = null;
    ws.onclose = null;
    try {
      ws.close();
    } catch {
      // close() on an already-closed socket throws; harmless.
    }
    this.ws = null;
  }

  private setStatus(s: AudioStreamerStatus): void {
    if (this.status === s) return;
    this.status = s;
    this.opts.onStatusChange?.(s);
  }
}

/// Build `<server>/audio?token=<jwt>`. Accepts both `ws://` and
/// `wss://`, preserves host/port, replaces path with `/audio`.
/// Returns null if the input doesn't parse as a WS URL.
export function buildAudioUrl(serverUrl: string, token: string): string | null {
  let u: URL;
  try {
    u = new URL(serverUrl);
  } catch {
    return null;
  }
  if (u.protocol !== "ws:" && u.protocol !== "wss:") return null;
  u.pathname = "/audio";
  u.search = "";
  u.searchParams.set("token", token);
  return u.toString();
}
