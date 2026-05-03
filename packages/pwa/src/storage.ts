import type { Settings } from "./types";

interface BridgeLike {
  setLocalStorage(key: string, value: string): Promise<boolean>;
  getLocalStorage(key: string): Promise<string>;
}

const KEYS = {
  serverUrl: "mc.serverUrl",
  serverToken: "mc.serverToken",
  sonioxKey: "mc.sonioxKey",
  lastMetadata: "mc.lastMetadata",
} as const;

type StorageKey = keyof typeof KEYS;

const ENV_KEYS: Partial<Record<StorageKey, string>> = {
  serverUrl: "VITE_DEFAULT_SERVER_URL",
  serverToken: "VITE_DEFAULT_SERVER_TOKEN",
  sonioxKey: "VITE_DEFAULT_SONIOX_KEY",
};

export async function loadSettings(
  bridge: BridgeLike,
  env: Record<string, string | undefined>,
): Promise<Settings> {
  const [url, token, key, meta] = await Promise.all([
    bridge.getLocalStorage(KEYS.serverUrl),
    bridge.getLocalStorage(KEYS.serverToken),
    bridge.getLocalStorage(KEYS.sonioxKey),
    bridge.getLocalStorage(KEYS.lastMetadata),
  ]);

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
    serverUrl: url || env[ENV_KEYS.serverUrl!] || "ws://localhost:7331",
    serverToken: token || env[ENV_KEYS.serverToken!] || "",
    sonioxKey: key || env[ENV_KEYS.sonioxKey!] || "",
    lastMetadata,
  };
}

export function makeStorage(bridge: BridgeLike) {
  return {
    async set<K extends StorageKey>(key: K, value: Settings[K]): Promise<void> {
      const raw = typeof value === "string" ? value : JSON.stringify(value);
      await bridge.setLocalStorage(KEYS[key], raw);
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
