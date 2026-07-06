import type { Settings } from "./types";

interface BridgeLike {
  setLocalStorage(key: string, value: string): Promise<boolean>;
  getLocalStorage(key: string): Promise<string>;
}

const KEYS = {
  serverToken: "mc.serverToken",
  lastMetadata: "mc.lastMetadata",
  glassesModes: "mc.glassesModes",
} as const;

/// Storage slot for the stable device id. Kept out of `KEYS`/`Settings`
/// because it isn't a user-facing setting — it's machine identity that
/// must persist across reconnects so the server keeps our audio-source
/// binding when the network flips (wifi→5G). See `getOrCreateDeviceId`.
const DEVICE_ID_KEY = "mc.deviceId";

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
  const [bridgeToken, bridgeMeta, bridgeGlasses] = await Promise.all([
    bridge.getLocalStorage(KEYS.serverToken),
    bridge.getLocalStorage(KEYS.lastMetadata),
    bridge.getLocalStorage(KEYS.glassesModes),
  ]);

  // Prefer bridge value; fall back to browser localStorage if bridge returns empty.
  const token = bridgeToken || lsGet(KEYS.serverToken);
  const meta = bridgeMeta || lsGet(KEYS.lastMetadata);
  const glasses = bridgeGlasses || lsGet(KEYS.glassesModes);

  return {
    serverToken: token || env[ENV_KEYS.serverToken!] || "",
    lastMetadata: parseRecord(meta, isString),
    glassesModes: parseRecord(glasses, isBoolean),
  };
}

/// Parse a JSON-string stored under one of the KEYS slots into a
/// flat `Record<string, T>`. Tolerates absence, malformed JSON, and
/// values of the wrong shape — returns `{}` in all failure modes
/// rather than throwing, since corrupt local storage shouldn't
/// strand the app. `validate` filters per-value type so a partially-
/// corrupt blob doesn't leak unsafe values into the rest of the app.
function parseRecord<T>(raw: string, validate: (v: unknown) => v is T): Record<string, T> {
  if (!raw) return {};
  try {
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out: Record<string, T> = {};
    for (const [k, v] of Object.entries(parsed)) {
      if (validate(v)) out[k] = v;
    }
    return out;
  } catch {
    return {};
  }
}

function isString(v: unknown): v is string {
  return typeof v === "string";
}

function isBoolean(v: unknown): v is boolean {
  return typeof v === "boolean";
}

/// Return this client's stable device id, reading the persisted value
/// or generating + persisting a fresh one. Written to both the bridge
/// KV (canonical on real glasses) and browser localStorage (simulator
/// safety-net), mirroring `makeStorage`. The id is sent on every
/// `register_device` so the server reuses our identity across
/// reconnects — without it, each reconnect mints a fresh server-side
/// device id, which breaks the audio-source binding and silently stops
/// recording after a network switch.
///
/// Callers should memoize the returned promise for the session (see
/// main.ts) so concurrent first-time calls share one generation rather
/// than racing to mint two ids before either persists.
export async function getOrCreateDeviceId(bridge: BridgeLike): Promise<string> {
  const existing = (await bridge.getLocalStorage(DEVICE_ID_KEY)) || lsGet(DEVICE_ID_KEY);
  if (existing) return existing;
  const id = generateUuid();
  await bridge.setLocalStorage(DEVICE_ID_KEY, id);
  lsSet(DEVICE_ID_KEY, id);
  return id;
}

/// `crypto.randomUUID` where available (all EvenHub WebViews + modern
/// browsers); a v4-shaped fallback otherwise so the id is still
/// well-formed in degraded/test environments.
function generateUuid(): string {
  const c = globalThis.crypto;
  if (c && typeof c.randomUUID === "function") return c.randomUUID();
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (ch) => {
    const r = (Math.random() * 16) | 0;
    const v = ch === "x" ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
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
