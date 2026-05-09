// WebSocket client with reconnect + heartbeat. Hand-ported from
// packages/pwa/src/ws.ts; the underlying API (`WebSocket`) is the
// same in React Native as in the browser, so the port is mostly a
// rename + a couple of import-path tweaks.
//
// Differences from the PWA version:
//   - Imports the wire types from `./contract` (no PWA-internal
//     `./types` re-exports) — type aliases at the top match the
//     names the PWA uses.
//   - That's it. The reconnect/heartbeat/queue logic is verbatim.

import type { Event, Intent } from "./contract";

/// Mirrors the PWA's `ServerEvent` alias for ease of cross-reference
/// when reading both clients side-by-side.
export type ServerEvent = Event;

/// State the host (e.g. a Zustand store) cares about — drives the
/// "connecting…" / "reconnecting…" UI affordances.
export type WsStatus = "connecting" | "open" | "reconnecting" | "closed" | "error";

interface Options {
  url: string;
  /// Each (re)connect fetches a fresh token. Sync return preserved
  /// for tests; prod paths return a Promise (Auth0's
  /// getAccessToken() is async).
  tokenProvider: () => string | Promise<string>;
  onEvent: (event: ServerEvent) => void;
  onStatus: (status: WsStatus) => void;
}

const INITIAL_BACKOFF_MS = 1000;
const MAX_BACKOFF_MS = 30_000;
const BACKOFF_FACTOR = 2;
const JITTER = 0.2;
const HEARTBEAT_LOSS_MS = 25_000;

export class ReconnectingSocket {
  private ws: WebSocket | null = null;
  private queue: Intent[] = [];
  private backoff = INITIAL_BACKOFF_MS;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private heartbeatTimer: ReturnType<typeof setTimeout> | null = null;
  private closed = false;

  constructor(private opts: Options) {
    this.connect();
  }

  send(intent: Intent): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(intent));
    } else {
      this.queue.push(intent);
    }
  }

  close(): void {
    this.closed = true;
    this.clearReconnect();
    this.clearHeartbeat();
    this.detachWs();
  }

  /** Detach handlers and close the underlying WS, if any. Prevents
   * late-fire events between `close()` and the actual close from
   * clobbering shared state.
   */
  private detachWs(): void {
    const ws = this.ws;
    if (ws) {
      ws.onopen = null;
      ws.onmessage = null;
      ws.onerror = null;
      ws.onclose = null;
      try {
        ws.close();
      } catch {
        // close() throws if already closed; harmless.
      }
      this.ws = null;
    }
  }

  private connect(): void {
    if (this.closed) return;
    this.detachWs();
    this.opts.onStatus("connecting");
    let result: string | Promise<string>;
    try {
      result = this.opts.tokenProvider();
    } catch (e) {
      console.warn("[ws] token fetch failed", e);
      this.opts.onStatus("error");
      this.scheduleReconnect();
      return;
    }
    if (typeof result === "string") {
      this.openWithToken(result);
    } else {
      result
        .then((t) => {
          if (!this.closed) this.openWithToken(t);
        })
        .catch((e) => {
          console.warn("[ws] token fetch failed", e);
          this.opts.onStatus("error");
          this.scheduleReconnect();
        });
    }
  }

  private openWithToken(token: string): void {
    if (this.closed) return;
    const fullUrl = `${this.opts.url}/?token=${encodeURIComponent(token)}`;
    const ws = new WebSocket(fullUrl);
    this.ws = ws;

    ws.onopen = () => {
      this.backoff = INITIAL_BACKOFF_MS;
      this.opts.onStatus("open");
      this.armHeartbeat();
      while (this.queue.length > 0) {
        const intent = this.queue.shift()!;
        ws.send(JSON.stringify(intent));
      }
    };

    ws.onmessage = (evt) => {
      this.armHeartbeat();
      try {
        const parsed = JSON.parse(evt.data as string) as ServerEvent;
        this.opts.onEvent(parsed);
      } catch {
        // Ignore malformed inbound; server contract says JSON only.
      }
    };

    ws.onerror = () => {
      this.opts.onStatus("error");
    };

    ws.onclose = () => {
      this.clearHeartbeat();
      if (this.closed) return;
      this.opts.onStatus("reconnecting");
      this.scheduleReconnect();
    };
  }

  private scheduleReconnect(): void {
    this.clearReconnect();
    const jitter = (Math.random() * 2 - 1) * JITTER * this.backoff;
    const delay = Math.min(MAX_BACKOFF_MS, this.backoff + jitter);
    this.reconnectTimer = setTimeout(() => {
      this.backoff = Math.min(MAX_BACKOFF_MS, this.backoff * BACKOFF_FACTOR);
      this.connect();
    }, delay);
  }

  private armHeartbeat(): void {
    this.clearHeartbeat();
    this.heartbeatTimer = setTimeout(() => {
      // No event in HEARTBEAT_LOSS_MS — assume connection dead.
      this.ws?.close();
    }, HEARTBEAT_LOSS_MS);
  }

  private clearReconnect(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }

  private clearHeartbeat(): void {
    if (this.heartbeatTimer) {
      clearTimeout(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }
}
