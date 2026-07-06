//! Device-pairing auth.
//!
//! Replaces the old @auth0/auth0-spa-js integration. The PWA runs
//! inside the EvenHub companion app's WebView (loaded from a
//! dynamic 127.0.0.1 port that Auth0 won't wildcard-callback to);
//! instead the user pairs the device from Auris-mobile, which mints
//! a short-lived code, and the PWA exchanges it here for a pair
//! of server-issued tokens.
//!
//! Public surface:
//!   - `initAuth(serverUrl, bridge)` — reads tokens from the host's
//!     persistent KV (with browser localStorage as a dev fallback),
//!     returns an `AuthBundle` ready to drive the WS + REST clients.
//!     `identity === null` means no paired session; the caller
//!     mounts the pair screen.
//!   - `auth.getAccessToken()` — current access token, refreshed
//!     automatically when it's near expiry. Throws `RePairRequired`
//!     when the refresh token is invalid (revoked or unknown) so
//!     the caller can clear UI + route back to pair.
//!   - `auth.redeem(code)` — pair this device by consuming a code
//!     from mobile. Persists tokens; populates `identity`.
//!   - `auth.logout()` — clear stored tokens. Doesn't tell the
//!     server (no remote revoke from a self-paired device).
//!
//! Persistence model: tokens go through `bridge.setLocalStorage` /
//! `bridge.getLocalStorage` — the EvenHub-app-managed KV that
//! survives WebView session resets (every app open is a fresh
//! WebView, with empty `window.localStorage`). We mirror writes to
//! `window.localStorage` so dev / regular-browser sessions still
//! work, and so existing pre-bridge tokens stay readable through
//! one boot after the migration lands.

import type { AuthIdentity } from "./types";
import { resolveDeviceLabel } from "./device-label";

interface KvBridge {
  setLocalStorage(key: string, value: string): Promise<boolean>;
  getLocalStorage(key: string): Promise<string>;
  /// Present on the real EvenHub bridge; used at redeem time to label
  /// the paired device by its glasses serial. Optional so callers/tests
  /// can pass a bare KV stub.
  getDeviceInfo?: () => Promise<{ sn?: string | null; model?: string | null } | null>;
}

/// Storage keys. Namespaced so they don't collide with future
/// settings keys.
const KEY_ACCESS = "auris.access_token";
const KEY_REFRESH = "auris.refresh_token";
const KEY_EXPIRES_AT = "auris.access_expires_at"; // ms-since-epoch
const KEY_DEVICE_ID = "auris.device_id";

/// Refresh window — when the access token has less than this many
/// milliseconds left, getAccessToken transparently swaps it out
/// before handing back. Keeps the WS / REST clients from ever
/// touching a token that's about to fail validation.
const REFRESH_AHEAD_MS = 5 * 60 * 1000;

/// Thrown when the stored refresh token is rejected by the server
/// (revoked device, expired, unknown). The boot path catches this
/// and re-mounts the pair screen with a banner; downstream callers
/// (WS, REST) treat it like any other auth failure.
export class RePairRequired extends Error {
  constructor(message = "Re-pair required") {
    super(message);
    this.name = "RePairRequired";
  }
}

export interface AuthBundle {
  /// Populated when there's a stored access token at boot. `null`
  /// means no paired session — the caller should mount the pair
  /// screen rather than trying to construct WS / REST clients.
  identity: AuthIdentity | null;
  /// Returns a usable access token. Refreshes transparently when
  /// the stored one is within the refresh window. Throws
  /// `RePairRequired` when the refresh is rejected.
  getAccessToken: () => Promise<string>;
  /// Exchange a user-typed (or pasted-from-mobile) pair code for
  /// tokens. Stores tokens + populates the returned identity. The
  /// caller is responsible for then booting the post-auth UI.
  redeem: (code: string) => Promise<AuthIdentity>;
  /// Clear stored tokens. The server isn't notified — that requires
  /// the authed mobile client (the device the user paired *from*)
  /// to call /pair/revoke. Local clearing returns the PWA to the
  /// pair screen.
  logout: () => Promise<void>;
}

