#!/usr/bin/env node
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { AurisClient } from "../core/client.js";
import { resolveConfig } from "../core/config.js";
import { createServer } from "./server.js";

async function main(): Promise<void> {
  const cfg = resolveConfig();
  const client = new AurisClient(cfg.baseUrl, async () => cfg.envToken);
  const server = createServer(client);
  const transport = new StdioServerTransport();
  await server.connect(transport);
  console.error(`auris-mcp connected (base ${cfg.baseUrl})`);
}

main().catch((e) => {
  console.error(`auris-mcp failed to start: ${(e as Error).message}`);
  process.exit(1);
});
