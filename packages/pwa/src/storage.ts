import type { Settings } from "./types";

interface BridgeLike {
  setLocalStorage(key: string, value: string): Promise<boolean>;
  getLocalStorage(key: string): Promise<string>;
}

const KEYS = {
  serverToken: "mc.serverToken",
  lastMetadata: "mc.lastMetadata",
} as const;

type StorageKey = keyof typeof KEYS;

const ENV_KEYS: Partial<Record<StorageKey, string>> = {
  serverToken: "VITE_DEFAULT_SERVER_TOKEN",
};

// Browser localStorage fallback. The Even Hub `bridge.setLocalStorage`
// is the canonical persistence layer on real glasses, but the simulator
// can be inconsistent across page refreshes / process restarts. Writing
// to both means the worst case is an extra few bytes in browser storage;
// the best case is settings survive when bridge state doesn't.
function lsGet(key: string): string {
  try {
    return globalThis.localStorage?.getItem(key) ?? "";
  } catch {
    return "";
  }
}
function lsSet(key: string, value: string): void {
  try {
    globalThis.localStorage?.setItem(key, value);
  } catch {
    // Storage unavailable / quota / etc. — ignore; bridge is canonical anyway.
  }
}

export async function loadSettings(
  bridge: BridgeLike,
  env: Record<string, string | undefined>,
): Promise<Settings> {
  const [bridgeToken, bridgeMeta] = await Promise.all([
    bridge.getLocalStorage(KEYS.serverToken),
    bridge.getLocalStorage(KEYS.lastMetadata),
  ]);

  // Prefer bridge value; fall back to browser localStorage if bridge returns empty.
  const token = bridgeToken || lsGet(KEYS.serverToken);
  const meta = bridgeMeta || lsGet(KEYS.lastMetadata);

  let lastMetadata: Record<string, string> = {};
  if (meta) {
    try {
      const parsed = JSON.parse(meta);
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        lastMetadata = parsed as Record<string, string>;
      }
    } catch {
      // Malformed JSON; ignore. Future writes will overwrite.
    }
  }

  return {
    serverToken: token || env[ENV_KEYS.serverToken!] || "",
    lastMetadata,
  };
}

export function makeStorage(bridge: BridgeLike) {
  return {
    async set<K extends StorageKey>(key: K, value: Settings[K]): Promise<void> {
      const raw = typeof value === "string" ? value : JSON.stringify(value);
      // Write to both layers. Bridge is canonical; localStorage is the
      // simulator-safety net. We don't await localStorage (it's sync).
      await bridge.setLocalStorage(KEYS[key], raw);
      lsSet(KEYS[key], raw);
    },
  };
}

export async function saveSetting<K extends StorageKey>(
  bridge: BridgeLike,
  key: K,
  value: Settings[K],
): Promise<void> {
  await makeStorage(bridge).set(key, value);
}