/// Browser-localStorage fallback for the bridge KV. Lets `vite dev`
/// (no real bridge) keep working, and lets pre-migration tokens be
/// read once on first post-upgrade boot.
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
    // Storage unavailable / quota — ignore; bridge is canonical anyway.
  }
}
function lsRemove(key: string): void {
  try {
    globalThis.localStorage?.removeItem(key);
  } catch {
    // ditto.
  }
}

/// Decode the unverified payload of a JWT. The server already
/// verified the signature when it minted the token, so we treat
/// the claims as authoritative for display purposes (sub / device_id).
/// Any token-tampering scenario surfaces server-side on the next
/// API call — we never act on these claims for authorization.
function decodeJwtPayload(token: string): Record<string, unknown> | null {
  const parts = token.split(".");
  if (parts.length !== 3) return null;
  try {
    // base64url → base64 → bytes → text → JSON
    const b64 = parts[1].replace(/-/g, "+").replace(/_/g, "/");
    const padded = b64 + "===".slice((b64.length + 3) % 4);
    return JSON.parse(atob(padded)) as Record<string, unknown>;
  } catch {
    return null;
  }
}

/// Build an `AuthIdentity` from an access token's claims. Pair-auth
/// tokens don't carry email / name / picture — we leave those `null`
/// rather than fabricating; settings-modal handles the null case.
function identityFromToken(token: string): AuthIdentity | null {
  const claims = decodeJwtPayload(token);
  if (!claims || typeof claims.sub !== "string") return null;
  return {
    sub: claims.sub,
    email: null,
    name: null,
    picture: null,
  };
}

/// Read one value, preferring the host's KV and falling back to
/// browser localStorage. Used by `loadStoredTokens` to support the
/// pre-bridge → bridge migration without forcing existing dev
/// sessions to re-pair.
async function readKv(bridge: KvBridge, key: string): Promise<string> {
  const fromBridge = await bridge.getLocalStorage(key);
  if (fromBridge) return fromBridge;
  return lsGet(key);
}

/// Write to both the host KV (canonical, survives WebView resets)
/// and browser localStorage (dev fallback, simulator safety net).
async function writeKv(bridge: KvBridge, key: string, value: string): Promise<void> {
  await bridge.setLocalStorage(key, value);
  lsSet(key, value);
}

async function removeKv(bridge: KvBridge, key: string): Promise<void> {
  // Setting empty mirrors the "value missing" semantics that
  // `getLocalStorage` already uses (returns "" for unknown keys).
  await bridge.setLocalStorage(key, "");
  lsRemove(key);
}

/// Read tokens from persistent storage. Returns null when any
/// required piece is missing — partial state is treated as "not
/// paired" rather than trying to recover.
/// Per-key presence of the 4 KV entries that make up a paired session.
/// Surfaced from `loadStoredTokens` so the boot path can log which
/// (if any) entries survived a cold app start — useful for diagnosing
/// the "device asks to re-pair every day" failure mode without
/// leaking token values into logs.
interface StoredTokensWithPresence {
  tokens: {
    access_token: string;
    refresh_token: string;
    expires_at: number;
    device_id: string;
  } | null;
  presence: { access: boolean; refresh: boolean; expires: boolean; device: boolean };
}

async function loadStoredTokens(bridge: KvBridge): Promise<StoredTokensWithPresence> {
  // Serialize bridge reads — per everything-evenhub/glasses-ui Best
  // Practices, all bridge.* calls share a single BLE link to the
  // glasses-side EvenHub app; parallel calls can race or crash the
  // connection. Storage isn't a free local op here.
  const access_token = await readKv(bridge, KEY_ACCESS);
  const refresh_token = await readKv(bridge, KEY_REFRESH);
  const expires_at_raw = await readKv(bridge, KEY_EXPIRES_AT);
  const device_id = await readKv(bridge, KEY_DEVICE_ID);
  const presence = {
    access: Boolean(access_token),
    refresh: Boolean(refresh_token),
    expires: Boolean(expires_at_raw),
    device: Boolean(device_id),
  };
  if (!access_token || !refresh_token || !expires_at_raw || !device_id) {
    return { tokens: null, presence };
  }
  const expires_at = Number.parseInt(expires_at_raw, 10);
  if (!Number.isFinite(expires_at)) return { tokens: null, presence };
  return { tokens: { access_token, refresh_token, expires_at, device_id }, presence };
}

