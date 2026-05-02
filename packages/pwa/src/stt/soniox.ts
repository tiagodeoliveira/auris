const SONIOX_URL = "wss://stt-rt.soniox.com/transcribe-websocket";

interface Options {
  apiKey: string;
  onTranscript: (t: { interim: string; final: string }) => void;
  onError?: (err: string) => void;
}

interface SonioxToken {
  text: string;
  is_final: boolean;
}

export class SonioxClient {
  private ws: WebSocket | null = null;
  private finalText = "";

  constructor(private opts: Options) {}

  start(): void {
    const ws = new WebSocket(SONIOX_URL);
    this.ws = ws;
    ws.onopen = () => {
      // Soniox config message — adapt fields to current API; this is a sketch.
      ws.send(
        JSON.stringify({
          api_key: this.opts.apiKey,
          audio_format: "pcm_s16le",
          sample_rate: 16000,
          num_channels: 1,
          model: "stt-rt-preview",
        }),
      );
    };
    ws.onmessage = (evt) => {
      try {
        const data = JSON.parse(evt.data);
        if (Array.isArray(data.tokens)) {
          let interim = "";
          for (const tok of data.tokens as SonioxToken[]) {
            if (tok.is_final) {
              this.finalText += tok.text;
            } else {
              interim += tok.text;
            }
          }
          this.opts.onTranscript({ final: this.finalText, interim });
        }
        if (data.error_code === 401 || data.error?.code === 401) {
          this.opts.onError?.("Soniox API key invalid");
        }
      } catch {
        // ignore malformed
      }
    };
    ws.onerror = () => this.opts.onError?.("Soniox WS error");
  }

  feed(pcm: Uint8Array): void {
    // 1 === WebSocket.OPEN; use literal to avoid dependency on static constant in tests
    if (this.ws?.readyState === 1) {
      this.ws.send(pcm);
    }
  }

  stop(): void {
    this.ws?.close();
    this.ws = null;
    this.finalText = "";
  }
}
