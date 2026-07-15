import { promises as fs } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

export interface Credentials {
  access_token: string;
  refresh_token?: string;
  expires_at: number; // unix seconds
}

export function defaultCredPath(): string {
  return join(homedir(), ".auris", "credentials.json");
}

interface DeviceCodeResp {
  device_code: string;
  user_code: string;
  verification_uri: string;
  verification_uri_complete: string;
  expires_in: number;
  interval: number;
}
interface TokenResp {
  access_token?: string;
  refresh_token?: string;
  expires_in?: number;
  error?: string;
  error_description?: string;
}

export async function deviceLogin(
  opts: { domain: string; audience: string; clientId: string },
  credPath: string = defaultCredPath(),
): Promise<Credentials> {
  const dcRes = await fetch(`https://${opts.domain}/oauth/device/code`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      client_id: opts.clientId,
      scope: "openid profile email offline_access",
      audience: opts.audience,
    }),
  });
  if (!dcRes.ok) throw new Error(`auth: device/code ${dcRes.status}: ${await dcRes.text()}`);
  const dc = (await dcRes.json()) as DeviceCodeResp;

  console.log(`\nVisit: ${dc.verification_uri_complete}`);
  console.log(`Code:  ${dc.user_code}\n`);
  console.log("Waiting for approval...");

  const deadline = Date.now() + dc.expires_in * 1000;
  let intervalMs = dc.interval * 1000;
  while (Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, intervalMs));
    const tRes = await fetch(`https://${opts.domain}/oauth/token`, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        grant_type: "urn:ietf:params:oauth:grant-type:device_code",
        device_code: dc.device_code,
        client_id: opts.clientId,
      }),
    });
    const t = (await tRes.json()) as TokenResp;
    if (t.access_token) {
      const cred: Credentials = {
        access_token: t.access_token,
        refresh_token: t.refresh_token,
        expires_at: Math.floor(Date.now() / 1000) + (t.expires_in ?? 0),
      };
      await persist(credPath, cred);
      console.log(`Logged in. Credentials saved to ${credPath}`);
      return cred;
    }
    if (t.error === "slow_down") intervalMs += 5000;
    else if (t.error && t.error !== "authorization_pending")
      throw new Error(`auth: ${t.error}: ${t.error_description ?? ""}`);
  }
  throw new Error("auth: device code expired before approval");
}

async function persist(credPath: string, cred: Credentials): Promise<void> {
  await fs.mkdir(join(credPath, ".."), { recursive: true });
  await fs.writeFile(credPath, JSON.stringify(cred, null, 2), { mode: 0o600 });
}

export async function loadCredentials(
  credPath: string = defaultCredPath(),
): Promise<Credentials | null> {
  try {
    return JSON.parse(await fs.readFile(credPath, "utf8")) as Credentials;
  } catch {
    return null;
  }
}

export async function clearCredentials(credPath: string = defaultCredPath()): Promise<void> {
  await fs.rm(credPath, { force: true });
}

const REFRESH_SKEW_SECONDS = 60;

export async function getAccessToken(
  opts: { domain: string; clientId: string },
  credPath: string = defaultCredPath(),
): Promise<string | null> {
  const cred = await loadCredentials(credPath);
  if (!cred) return null;
  if (cred.expires_at - REFRESH_SKEW_SECONDS > Math.floor(Date.now() / 1000))
    return cred.access_token;
  if (!cred.refresh_token) return null;
  const res = await fetch(`https://${opts.domain}/oauth/token`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "refresh_token",
      client_id: opts.clientId,
      refresh_token: cred.refresh_token,
    }),
  });
  if (!res.ok) return null;
  const t = (await res.json()) as TokenResp;
  if (!t.access_token) return null;
  const fresh: Credentials = {
    access_token: t.access_token,
    refresh_token: t.refresh_token ?? cred.refresh_token,
    expires_at: Math.floor(Date.now() / 1000) + (t.expires_in ?? 0),
  };
  await persist(credPath, fresh);
  return fresh.access_token;
}

export function decodeToken(jwt: string): { sub?: string; email?: string; exp?: number } {
  try {
    const payload = jwt.split(".")[1];
    return JSON.parse(Buffer.from(payload, "base64url").toString("utf8"));
  } catch {
    return {};
  }
}
