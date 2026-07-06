import { describe, expect, test, vi, beforeEach, afterEach } from "vitest";
import { GlassesAudioSource } from "./glasses-audio-source";
import { createStore } from "./store";
import { defaultAppState } from "./types";

class MockWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;

  static instances: MockWebSocket[] = [];
  readyState = MockWebSocket.CONNECTING;
  binaryType: BinaryType = "blob";
  onopen: ((e: Event) => void) | null = null;
  onclose: ((e: CloseEvent) => void) | null = null;
  onerror: ((e: Event) => void) | null = null;
  sent: ArrayBuffer[] = [];

  constructor(public url: string) {
    MockWebSocket.instances.push(this);
  }

  send(data: ArrayBuffer) {
    this.sent.push(data);
  }

  close() {
    this.readyState = MockWebSocket.CLOSED;
    this.onclose?.(new CloseEvent("close"));
  }

  simulateOpen() {
    this.readyState = MockWebSocket.OPEN;
    this.onopen?.(new Event("open"));
  }

  /// Simulate an unexpected network close — what happens on wifi
  /// blip, browser tab background-throttle kill, etc.
  simulateNetworkClose() {
    this.readyState = MockWebSocket.CLOSED;
    this.onclose?.(new CloseEvent("close"));
  }
}

function mockBridge() {
  return { audioControl: vi.fn(async (_open: boolean) => true) };
}

