import { describe, expect, test, vi, beforeEach, afterEach } from "vitest";
import { SonioxClient } from "./soniox";

class MockWS {
  static instances: MockWS[] = [];
  readyState = 1;
  sent: any[] = [];
  onopen: ((e: Event) => void) | null = null;
  onmessage: ((e: MessageEvent) => void) | null = null;
  onclose: ((e: CloseEvent) => void) | null = null;
  onerror: ((e: Event) => void) | null = null;
  constructor(public url: string) {
    MockWS.instances.push(this);
  }
  send(d: any) {
    this.sent.push(d);
  }
  close() {
    this.onclose?.(new CloseEvent("close"));
  }
  simulateOpen() {
    this.onopen?.(new Event("open"));
  }
  simulateMessage(data: any) {
    this.onmessage?.(new MessageEvent("message", { data: JSON.stringify(data) }));
  }
}

describe("SonioxClient", () => {
  beforeEach(() => {
    MockWS.instances = [];
    (globalThis as any).WebSocket = MockWS;
  });
  afterEach(() => {
    delete (globalThis as any).WebSocket;
  });

  test("opens WS with auth in handshake", () => {
    const onTranscript = vi.fn();
    const c = new SonioxClient({ apiKey: "key", onTranscript });
    c.start();
    MockWS.instances[0].simulateOpen();
    expect(MockWS.instances[0].sent[0]).toContain("key");
  });

  test("forwards PCM frames as binary", () => {
    const onTranscript = vi.fn();
    const c = new SonioxClient({ apiKey: "key", onTranscript });
    c.start();
    MockWS.instances[0].simulateOpen();
    const pcm = new Uint8Array(3200);
    c.feed(pcm);
    expect(MockWS.instances[0].sent.length).toBeGreaterThan(1);
  });

  test("calls onTranscript on interim message", () => {
    const onTranscript = vi.fn();
    const c = new SonioxClient({ apiKey: "key", onTranscript });
    c.start();
    MockWS.instances[0].simulateOpen();
    MockWS.instances[0].simulateMessage({ tokens: [{ text: "hello", is_final: false }] });
    expect(onTranscript).toHaveBeenCalledWith(
      expect.objectContaining({ interim: "hello", final: "" }),
    );
  });

  test("accumulates final tokens", () => {
    const onTranscript = vi.fn();
    const c = new SonioxClient({ apiKey: "key", onTranscript });
    c.start();
    MockWS.instances[0].simulateOpen();
    MockWS.instances[0].simulateMessage({
      tokens: [
        { text: "hello ", is_final: true },
        { text: "world", is_final: false },
      ],
    });
    expect(onTranscript).toHaveBeenCalledWith(
      expect.objectContaining({ interim: "world", final: "hello " }),
    );
  });
});
