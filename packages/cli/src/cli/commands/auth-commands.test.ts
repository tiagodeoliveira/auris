import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { logoutCommand, whoamiCommand } from "./auth.js";

function tmpCred(): string {
  return join(tmpdir(), `auris-cli-${Math.random().toString(36).slice(2)}.json`);
}

describe("whoamiCommand", () => {
  it("reports sub + expiry from stored credentials", async () => {
    const p = tmpCred();
    const payload = Buffer.from(
      JSON.stringify({ sub: "google-oauth2|1", exp: 4102444800 }),
    ).toString("base64url");
    await fs.writeFile(
      p,
      JSON.stringify({ access_token: `h.${payload}.s`, expires_at: 4102444800 }),
    );
    const line = await whoamiCommand(p);
    expect(line).toContain("google-oauth2|1");
    await fs.rm(p, { force: true });
  });

  it("says not logged in when there are no credentials", async () => {
    expect(await whoamiCommand(tmpCred())).toMatch(/not logged in/i);
  });
});

describe("logoutCommand", () => {
  it("removes the credential file", async () => {
    const p = tmpCred();
    await fs.writeFile(p, "{}");
    await logoutCommand(p);
    await expect(fs.readFile(p, "utf8")).rejects.toBeTruthy();
  });
});
