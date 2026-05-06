//! Auth0 SPA integration.
//!
//! Wraps `@auth0/auth0-spa-js` with the bits the rest of the PWA
//! needs: `getAccessToken()` for the WS query string and REST Bearer
//! header, `loginWithRedirect()` for the sign-in button, and
//! `logout()` for the settings menu.
//!
//! Tokens are kept inside the Auth0 client (refresh-token rotation
//! enabled, localStorage cache for SPA reload survivability). The
//! app store mirrors *only* the user's profile (`AuthIdentity`), not
//! the access token — that way nothing token-y lands in store
//! snapshots / dev-tools / logs.

import { createAuth0Client, type Auth0Client } from "@auth0/auth0-spa-js";
import type { AuthIdentity } from "./types";

/// Build-time config baked from Vite env vars. The PWA never sees a
/// runtime config endpoint — these have to be set at build time and
/// will produce hard errors if missing so we don't silently fall back
/// to no-auth.
export interface Auth0Config {
  domain: string;
  clientId: string;
  audience: string;
}

export function readAuth0Config(env: Record<string, string | undefined>): Auth0Config | null {
  const domain = env.VITE_AUTH0_DOMAIN;
  const clientId = env.VITE_AUTH0_PWA_CLIENT_ID;
  const audience = env.VITE_AUTH0_API_AUDIENCE;
  if (!domain || !clientId || !audience) return null;
  return { domain, clientId, audience };
}

export interface AuthBundle {
  client: Auth0Client;
  identity: AuthIdentity | null;
  /// Returns a usable access token. Auth0 SDK caches and refreshes
  /// it under the hood; calling this on every request is fine.
  getAccessToken: () => Promise<string>;
  loginWithRedirect: () => Promise<void>;
  logout: () => Promise<void>;
}

/// Initialize the Auth0 client and resolve the current session.
///
/// On boot the SDK looks at the URL: if it carries an OAuth
/// `code` + `state` (we just came back from the Universal Login
/// page), it exchanges the code for tokens and cleans the URL. If
/// not, it tries `getTokenSilently()` to see if there's still a
/// valid session (refresh-token in localStorage). Either way we end
/// up with `isAuthenticated() === true` and a fetchable token, or
/// not — at which point the caller should render the login screen.
export async function initAuth(config: Auth0Config): Promise<AuthBundle> {
  const client = await createAuth0Client({
    domain: config.domain,
    clientId: config.clientId,
    authorizationParams: {
      audience: config.audience,
      redirect_uri: `${window.location.origin}${window.location.pathname.replace(/\/[^/]*$/, "")}/`,
      scope: "openid profile email offline_access",
    },
    cacheLocation: "localstorage",
    useRefreshTokens: true,
    useRefreshTokensFallback: true,
  });

  // If we just came back from /authorize with the code in the URL,
  // finalize the exchange and strip the params so refreshing the
  // page doesn't try to redeem the same code twice.
  if (window.location.search.includes("code=") && window.location.search.includes("state=")) {
    try {
      await client.handleRedirectCallback();
    } catch (e) {
      // Bad state / replay / etc. — clear and let the caller show
      // the login screen so the user can try again.
      console.warn("[auth] redirect callback failed", e);
    }
    const cleanUrl = window.location.pathname + window.location.hash;
    window.history.replaceState({}, document.title, cleanUrl);
  }

  let identity: AuthIdentity | null = null;
  if (await client.isAuthenticated()) {
    const user = await client.getUser();
    if (user && user.sub) {
      identity = {
        sub: user.sub,
        email: user.email ?? null,
        name: user.name ?? null,
        picture: user.picture ?? null,
      };
    }
  }

  return {
    client,
    identity,
    getAccessToken: async () => client.getTokenSilently(),
    loginWithRedirect: async () => {
      await client.loginWithRedirect({
        authorizationParams: {
          // Forward to the same path the user opened — keeps "I
          // bookmarked /meetings/123" working through login.
          redirect_uri: `${window.location.origin}${window.location.pathname.replace(/\/[^/]*$/, "")}/`,
        },
      });
    },
    logout: async () => {
      await client.logout({
        logoutParams: {
          returnTo: `${window.location.origin}${window.location.pathname.replace(/\/[^/]*$/, "")}/`,
        },
      });
    },
  };
}
