// Server-mediated STT client. Connects to the auris
// server's `/stt` WebSocket; the server holds the upstream provider's
// API key and can swap providers without a client redeploy.
//
// Wire shape (server-side: packages/server/src/stt_ws.rs):
//   client → server : binary PCM (16 kHz mono S16LE) frames
//   server → client : tagged JSON
//     - {type:"ready"}
//     - {type:"interim",text}
//     - {type:"final",text,t_start_ms,t_end_ms}
//     - {type:"error",code,message}
//
// The shape mirrors the prior SonioxClient (start/feed/stop) so
// listening.ts barely changes. Final + interim are still surfaced as
// the rolling `{ final, interim }` pair the rest of the app expects.

interface Options {
  serverUrl: string;
  /// Returns a fresh access token (Auth0 silent renewal). Called once
  /// at start; the token is appended as `?token=` on the WS URL.
  getAccessToken: () => Promise<string>;
  onTranscript: (t: { interim: string; final: string }) => void;
  onError?: (err: string) => void;
  onReady?: () => void;
}

export class ServerSttClient {
  private ws: WebSocket | null = null;
  private finalText = "";
  private pendingFrames: Uint8Array[] = [];
  private opened = false;

  constructor(private opts: Options) {}

  async start(): Promise<void> {
    let token: string;
    try {
      token = await this.opts.getAccessToken();
    } catch (e) {
      this.opts.onError?.(`Auth failed: ${(e as Error).message ?? String(e)}`);
      return;
    }

    const url = buildSttUrl(this.opts.serverUrl, token);
    if (!url) {
      this.opts.onError?.("Invalid server URL");
      return;
    }

    let ws: WebSocket;
    try {
      ws = new WebSocket(url);
    } catch (e) {
      this.opts.onError?.(`STT WS open failed: ${(e as Error).message ?? String(e)}`);
      return;
    }
    ws.binaryType = "arraybuffer";
    this.ws = ws;

    ws.onopen = () => {
      this.opened = true;
      // Drain any frames that arrived between start() and the WS
      // handshake completing — they're already in mic time order.
      for (const pcm of this.pendingFrames) {
        ws.send(pcm);
      }
      this.pendingFrames.length = 0;
    };

    ws.onmessage = (evt) => {
      // Server sends JSON text frames only. Binary back is reserved
      // for future shape changes; ignore for now.
      if (typeof evt.data !== "string") return;
      let msg: { type?: string; text?: string; code?: string; message?: string };
      try {
        msg = JSON.parse(evt.data);
      } catch {
        return;
      }
      switch (msg.type) {
        case "ready":
          this.opts.onReady?.();
          break;
        case "interim":
          this.opts.onTranscript({ final: this.finalText, interim: msg.text ?? "" });
          break;
        case "final":
          // Append a space between utterances so the rolling buffer
          // reads naturally. Trim leading whitespace to avoid double
          // spaces if the server already emitted one.
          this.finalText = appendUtterance(this.finalText, msg.text ?? "");
          this.opts.onTranscript({ final: this.finalText, interim: "" });
          break;
        case "error":
          this.opts.onError?.(msg.message ?? msg.code ?? "STT error");
          break;
      }
    };

    ws.onerror = () => this.opts.onError?.("STT WS error");
    ws.onclose = () => {
      this.opened = false;
    };
  }

  feed(pcm: Uint8Array): void {
    if (!this.ws) return;
    if (this.opened && this.ws.readyState === 1) {
      this.ws.send(pcm);
    } else {
      // Buffer frames received before the WS handshake completed.
      // Without this we'd lose the first ~100-300 ms of dictation.
      this.pendingFrames.push(pcm);
    }
  }

  stop(): void {
    const ws = this.ws;
    this.ws = null;
    this.opened = false;
    this.pendingFrames.length = 0;
    this.finalText = "";
    if (!ws) return;
    // Send a "stop" hint so the server flushes any in-flight buffer
    // before tearing down. Closing without it works too — the
    // provider has its own on-cancel flush — but the explicit hint
    // shaves ~300 ms off the close path on slow networks.
    if (ws.readyState === 1) {
      try {
        ws.send(JSON.stringify({ type: "stop" }));
      } catch {
        // Ignore — closing anyway.
      }
    }
    ws.close();
  }
}

function appendUtterance(prev: string, next: string): string {
  const cleaned = next.trim();
  if (!cleaned) return prev;
  if (!prev) return cleaned;
  return `${prev} ${cleaned}`;
}

/// Build `<server>/stt?token=<jwt>`. Accepts both `ws://` and `wss://`,
/// preserves the host/port, and replaces the path with `/stt`. Returns
/// null if the input doesn't parse as a WS URL.
export function buildSttUrl(serverUrl: string, token: string): string | null {
  let u: URL;
  try {
    u = new URL(serverUrl);
  } catch {
    return null;
  }
  if (u.protocol !== "ws:" && u.protocol !== "wss:") return null;
  u.pathname = "/stt";
  u.search = "";
  u.searchParams.set("token", token);
  return u.toString();
}
