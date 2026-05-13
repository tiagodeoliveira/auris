// Auth0 native PKCE client. Mirrors the Mac app's Auth0Client.swift
// in shape and storage discipline:
//
//   - signIn() opens the system browser for the Auth0 universal
//     login → handles the deep-link callback → exchanges the auth
//     code for access + refresh tokens.
//   - Refresh token lives in expo-secure-store (Keychain on iOS,
//     EncryptedSharedPreferences on Android). Access token stays
//     in memory only.
//   - getAccessToken() returns a non-expired access token, silently
//     refreshing via the refresh-token grant if the cached one is
//     stale. Same shape the WS client + REST clients consume via
//     `tokenProvider`.
//   - signOut() clears the refresh token and resolves the cached
//     identity to null. The user has to sign in again afterward.
//
// Auth0 dashboard config:
//   - Application type: Native
//   - Allowed Callback URLs: must include `auris://callback`
//     (matches the Mac app — we share the Auth0 application
//     between both clients).
//   - Allowed Logout URLs: same scheme.
//   - Grants: must include "Refresh Token" + "Authorization Code".
//   - API → Allow Offline Access: must be on (server-side; that's
//     what unlocks issuing refresh tokens to native clients).

import * as AuthSession from "expo-auth-session";
import * as SecureStore from "expo-secure-store";
import * as WebBrowser from "expo-web-browser";

import { auth0Config, auth0Configured } from "../config";

// Required for the auth callback to dismiss the in-app browser
// reliably on iOS. Calling once at module load is the documented
// pattern. No-op on Android.
WebBrowser.maybeCompleteAuthSession();

const REFRESH_TOKEN_KEY = "auris.auth.refreshToken";
const REDIRECT_URI = "auris://callback";
const SCOPES = ["openid", "profile", "email", "offline_access"];

/// Identity surfaced after a successful sign-in or refresh. Mirrors
/// the Mac client's `Identity` shape; populated from the ID-token's
/// `sub` (subject) and the userinfo endpoint when we need name/email.
export interface Identity {
  sub: string;
  email?: string;
  name?: string;
}

interface CachedAccessToken {
  token: string;
  /// Unix epoch ms; we refresh ~30s before this to avoid mid-request
  /// expirations.
  expiresAt: number;
}

interface AuthState {
  identity: Identity | null;
  cached: CachedAccessToken | null;
}

const state: AuthState = {
  identity: null,
  cached: null,
};

/// Listeners notified on signed-in / signed-out transitions. The
/// store subscribes to this so React can re-render the auth gate.
type Listener = (identity: Identity | null) => void;
const listeners = new Set<Listener>();

function notify(): void {
  for (const l of listeners) l(state.identity);
}

export function subscribe(l: Listener): () => void {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}

/// Re-discovery is cheap (one HTTP fetch) but cached for the
/// process lifetime — Auth0's discovery doc doesn't change without
/// tenant config changes.
let discoveryCache: AuthSession.DiscoveryDocument | null = null;
async function discovery(): Promise<AuthSession.DiscoveryDocument> {
  if (discoveryCache) return discoveryCache;
  if (!auth0Configured) {
    throw new Error("Auth0 not configured — set the EXPO_PUBLIC_AUTH0_* env vars.");
  }
  const issuer = `https://${auth0Config.domain}`;
  const result = await AuthSession.fetchDiscoveryAsync(issuer);
  discoveryCache = result;
  return result;
}

/// Boot path: re-load the refresh token from secure storage and try
/// to silently get a fresh access token. If that succeeds we're
/// signed-in; if it fails (network down, refresh revoked, etc.) the
/// app behaves as signed-out and the user can retry signIn().
///
/// Idempotent — calling repeatedly is fine, repeated calls just
/// refresh an existing access token if one's already cached.
export async function bootstrap(): Promise<Identity | null> {
  if (!auth0Configured) return null;
  const refreshToken = await SecureStore.getItemAsync(REFRESH_TOKEN_KEY);
  if (!refreshToken) return null;
  try {
    const fresh = await refreshAccessToken(refreshToken);
    state.cached = {
      token: fresh.accessToken,
      expiresAt: expiryFromResponse(fresh),
    };
    state.identity = parseIdentity(fresh.idToken) ?? state.identity;
    notify();
    return state.identity;
  } catch (e) {
    console.warn("[auth0] bootstrap refresh failed", e);
    // Don't clear the refresh token here — could be a transient
    // network failure rather than an expired/revoked token. signIn()
    // will overwrite it cleanly when the user retries.
    return null;
  }
}

