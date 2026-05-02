import { describe, expect, test, vi, beforeEach, afterEach } from "vitest";
import { ReconnectingSocket } from "./ws";
import type { Intent, ServerEvent } from "./types";

class MockWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;

  static instances: MockWebSocket[] = [];
  readyState = MockWebSocket.CONNECTING;
  onopen: ((e: Event) => void) | null = null;
  onmessage: ((e: MessageEvent) => void) | null = null;
  onclose: ((e: CloseEvent) => void) | null = null;
  onerror: ((e: Event) => void) | null = null;
  sent: string[] = [];

  constructor(public url: string) {
    MockWebSocket.instances.push(this);
  }

  send(data: string) {
    this.sent.push(data);
  }

  close() {
    this.readyState = MockWebSocket.CLOSED;
    this.onclose?.(new CloseEvent("close"));
  }

  // Test helpers
  simulateOpen() {
    this.readyState = MockWebSocket.OPEN;
    this.onopen?.(new Event("open"));
  }
  simulateMessage(data: unknown) {
    this.onmessage?.(new MessageEvent("message", { data: JSON.stringify(data) }));
  }
  simulateClose() {
    this.readyState = MockWebSocket.CLOSED;
    this.onclose?.(new CloseEvent("close"));
  }
}

describe("ReconnectingSocket", () => {
  let originalWebSocket: typeof globalThis.WebSocket;

  beforeEach(() => {
    MockWebSocket.instances = [];
    originalWebSocket = globalThis.WebSocket;
    (globalThis as any).WebSocket = MockWebSocket;
    vi.useFakeTimers();
  });

  afterEach(() => {
    (globalThis as any).WebSocket = originalWebSocket;
    vi.useRealTimers();
  });

  test("connects with token in URL", () => {
    const sock = new ReconnectingSocket({
      url: "ws://laptop:7331",
      token: "tok",
      onEvent: vi.fn(),
      onStatus: vi.fn(),
    });
    expect(MockWebSocket.instances[0].url).toBe("ws://laptop:7331/?token=tok");
    sock.close();
  });

  test("onStatus reports open after WebSocket opens", () => {
    const onStatus = vi.fn();
    new ReconnectingSocket({
      url: "ws://laptop:7331",
      token: "tok",
      onEvent: vi.fn(),
      onStatus,
    });
    MockWebSocket.instances[0].simulateOpen();
    expect(onStatus).toHaveBeenCalledWith("open");
  });

  test("forwards messages to onEvent", () => {
    const onEvent = vi.fn();
    new ReconnectingSocket({
      url: "ws://laptop:7331",
      token: "tok",
      onEvent,
      onStatus: vi.fn(),
    });
    MockWebSocket.instances[0].simulateOpen();
    const evt: ServerEvent = { type: "status", status: { listening: false, paused: false } };
    MockWebSocket.instances[0].simulateMessage(evt);
    expect(onEvent).toHaveBeenCalledWith(evt);
  });

  test("queues sends while not open, drains on open", () => {
    const sock = new ReconnectingSocket({
      url: "ws://laptop:7331",
      token: "tok",
      onEvent: vi.fn(),
      onStatus: vi.fn(),
    });
    const intent: Intent = { type: "stop_meeting" };
    sock.send(intent);
    expect(MockWebSocket.instances[0].sent).toEqual([]);
    MockWebSocket.instances[0].simulateOpen();
    expect(MockWebSocket.instances[0].sent).toEqual([JSON.stringify(intent)]);
  });

  test("reconnects with backoff after close", () => {
    new ReconnectingSocket({
      url: "ws://laptop:7331",
      token: "tok",
      onEvent: vi.fn(),
      onStatus: vi.fn(),
    });
    MockWebSocket.instances[0].simulateOpen();
    MockWebSocket.instances[0].simulateClose();
    expect(MockWebSocket.instances).toHaveLength(1);
    vi.advanceTimersByTime(1500); // initial delay 1000ms + jitter
    expect(MockWebSocket.instances).toHaveLength(2);
  });

  test("heartbeat-loss triggers reconnect", () => {
    new ReconnectingSocket({
      url: "ws://laptop:7331",
      token: "tok",
      onEvent: vi.fn(),
      onStatus: vi.fn(),
    });
    MockWebSocket.instances[0].simulateOpen();
    vi.advanceTimersByTime(26_000); // exceeds 25s heartbeat-loss threshold
    expect(MockWebSocket.instances[0].readyState).toBe(MockWebSocket.CLOSED);
  });

  test("any inbound message resets heartbeat-loss timer", () => {
    new ReconnectingSocket({
      url: "ws://laptop:7331",
      token: "tok",
      onEvent: vi.fn(),
      onStatus: vi.fn(),
    });
    MockWebSocket.instances[0].simulateOpen();
    vi.advanceTimersByTime(20_000);
    MockWebSocket.instances[0].simulateMessage({
      type: "status",
      status: { listening: false, paused: false },
    });
    vi.advanceTimersByTime(20_000); // 40s total but reset at 20s; well under 25s from reset
    expect(MockWebSocket.instances[0].readyState).toBe(MockWebSocket.OPEN);
  });

  test("close stops further reconnection", () => {
    const sock = new ReconnectingSocket({
      url: "ws://laptop:7331",
      token: "tok",
      onEvent: vi.fn(),
      onStatus: vi.fn(),
    });
    sock.close();
    MockWebSocket.instances[0].simulateClose();
    vi.advanceTimersByTime(60_000);
    expect(MockWebSocket.instances).toHaveLength(1);
  });
});
