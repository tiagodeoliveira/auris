import { vi } from "vitest";

export type BridgeEvent = unknown; // refined when stt + input modules need it

export interface MockBridgeStorage {
  [key: string]: string;
}

export function createMockBridge() {
  const storage: MockBridgeStorage = {};
  const eventListeners: Array<(e: BridgeEvent) => void> = [];
  const deviceListeners: Array<(s: unknown) => void> = [];
  const launchSourceListeners: Array<(s: "appMenu" | "glassesMenu") => void> = [];

  return {
    storage,
    eventListeners,
    deviceListeners,
    launchSourceListeners,

    setLocalStorage: vi.fn(async (key: string, value: string) => {
      storage[key] = value;
      return true;
    }),
    getLocalStorage: vi.fn(async (key: string) => storage[key] ?? ""),
    audioControl: vi.fn(async (_open: boolean) => true),
    createStartUpPageContainer: vi.fn(async () => 0),
    rebuildPageContainer: vi.fn(async () => true),
    textContainerUpgrade: vi.fn(async () => true),
    updateImageRawData: vi.fn(async () => "success"),
    shutDownPageContainer: vi.fn(async () => true),
    onEvenHubEvent: vi.fn((cb: (e: BridgeEvent) => void) => {
      eventListeners.push(cb);
      return () => {
        const i = eventListeners.indexOf(cb);
        if (i >= 0) eventListeners.splice(i, 1);
      };
    }),
    onDeviceStatusChanged: vi.fn((cb: (s: unknown) => void) => {
      deviceListeners.push(cb);
      return () => {
        const i = deviceListeners.indexOf(cb);
        if (i >= 0) deviceListeners.splice(i, 1);
      };
    }),
    onLaunchSource: vi.fn((cb: (s: "appMenu" | "glassesMenu") => void) => {
      launchSourceListeners.push(cb);
      return () => {
        const i = launchSourceListeners.indexOf(cb);
        if (i >= 0) launchSourceListeners.splice(i, 1);
      };
    }),

    // Test helpers
    simulateEvent(e: BridgeEvent) {
      for (const cb of eventListeners) cb(e);
    },
  };
}

export type MockBridge = ReturnType<typeof createMockBridge>;
