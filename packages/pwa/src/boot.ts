import { loadSettings } from "./storage";
import type { Store } from "./store";
import { paintAurisMark } from "./glasses/auris-mark-bitmap";
import { buildEntryLayout } from "./glasses/layout-entry";
import { buildUnpairedLayout } from "./glasses/layout-unpaired";
import { firstEnabledGlassesMode } from "./input/gesture-router";

interface BridgeLike {
  setLocalStorage(key: string, value: string): Promise<boolean>;
  getLocalStorage(key: string): Promise<string>;
  createStartUpPageContainer(container: unknown): Promise<number>;
  /// Used to populate the unpaired layout's logo image container.
  /// Calls to this SDK method must be serial — we only invoke it
  /// once at boot, so no queueing is required here.
  updateImageRawData(data: unknown): Promise<unknown>;
  onEvenHubEvent(cb: (e: unknown) => void): () => void;
  onDeviceStatusChanged(cb: (s: unknown) => void): () => void;
  onLaunchSource?(cb: (s: "appMenu" | "glassesMenu") => void): () => void;
}

interface BootOptions {
  bridge: BridgeLike;
  store: Store;
  env: Record<string, string | undefined>;
  /// Picks the initial glasses page container. Paired users see the
  /// "⌁ Ready" idle layout immediately; unpaired users see a brand +
  /// pair-prompt layout so a user wearing the glasses understands
  /// why nothing's happening. main.ts decides by reading tokens
  /// from localStorage before boot runs.
  isPaired: boolean;
}

const STARTUP_RESULT_NAMES: Record<number, string> = {
  0: "success",
  1: "invalid",
  2: "oversize",
  3: "outOfMemory",
};

export async function boot({ bridge, store, env, isPaired }: BootOptions): Promise<void> {
  // 1. Subscribe to launch source first (single-fire event).
  bridge.onLaunchSource?.((_source) => {
    // Phase 0 doesn't branch on launch source; just observe.
  });

  // 2. Load settings.
  const settings = await loadSettings(bridge, env);
  // Reconcile the initial glasses mode against the user's per-mode
  // opt-outs (Settings → Glasses display). `defaultAppState` lands
  // on "transcript" — fine for the first-ever launch, but if the
  // user has opted out of transcript we'd render it once at meeting
  // start before the cycle could escape. `availableModes` is still
  // empty here (pre-handshake), so `firstEnabledGlassesMode` uses
  // its static-order fallback.
  const initialGlassesMode = firstEnabledGlassesMode([], settings.glassesModes);
  store.update({ settings, glassesCurrentMode: initialGlassesMode });

  // 3. Create the initial startup page container. Paired users see
  // the "⌁ Ready" idle screen immediately; unpaired users see a
  // pair-prompt layout. main.ts flips the container to idle the
  // moment a redeem succeeds (no full app reload needed).
  const initialLayout = isPaired ? buildEntryLayout() : buildUnpairedLayout();
  const result = await bridge.createStartUpPageContainer(initialLayout);
  // Latch the bridge-ready signal so main.ts can decide whether to
  // advertise `audio_capture` to the server. A non-zero startup
  // result (prototype mode without glasses) keeps the flag false.
  store.update({ glassesBridgeReady: result === 0 });
  if (result !== 0) {
    const reason = STARTUP_RESULT_NAMES[result] ?? `code ${result}`;
    // In dev (prototype mode), the companion app's bridge may not fully
    // forward this call to real glasses. Degrade to a dismissable warning
    // so the phone-UI dev loop isn't blocked — production keeps the hard
    // fail because no-glasses-display = unusable app.
    if (import.meta.env.DEV) {
      console.warn(`[boot] createStartUpPageContainer returned ${reason} — continuing in dev`);
      store.update({
        errorOverlay: {
          title: "Failed to initialize glasses display (dev)",
          message: `createStartUpPageContainer returned ${reason}. Likely a prototype-mode limitation — phone UI will still work; tap to dismiss.`,
          dismissable: true,
        },
      });
    } else {
      store.update({
        errorOverlay: {
          title: "Failed to initialize glasses display",
          message: `createStartUpPageContainer returned ${reason}. Please file a bug.`,
          dismissable: false,
        },
      });
      return;
    }
  }

  // 3a. Populate the brand-mark image container. Only the unpaired
  // splash uses the image; the paired entry layout has no image
  // container, so this call is a no-op there. Drawn at runtime via
  // Canvas2D so the bundle doesn't ship a baked PNG just for the
  // splash. Failures degrade to a logoless splash — see
  // `paintAurisMark`.
  if (!isPaired) {
    await paintAurisMark(bridge);
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