/// Open the system browser for Auth0 universal login, exchange the
/// resulting code for tokens, persist the refresh token. Throws on
/// user cancellation or any auth failure — caller surfaces in UI.
export async function signIn(): Promise<Identity> {
  if (!auth0Configured) {
    throw new Error("Auth0 not configured.");
  }
  const disc = await discovery();
  const request = new AuthSession.AuthRequest({
    clientId: auth0Config.clientId,
    redirectUri: REDIRECT_URI,
    scopes: SCOPES,
    extraParams: { audience: auth0Config.audience },
    responseType: AuthSession.ResponseType.Code,
    usePKCE: true,
  });
  const result = await request.promptAsync(disc);
  if (result.type !== "success") {
    throw new Error(`Sign-in cancelled or failed: ${result.type}`);
  }
  const code = result.params.code;
  if (!code) {
    throw new Error("Sign-in returned no authorization code.");
  }
  // Exchange the auth code for tokens. PKCE: send the verifier we
  // generated alongside the original request.
  const exchanged = await AuthSession.exchangeCodeAsync(
    {
      clientId: auth0Config.clientId,
      code,
      redirectUri: REDIRECT_URI,
      extraParams: request.codeVerifier ? { code_verifier: request.codeVerifier } : undefined,
    },
    disc,
  );

  if (!exchanged.refreshToken) {
    throw new Error(
      "Auth0 returned no refresh token. Check that the API has 'Allow Offline Access' on " +
        "and the Native application's grant types include 'Refresh Token'.",
    );
  }
  await SecureStore.setItemAsync(REFRESH_TOKEN_KEY, exchanged.refreshToken);
  state.cached = {
    token: exchanged.accessToken,
    expiresAt: expiryFromResponse(exchanged),
  };
  state.identity = parseIdentity(exchanged.idToken) ?? { sub: "unknown" };
  notify();
  return state.identity;
}

/// Drop persisted refresh token + cached access token + identity.
/// The next request will fail until signIn() runs again.
export async function signOut(): Promise<void> {
  await SecureStore.deleteItemAsync(REFRESH_TOKEN_KEY);
  state.cached = null;
  state.identity = null;
  notify();
}

/// The function that downstream WS / REST clients call. Returns a
/// non-expired access token. If the cache is empty or about to
/// expire, refreshes silently using the persisted refresh token.
/// Throws if no refresh token is available — caller should surface
/// "please sign in" UI.
export async function getAccessToken(): Promise<string> {
  if (!auth0Configured) {
    throw new Error("Auth0 not configured.");
  }
  const now = Date.now();
  if (state.cached && state.cached.expiresAt - 30_000 > now) {
    return state.cached.token;
  }
  const refreshToken = await SecureStore.getItemAsync(REFRESH_TOKEN_KEY);
  if (!refreshToken) {
    throw new Error("Not signed in.");
  }
  const fresh = await refreshAccessToken(refreshToken);
  state.cached = {
    token: fresh.accessToken,
    expiresAt: expiryFromResponse(fresh),
  };
  // Auth0 may rotate the refresh token; if a new one came back,
  // persist it. Old token is invalidated server-side.
  if (fresh.refreshToken && fresh.refreshToken !== refreshToken) {
    await SecureStore.setItemAsync(REFRESH_TOKEN_KEY, fresh.refreshToken);
  }
  if (fresh.idToken) {
    state.identity = parseIdentity(fresh.idToken) ?? state.identity;
  }
  return fresh.accessToken;
}

/// Read-only accessor for the store / UI. Doesn't trigger a refresh.
export function currentIdentity(): Identity | null {
  return state.identity;
}

// ──────────────────────────  internals  ──────────────────────────

async function refreshAccessToken(refreshToken: string): Promise<AuthSession.TokenResponse> {
  const disc = await discovery();
  return AuthSession.refreshAsync(
    {
      clientId: auth0Config.clientId,
      refreshToken,
      // Re-send `audience` on refresh — Auth0 sometimes drops it
      // back to the Management API audience without it. Same
      // workaround the Mac client documents.
      extraParams: { audience: auth0Config.audience },
    },
    disc,
  );
}

function expiryFromResponse(r: AuthSession.TokenResponse): number {
  // expiresIn is seconds; default to 5 min if Auth0 omits it
  // (shouldn't happen for our tenant config, but be defensive).
  const seconds = r.expiresIn ?? 300;
  return Date.now() + seconds * 1000;
}

/// Parse the identity claims out of a JWT id_token without verifying
/// the signature. Auth0 fetched it for us over TLS from a trusted
/// issuer, and it never authenticates anything client-side — we use
/// it for display purposes only (sub / email / name in Settings).
function parseIdentity(idToken: string | undefined): Identity | null {
  if (!idToken) return null;
  const parts = idToken.split(".");
  if (parts.length < 2) return null;
  try {
    const payload = parts[1];
    const padded = payload + "=".repeat((4 - (payload.length % 4)) % 4);
    const base64 = padded.replace(/-/g, "+").replace(/_/g, "/");
    const json = JSON.parse(globalThis.atob(base64)) as {
      sub?: string;
      email?: string;
      name?: string;
    };
    if (!json.sub) return null;
    return { sub: json.sub, email: json.email, name: json.name };
  } catch {
    return null;
  }
}
