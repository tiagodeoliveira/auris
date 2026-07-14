import { describe, expect, it } from "vitest";
import { loadConfig } from "./config.js";

describe("loadConfig", () => {
  it("reads token and defaults the base URL", () => {
    const cfg = loadConfig({ AURIS_MCP_TOKEN: "tok" } as NodeJS.ProcessEnv);
    expect(cfg.token).toBe("tok");
    expect(cfg.baseUrl).toBe("https://auris.tiago.tools");
  });

  it("honors AURIS_BASE_URL and trims a trailing slash", () => {
    const cfg = loadConfig({
      AURIS_MCP_TOKEN: "tok",
      AURIS_BASE_URL: "https://example.test/",
    } as NodeJS.ProcessEnv);
    expect(cfg.baseUrl).toBe("https://example.test");
  });

  it("throws a clear error when the token is missing", () => {
    expect(() => loadConfig({} as NodeJS.ProcessEnv)).toThrow(/AURIS_MCP_TOKEN/);
  });

  it("throws when the token is empty", () => {
    expect(() => loadConfig({ AURIS_MCP_TOKEN: "" } as NodeJS.ProcessEnv)).toThrow(
      /AURIS_MCP_TOKEN/,
    );
  });
});
