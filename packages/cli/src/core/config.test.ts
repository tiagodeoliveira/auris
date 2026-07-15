import { describe, expect, it } from "vitest";
import { requireAuth0, resolveConfig } from "./config.js";

describe("resolveConfig", () => {
  it("falls back to DEFAULTS (dev base URL, empty auth0) with a clean env", () => {
    const c = resolveConfig({} as NodeJS.ProcessEnv);
    expect(c.baseUrl).toBe("http://localhost:7331");
    expect(c.auth0).toEqual({ domain: "", audience: "", clientId: "" });
    expect(c.envToken).toBeNull();
  });

  it("honors env overrides and trims the base URL slash", () => {
    const c = resolveConfig({
      AURIS_BASE_URL: "https://auris.test/",
      AURIS_AUTH0_DOMAIN: "d",
      AURIS_AUTH0_AUDIENCE: "a",
      AURIS_AUTH0_CLIENT_ID: "c",
    } as NodeJS.ProcessEnv);
    expect(c.baseUrl).toBe("https://auris.test");
    expect(c.auth0).toEqual({ domain: "d", audience: "a", clientId: "c" });
  });

  it("resolves envToken: AURIS_TOKEN wins over AURIS_MCP_TOKEN", () => {
    expect(
      resolveConfig({ AURIS_TOKEN: "x", AURIS_MCP_TOKEN: "y" } as NodeJS.ProcessEnv).envToken,
    ).toBe("x");
    expect(resolveConfig({ AURIS_MCP_TOKEN: "y" } as NodeJS.ProcessEnv).envToken).toBe("y");
  });
});

describe("requireAuth0", () => {
  it("throws a clear error when any field is empty", () => {
    expect(() => requireAuth0({ domain: "", audience: "a", clientId: "c" })).toThrow(
      /Auth0 not configured/,
    );
  });
  it("returns the config when complete", () => {
    const a = { domain: "d", audience: "a", clientId: "c" };
    expect(requireAuth0(a)).toBe(a);
  });
});
