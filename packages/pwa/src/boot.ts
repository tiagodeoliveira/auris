import { loadSettings } from "./storage";
import type { Store } from "./store";
import { buildIdleLayout } from "./glasses/layout-idle";

interface BridgeLike {
  setLocalStorage(key: string, value: string): Promise<boolean>;
  getLocalStorage(key: string): Promise<string>;
  createStartUpPageContainer(container: unknown): Promise<number>;
  onEvenHubEvent(cb: (e: unknown) => void): () => void;
  onDeviceStatusChanged(cb: (s: unknown) => void): () => void;
  onLaunchSource?(cb: (s: "appMenu" | "glassesMenu") => void): () => void;
}

interface BootOptions {
  bridge: BridgeLike;
  store: Store;
  env: Record<string, string | undefined>;
}

const STARTUP_RESULT_NAMES: Record<number, string> = {
  0: "success",
  1: "invalid",
  2: "oversize",
  3: "outOfMemory",
};

export async function boot({ bridge, store, env }: BootOptions): Promise<void> {
  // 1. Subscribe to launch source first (single-fire event).
  bridge.onLaunchSource?.((_source) => {
    // Phase 0 doesn't branch on launch source; just observe.
  });

  // 2. Load settings.
  const settings = await loadSettings(bridge, env);
  store.update({ settings });

  // 3. Create startup page container with placeholder Layout A content.
  // (Real Layout A renders in Task 7.)
  const result = await bridge.createStartUpPageContainer(buildIdleLayout());
  if (result !== 0) {
    store.update({
      errorOverlay: {
        title: "Failed to initialize glasses display",
        message: `createStartUpPageContainer returned ${STARTUP_RESULT_NAMES[result] ?? `code ${result}`}. Please file a bug.`,
        dismissable: false,
      },
    });
    return;
  }

  // 4. Subscribe to bridge events.
  // Real event routing is wired in main.ts via handleBridgeEvent + handleLifecycleEvent.
  bridge.onDeviceStatusChanged((_status) => {
    // Status reflection lands in later tasks.
  });

  // (Auth0 login is now the gating step at first run; no token-prompt
  // needed here. main.ts renders the login screen when there's no
  // active session.)
}