describe("GlassesAudioSource", () => {
  let originalWebSocket: typeof globalThis.WebSocket;

  beforeEach(() => {
    MockWebSocket.instances = [];
    originalWebSocket = globalThis.WebSocket;
    (globalThis as any).WebSocket = MockWebSocket;
  });

  afterEach(() => {
    (globalThis as any).WebSocket = originalWebSocket;
    vi.useRealTimers();
  });

  test("start() dials /audio with the token in the query string", async () => {
    const bridge = mockBridge();
    const store = createStore(defaultAppState());
    const src = new GlassesAudioSource(bridge, store, {
      getServerUrl: () => "ws://laptop:7331",
      getAccessToken: async () => "tok-123",
    });
    await src.start();
    expect(MockWebSocket.instances).toHaveLength(1);
    expect(MockWebSocket.instances[0].url).toBe("ws://laptop:7331/audio?token=tok-123");
  });

  test("mic stays OFF until the socket actually opens", async () => {
    // Frames sent before the WS handshake completes would otherwise
    // be dropped by the browser — turning the mic on too early just
    // produces a brief silent preamble on the server.
    const bridge = mockBridge();
    const store = createStore(defaultAppState());
    const src = new GlassesAudioSource(bridge, store, {
      getServerUrl: () => "ws://laptop:7331",
      getAccessToken: async () => "tok",
    });
    await src.start();
    expect(bridge.audioControl).not.toHaveBeenCalled();
    MockWebSocket.instances[0].simulateOpen();
    expect(bridge.audioControl).toHaveBeenCalledWith(true);
  });

  test("feed() forwards PCM as binary frames once OPEN", async () => {
    const bridge = mockBridge();
    const store = createStore(defaultAppState());
    const src = new GlassesAudioSource(bridge, store, {
      getServerUrl: () => "ws://laptop:7331",
      getAccessToken: async () => "tok",
    });
    await src.start();
    const ws = MockWebSocket.instances[0];
    // Pre-open feed must be dropped to avoid send-on-CONNECTING errors.
    src.feed(new Uint8Array([1, 2, 3, 4]));
    expect(ws.sent).toEqual([]);
    ws.simulateOpen();
    src.feed(new Uint8Array([5, 6, 7, 8]));
    expect(ws.sent).toHaveLength(1);
    expect(new Uint8Array(ws.sent[0])).toEqual(new Uint8Array([5, 6, 7, 8]));
  });

  test("stop() closes the socket and turns the mic off", async () => {
    const bridge = mockBridge();
    const store = createStore(defaultAppState());
    const src = new GlassesAudioSource(bridge, store, {
      getServerUrl: () => "ws://laptop:7331",
      getAccessToken: async () => "tok",
    });
    await src.start();
    MockWebSocket.instances[0].simulateOpen();
    await src.stop();
    expect(MockWebSocket.instances[0].readyState).toBe(MockWebSocket.CLOSED);
    expect(bridge.audioControl).toHaveBeenLastCalledWith(false);
  });

  test("re-entrant start() does not open a second socket", async () => {
    // Two state-change ticks firing back-to-back must converge on
    // one connection — opening two would race over the same /audio
    // slot on the server.
    const bridge = mockBridge();
    const store = createStore(defaultAppState());
    const src = new GlassesAudioSource(bridge, store, {
      getServerUrl: () => "ws://laptop:7331",
      getAccessToken: async () => "tok",
    });
    await Promise.all([src.start(), src.start()]);
    expect(MockWebSocket.instances).toHaveLength(1);
  });

  test("start() publishes connecting → streaming on open", async () => {
    const bridge = mockBridge();
    const store = createStore(defaultAppState());
    const src = new GlassesAudioSource(bridge, store, {
      getServerUrl: () => "ws://laptop:7331",
      getAccessToken: async () => "tok",
    });
    await src.start();
    expect(store.get().audioCaptureState.kind).toBe("connecting");
    MockWebSocket.instances[0].simulateOpen();
    expect(store.get().audioCaptureState.kind).toBe("streaming");
  });

  test("unexpected close schedules a reconnect with backoff", async () => {
    // The original bug: WS reset → audio stayed dead because the
    // main.ts reactor only fires on bind-state changes, not on
    // socket close. The state machine has to recover itself.
    vi.useFakeTimers();
    const bridge = mockBridge();
    const store = createStore(defaultAppState());
    const src = new GlassesAudioSource(bridge, store, {
      getServerUrl: () => "ws://laptop:7331",
      getAccessToken: async () => "tok",
    });
    await src.start();
    MockWebSocket.instances[0].simulateOpen();
    expect(store.get().audioCaptureState.kind).toBe("streaming");

    MockWebSocket.instances[0].simulateNetworkClose();
    expect(store.get().audioCaptureState.kind).toBe("reconnecting");
    // Initial backoff is 500ms. Advance just shy — no new socket yet.
    await vi.advanceTimersByTimeAsync(499);
    expect(MockWebSocket.instances).toHaveLength(1);
    // Cross the threshold — second socket opens.
    await vi.advanceTimersByTimeAsync(2);
    expect(MockWebSocket.instances).toHaveLength(2);
    expect(store.get().audioCaptureState.kind).toBe("connecting");
  });

  test("successful reconnect resets the backoff ladder", async () => {
    // Multi-flap scenario: blip → recover → blip. The second blip
    // should restart at 500ms, not at whatever attempt the first
    // sequence reached.
    vi.useFakeTimers();
    const bridge = mockBridge();
    const store = createStore(defaultAppState());
    const src = new GlassesAudioSource(bridge, store, {
      getServerUrl: () => "ws://laptop:7331",
      getAccessToken: async () => "tok",
    });
    await src.start();
    MockWebSocket.instances[0].simulateOpen();

    // First blip → reconnect → recover.
    MockWebSocket.instances[0].simulateNetworkClose();
    await vi.advanceTimersByTimeAsync(500);
    MockWebSocket.instances[1].simulateOpen();
    expect(store.get().audioCaptureState.kind).toBe("streaming");

    // Second blip → backoff restarts at 500ms (not 1000ms).
    MockWebSocket.instances[1].simulateNetworkClose();
    await vi.advanceTimersByTimeAsync(499);
    expect(MockWebSocket.instances).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(2);
    expect(MockWebSocket.instances).toHaveLength(3);
  });

  test("stop() during reconnecting cancels the pending retry", async () => {
    // User pressed Stop while we were in backoff — no zombie socket
    // should open after the user's intent moved on.
    vi.useFakeTimers();
    const bridge = mockBridge();
    const store = createStore(defaultAppState());
    const src = new GlassesAudioSource(bridge, store, {
      getServerUrl: () => "ws://laptop:7331",
      getAccessToken: async () => "tok",
    });
    await src.start();
    MockWebSocket.instances[0].simulateOpen();
    MockWebSocket.instances[0].simulateNetworkClose();
    expect(store.get().audioCaptureState.kind).toBe("reconnecting");

    await src.stop();
    expect(store.get().audioCaptureState.kind).toBe("idle");

    // Advance well past any backoff — no new socket must open.
    await vi.advanceTimersByTimeAsync(20_000);
    expect(MockWebSocket.instances).toHaveLength(1);
  });

  test("token-fetch failure is retryable, not terminal", async () => {
    // A failed token fetch during reconnect almost always means the
    // network is down (wifi→5G, sleep/wake) — exactly when we must
    // keep trying. It must NOT strand the ladder in `failed`.
    vi.useFakeTimers();
    const bridge = mockBridge();
    const store = createStore(defaultAppState());
    let throwToken = true;
    const src = new GlassesAudioSource(bridge, store, {
      getServerUrl: () => "ws://laptop:7331",
      getAccessToken: async () => {
        if (throwToken) throw new Error("network down");
        return "tok";
      },
    });
    await src.start();
    // Token threw → we're reconnecting, not failed.
    expect(store.get().audioCaptureState.kind).toBe("reconnecting");
    expect(MockWebSocket.instances).toHaveLength(0);

    // Network is still down across the first backoff — keeps retrying.
    await vi.advanceTimersByTimeAsync(500);
    expect(store.get().audioCaptureState.kind).toBe("reconnecting");
    expect(MockWebSocket.instances).toHaveLength(0);

    // Network returns: the next scheduled attempt opens a socket.
    throwToken = false;
    await vi.advanceTimersByTimeAsync(1000);
    expect(MockWebSocket.instances).toHaveLength(1);
    MockWebSocket.instances[0].simulateOpen();
    expect(store.get().audioCaptureState.kind).toBe("streaming");
  });

  test("missing server URL is terminal (config error, not network)", async () => {
    vi.useFakeTimers();
    const bridge = mockBridge();
    const store = createStore(defaultAppState());
    const src = new GlassesAudioSource(bridge, store, {
      getServerUrl: () => "",
      getAccessToken: async () => "tok",
    });
    await src.start();
    expect(store.get().audioCaptureState.kind).toBe("failed");
    await vi.advanceTimersByTimeAsync(20_000);
    expect(MockWebSocket.instances).toHaveLength(0);
  });
});
