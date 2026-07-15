import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { deviceLogin } from "./auth.js";

function tmpCred(): string {
  return join(tmpdir(), `auris-devicelogin-${Math.random().toString(36).slice(2)}.json`);
}

const OPTS = { domain: "auth.test", audience: "aud", clientId: "cid" };

function jsonResponse(body: unknown, ok = true) {
  return {
    ok,
    status: ok ? 200 : 400,
    json: async () => body,
    text: async () => JSON.stringify(body),
  };
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("deviceLogin", () => {
  it("polls until approval and persists credentials", async () => {
    const p = tmpCred();
    const deviceCode = {
      device_code: "dc-1",
      user_code: "ABCD-EFGH",
      verification_uri: "https://auth.test/activate",
      verification_uri_complete: "https://auth.test/activate?user_code=ABCD-EFGH",
      expires_in: 300,
      interval: 0,
    };
    let tokenCall = 0;
    const fetchMock = vi.fn(async (url: string) => {
      if (url.includes("/oauth/device/code")) return jsonResponse(deviceCode);
      if (url.includes("/oauth/token")) {
        tokenCall += 1;
        if (tokenCall === 1) return jsonResponse({ error: "authorization_pending" });
        return jsonResponse({ access_token: "tok-1", refresh_token: "ref-1", expires_in: 3600 });
      }
      throw new Error(`unexpected fetch: ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});

    const cred = await deviceLogin(OPTS, p);

    expect(cred.access_token).toBe("tok-1");
    expect(cred.refresh_token).toBe("ref-1");
    const saved = JSON.parse(await fs.readFile(p, "utf8"));
    expect(saved.access_token).toBe("tok-1");
    expect(tokenCall).toBe(2);

    logSpy.mockRestore();
    await fs.rm(p, { force: true });
  });

  it("rejects on a non-pending Auth0 error", async () => {
    const p = tmpCred();
    const deviceCode = {
      device_code: "dc-2",
      user_code: "IJKL-MNOP",
      verification_uri: "https://auth.test/activate",
      verification_uri_complete: "https://auth.test/activate?user_code=IJKL-MNOP",
      expires_in: 300,
      interval: 0,
    };
    const fetchMock = vi.fn(async (url: string) => {
      if (url.includes("/oauth/device/code")) return jsonResponse(deviceCode);
      if (url.includes("/oauth/token"))
        return jsonResponse({ error: "access_denied", error_description: "user declined" });
      throw new Error(`unexpected fetch: ${url}`);
    });
    vi.stubGlobal("fetch", fetchMock);
    const logSpy = vi.spyOn(console, "log").mockImplementation(() => {});

    await expect(deviceLogin(OPTS, p)).rejects.toThrow(/access_denied/);

    logSpy.mockRestore();
    await fs.rm(p, { force: true });
  });
});
