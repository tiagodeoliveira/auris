import type { Intent, ServerEvent, WsStatus } from "./types";

interface Options {
  url: string;
  token: string;
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
   * late-fire events (browsers can still emit `onerror` between
   * `close()` and the actual close) from clobbering shared state —
   * specifically, the old socket's `onerror` was overwriting
   * `wsStatus` after a fresh socket had already fired "open" via
   * `reconnect()` in main.ts.
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
    // Detach any stale WS — covers both internal reconnect (after
    // a transient failure) and any caller that triggers connect()
    // while a previous socket is still in flight.
    this.detachWs();
    const fullUrl = `${this.opts.url}/?token=${encodeURIComponent(this.opts.token)}`;
    this.opts.onStatus("connecting");
    const ws = new WebSocket(fullUrl);
    this.ws = ws;

    ws.onopen = () => {
      this.backoff = INITIAL_BACKOFF_MS;
      this.opts.onStatus("open");
      this.armHeartbeat();
      // Drain queue.
      while (this.queue.length > 0) {
        const intent = this.queue.shift()!;
        ws.send(JSON.stringify(intent));
      }
    };

    ws.onmessage = (evt) => {
      this.armHeartbeat();
      try {
        const parsed = JSON.parse(evt.data) as ServerEvent;
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
