// Pre-pack sync step for the EvenHub G2 app's `app.json`.
//
//   1. Ensure the project's package.json `version` is mirrored to
//      `app.json.version` — both are user-visible (the latter is what
//      the EvenHub store shows; the former is what the in-app
//      settings modal displays via VITE_APP_VERSION). Single source
//      of truth: package.json. Run on every pack so the .ehpk
//      version always matches the release that produced it.
//   2. Ensure the `network` permission whitelist covers every origin
//      the bundled app actually talks to: the auris server (from
//      VITE_SERVER_URL). Auth happens via the server's own
//      /pair/* endpoints, so no third-party domains need a slot
//      here. The EvenHub permission gate drops fetch() calls to
//      non-whitelisted origins.
//
// All env values are read from .env.local (Vite convention). Missing
// values fall back to local-dev defaults so the script never aborts.

import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const APP_JSON = resolve("app.json");
const PKG_JSON = resolve("package.json");

/// Read env vars from Vite's `.env` files in precedence order
/// (matches the runtime resolution Vite itself does):
///   1. .env.local   — local overrides, gitignored
///   2. .env         — committed defaults
/// Later files do NOT overwrite earlier ones — first-wins matches
/// Vite's documented behavior for build-time substitution.
function readEnv(): Record<string, string> {
  const env: Record<string, string> = {};
  for (const filename of [".env.local", ".env"]) {
    try {
      const text = readFileSync(resolve(filename), "utf-8");
      for (const line of text.split("\n")) {
        const m = line.match(/^([A-Z_][A-Z0-9_]*)=(.*)$/);
        if (m && !(m[1] in env)) {
          env[m[1]] = m[2].replace(/^"|"$/g, "");
        }
      }
    } catch {
      // Missing file is fine — both are optional.
    }
  }
  return env;
}

function wsToHttp(url: string): string {
  return url.replace(/^ws/, "http");
}

function originOnly(url: string): string {
  try {
    const u = new URL(url);
    return `${u.protocol}//${u.host}`;
  } catch {
    return url;
  }
}

const env = readEnv();
// Match the runtime resolution order in src/server-url.ts:
// VITE_SERVER_URL preferred, VITE_DEFAULT_SERVER_URL legacy fallback,
// then a local-dev default.
const serverUrl = env.VITE_SERVER_URL ?? env.VITE_DEFAULT_SERVER_URL ?? "ws://localhost:7331";
const serverOrigin = originOnly(wsToHttp(serverUrl));

// Optional dev-mode addition: when set, allow the bundle to load
// its own subresources from the Vite dev server's LAN address. The
// EvenHub companion app's prototype-mode flow scans a QR pointing
// at this origin; without it in the whitelist the companion's
// permission gate drops the subresource fetches and the page
// renders blank. Set VITE_PWA_DEV_HOST=192.168.4.86 (your LAN IP)
// in .env.local before running sync to enable dev-on-real-glasses.
// Both http and ws entries are added — Vite's HMR uses the latter.
const devHost = env.VITE_PWA_DEV_HOST;
const devPort = env.VITE_PWA_DEV_PORT ?? "5173";
const devOrigins = devHost ? [`http://${devHost}:${devPort}`, `ws://${devHost}:${devPort}`] : [];

const requiredOrigins: string[] = [serverOrigin, ...devOrigins];

const pkg = JSON.parse(readFileSync(PKG_JSON, "utf-8")) as { version: string };
const app = JSON.parse(readFileSync(APP_JSON, "utf-8")) as {
  version: string;
  permissions?: { name: string; whitelist?: string[] }[];
};

let versionChanged = false;
if (app.version !== pkg.version) {
  app.version = pkg.version;
  versionChanged = true;
}

for (const perm of app.permissions ?? []) {
  if (perm.name === "network") {
    perm.whitelist = Array.from(new Set([...(perm.whitelist ?? []), ...requiredOrigins]));
  }
}
writeFileSync(APP_JSON, JSON.stringify(app, null, 2) + "\n");

const versionMsg = versionChanged
  ? ` (version bumped to ${pkg.version})`
  : ` (version already ${pkg.version})`;
console.log(
  `sync-whitelist: app.json synced — origins: [${requiredOrigins.join(", ")}]${versionMsg}`,
);
