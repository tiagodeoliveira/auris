import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { clearCredentials, decodeToken, getAccessToken, loadCredentials } from "./auth.js";

function tmpCred(): string {
  return join(tmpdir(), `auris-auth-${Math.random().toString(36).slice(2)}.json`);
}
const OPTS = { domain: "auth.test", clientId: "cid" };
function tokenResp(body: unknown, ok = true) {
  return vi.fn().mockResolvedValue({ ok, json: async () => body });
}
afterEach(() => vi.restoreAllMocks());

describe("getAccessToken", () => {
  it("returns the cached token when it is not near expiry", async () => {
    const p = tmpCred();
    await fs.writeFile(
      p,
      JSON.stringify({ access_token: "cached", expires_at: Math.floor(Date.now() / 1000) + 3600 }),
    );
    expect(await getAccessToken(OPTS, p)).toBe("cached");
    await fs.rm(p, { force: true });
  });

  it("refreshes via refresh_token when the access token is expired", async () => {
    const p = tmpCred();
    await fs.writeFile(
      p,
      JSON.stringify({
        access_token: "old",
        refresh_token: "r",
        expires_at: Math.floor(Date.now() / 1000) - 10,
      }),
    );
    vi.stubGlobal("fetch", tokenResp({ access_token: "fresh", expires_in: 3600 }));
    expect(await getAccessToken(OPTS, p)).toBe("fresh");
    const saved = JSON.parse(await fs.readFile(p, "utf8"));
    expect(saved.access_token).toBe("fresh");
    expect(saved.refresh_token).toBe("r"); // preserved when Auth0 omits a new one
    await fs.rm(p, { force: true });
  });

  it("returns null when there are no credentials", async () => {
    expect(await getAccessToken(OPTS, tmpCred())).toBeNull();
  });

  it("returns null when expired and there is no refresh token", async () => {
    const p = tmpCred();
    await fs.writeFile(p, JSON.stringify({ access_token: "old", expires_at: 0 }));
    expect(await getAccessToken(OPTS, p)).toBeNull();
    await fs.rm(p, { force: true });
  });
});

describe("loadCredentials / clearCredentials", () => {
  it("loads null for a missing file and clears an existing one", async () => {
    const p = tmpCred();
    expect(await loadCredentials(p)).toBeNull();
    await fs.writeFile(p, JSON.stringify({ access_token: "a", expires_at: 1 }));
    expect((await loadCredentials(p))?.access_token).toBe("a");
    await clearCredentials(p);
    expect(await loadCredentials(p)).toBeNull();
  });
});

describe("decodeToken", () => {
  it("extracts sub/exp from a JWT payload without verifying", () => {
    const payload = Buffer.from(JSON.stringify({ sub: "google-oauth2|1", exp: 123 })).toString(
      "base64url",
    );
    const d = decodeToken(`h.${payload}.sig`);
    expect(d).toMatchObject({ sub: "google-oauth2|1", exp: 123 });
  });
});
