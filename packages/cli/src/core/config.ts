import { DEFAULTS } from "./defaults.js";

export interface Auth0Config {
  domain: string;
  audience: string;
  clientId: string;
}

export interface ResolvedConfig {
  baseUrl: string;
  auth0: Auth0Config;
  /** Static bearer override (AURIS_TOKEN, or legacy AURIS_MCP_TOKEN); null if unset. */
  envToken: string | null;
}

function pick(envVal: string | undefined, fallback: string): string {
  return envVal?.trim() || fallback;
}

export function resolveConfig(env: NodeJS.ProcessEnv = process.env): ResolvedConfig {
  const baseUrl = pick(env.AURIS_BASE_URL, DEFAULTS.aurisBaseUrl).replace(/\/+$/, "");
  const auth0: Auth0Config = {
    domain: pick(env.AURIS_AUTH0_DOMAIN, DEFAULTS.auth0Domain),
    audience: pick(env.AURIS_AUTH0_AUDIENCE, DEFAULTS.auth0Audience),
    clientId: pick(env.AURIS_AUTH0_CLIENT_ID, DEFAULTS.auth0ClientId),
  };
  const envToken = env.AURIS_TOKEN?.trim() || env.AURIS_MCP_TOKEN?.trim() || null;
  return { baseUrl, auth0, envToken };
}

/** Assert Auth0 is configured (release build or env). Throws otherwise. */
export function requireAuth0(a: Auth0Config): Auth0Config {
  if (!a.domain || !a.audience || !a.clientId) {
    throw new Error(
      "Auth0 not configured — set AURIS_AUTH0_DOMAIN / AURIS_AUTH0_AUDIENCE / AURIS_AUTH0_CLIENT_ID (or use a released build).",
    );
  }
  return a;
}
