import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const APP_JSON = resolve("app.json");
const ENV_LOCAL = resolve(".env.local");

function readEnvLocal(): Record<string, string> {
  try {
    const text = readFileSync(ENV_LOCAL, "utf-8");
    const env: Record<string, string> = {};
    for (const line of text.split("\n")) {
      const m = line.match(/^([A-Z_][A-Z0-9_]*)=(.*)$/);
      if (m) env[m[1]] = m[2].replace(/^"|"$/g, "");
    }
    return env;
  } catch {
    return {};
  }
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

const env = readEnvLocal();
const serverUrl = env.VITE_DEFAULT_SERVER_URL ?? "ws://localhost:7331";
const origin = originOnly(wsToHttp(serverUrl));

const app = JSON.parse(readFileSync(APP_JSON, "utf-8"));
for (const perm of app.permissions ?? []) {
  if (perm.name === "network") {
    perm.whitelist = Array.from(new Set([...(perm.whitelist ?? []), origin]));
  }
}
writeFileSync(APP_JSON, JSON.stringify(app, null, 2) + "\n");
console.log(`sync-whitelist: ensured ${origin} is in app.json network whitelist`);