async function persistTokens(
  bridge: KvBridge,
  args: {
    access_token: string;
    refresh_token: string;
    expires_in: number;
    device_id: string;
  },
): Promise<void> {
  // CRITICAL: writes must be sequential. Promise.all here was the
  // root cause of "device asks to re-pair every day" — the 4 parallel
  // setLocalStorage calls share one BLE channel, so they raced and
  // some keys never landed. The next cold boot saw partial state →
  // loadStoredTokens returned null → pair screen. See glasses-ui
  // best practices: "Serialize all bridge calls, not just images."
  const expires_at = Date.now() + args.expires_in * 1000;
  await writeKv(bridge, KEY_ACCESS, args.access_token);
  await writeKv(bridge, KEY_REFRESH, args.refresh_token);
  await writeKv(bridge, KEY_EXPIRES_AT, expires_at.toString(10));
  await writeKv(bridge, KEY_DEVICE_ID, args.device_id);
}

async function clearStoredTokens(bridge: KvBridge): Promise<void> {
  // Serialized for the same reason as persistTokens.
  await removeKv(bridge, KEY_ACCESS);
  await removeKv(bridge, KEY_REFRESH);
  await removeKv(bridge, KEY_EXPIRES_AT);
  await removeKv(bridge, KEY_DEVICE_ID);
}

/// Derive the HTTP origin from the WS server URL. Same trick the
/// REST clients use (meetings-api / artifacts-api) — WS + REST
/// share an axum router on one port.
function deriveHttpBase(serverUrl: string): string {
  try {
    const u = new URL(serverUrl);
    if (u.protocol === "ws:") u.protocol = "http:";
    else if (u.protocol === "wss:") u.protocol = "https:";
    u.pathname = "";
    u.search = "";
    return u.origin;
  } catch {
    // Fall back unchanged; the request will surface a CORS / DNS
    // error which is more diagnostic than us guessing wrong here.
    return serverUrl;
  }
}

/// Initialize the auth bundle. One async storage read at boot to
/// pull tokens out of the host KV. Tokens are exercised lazily
/// when the WS / REST clients first need them; only the initial
/// read happens here.
export async function initAuth(serverUrl: string, bridge: KvBridge): Promise<AuthBundle> {
  const httpBase = deriveHttpBase(serverUrl);
  const { tokens: stored, presence } = await loadStoredTokens(bridge);
  // Diagnostic for the "re-pair every day" investigation. Reports
  // which of the 4 KV keys survived a cold app start without leaking
  // token values — `1111` means all four landed, anything else means
  // the previous session's write was partial (bridge KV race, see
  // persistTokens).
  console.log(
    `[auth] boot kv presence access=${presence.access ? 1 : 0} refresh=${presence.refresh ? 1 : 0} expires=${presence.expires ? 1 : 0} device=${presence.device ? 1 : 0}`,
  );
  let identity: AuthIdentity | null = null;
  if (stored) {
    identity = identityFromToken(stored.access_token);
    // If the access token is so old we can't even read claims, treat
    // as unpaired and clear — keeps boot deterministic.
    if (!identity) await clearStoredTokens(bridge);
  }

  /// Single in-flight refresh promise so concurrent getAccessToken
  /// callers (WS reconnect + REST request happening simultaneously)
  /// don't blow through the refresh-token rotation budget. The
  /// server rotates on every successful refresh, so two parallel
  /// refresh requests would race — one wins, the other becomes
  /// invalid mid-flight.
  let inFlightRefresh: Promise<string> | null = null;

  async function refreshNow(): Promise<string> {
    const { tokens: current } = await loadStoredTokens(bridge);
    if (!current) throw new RePairRequired();
    const resp = await fetch(`${httpBase}/pair/refresh`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ refresh_token: current.refresh_token }),
    });
    if (resp.status === 401) {
      await clearStoredTokens(bridge);
      throw new RePairRequired();
    }
    if (!resp.ok) {
      // Network / 5xx — don't clear tokens; let the caller retry.
      throw new Error(`Refresh failed (HTTP ${resp.status})`);
    }
    const json = (await resp.json()) as {
      access_token: string;
      refresh_token: string;
      expires_in: number;
    };
    await persistTokens(bridge, {
      access_token: json.access_token,
      refresh_token: json.refresh_token,
      expires_in: json.expires_in,
      device_id: current.device_id,
    });
    return json.access_token;
  }

  return {
    identity,
    async getAccessToken() {
      const { tokens: current } = await loadStoredTokens(bridge);
      if (!current) throw new RePairRequired();
      // Plenty of headroom — return as-is.
      if (Date.now() < current.expires_at - REFRESH_AHEAD_MS) {
        return current.access_token;
      }
      // Coalesce concurrent refreshes onto one in-flight call.
      if (!inFlightRefresh) {
        inFlightRefresh = refreshNow().finally(() => {
          inFlightRefresh = null;
        });
      }
      return inFlightRefresh;
    },
    async redeem(code: string) {
      const url = `${httpBase}/pair/redeem`;
      let resp: Response;
      try {
        // Pre-flight log so the Safari Web Inspector (USB-attached
        // to the phone) shows what we're about to send. Cheap, and
        // load-bearing while we're still hunting "Load failed"
        // failures inside the EvenHub WebView.
        // Label the paired device by the glasses' serial ("<serial>
        // (G2)") so it's distinguishable in mobile's paired-devices
        // list. Omitted when no serial is available so the server
        // keeps its "G2 glasses" default.
        const deviceLabel = await resolveDeviceLabel(bridge);
        console.log("[pair] POST", url, { codeLen: code.length, hasLabel: deviceLabel !== null });
        resp = await fetch(url, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(deviceLabel ? { code, device_label: deviceLabel } : { code }),
        });
        console.log("[pair] response", { status: resp.status, ok: resp.ok });
      } catch (e) {
        // WKWebView throws TypeError("Load failed") for any low-level
        // fetch failure (no DNS, TLS, CORS rejection, ATS block,
        // EvenHub permission gate, …). The message alone is useless
        // for diagnosis; dump every property we can reach.
        const err = e as { name?: string; message?: string; cause?: unknown };
        console.warn("[pair] fetch threw", err);
        const detail = [
          err.name ?? "Error",
          err.message ?? String(e),
          err.cause ? `cause=${String(err.cause)}` : null,
        ]
          .filter(Boolean)
          .join(" · ");
        throw new Error(`Couldn't reach ${url} — ${detail}`);
      }
      if (!resp.ok) {
        // 400 is the common case (bad / expired / used code); the
        // server maps all of those to "invalid_code" without
        // distinguishing. Surface as a typed error for the UI.
        let detail = `Pair failed (HTTP ${resp.status})`;
        try {
          const body = (await resp.json()) as { error?: string; detail?: string };
          if (body.detail) detail = body.detail;
          else if (body.error) detail = body.error;
        } catch {
          // Body wasn't JSON — keep the HTTP-status default.
        }
        throw new Error(detail);
      }
      const json = (await resp.json()) as {
        access_token: string;
        refresh_token: string;
        device_id: string;
        expires_in: number;
      };
      await persistTokens(bridge, json);
      const newIdentity = identityFromToken(json.access_token);
      if (!newIdentity) {
        // Server returned a JWT we can't parse — shouldn't happen,
        // but bail loudly rather than masquerade as authenticated.
        await clearStoredTokens(bridge);
        throw new Error("Server returned a malformed access token.");
      }
      identity = newIdentity;
      return newIdentity;
    },
    async logout() {
      await clearStoredTokens(bridge);
      identity = null;
    },
  };
}
