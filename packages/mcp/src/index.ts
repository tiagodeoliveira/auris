#!/usr/bin/env node
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { AurisClient } from "./client.js";
import { loadConfig } from "./config.js";
import { createServer } from "./server.js";

async function main(): Promise<void> {
  const config = loadConfig();
  const client = new AurisClient(config);
  const server = createServer(client);
  const transport = new StdioServerTransport();
  await server.connect(transport);
  // Log to stderr only — stdout is the MCP stdio channel.
  console.error(`auris-mcp connected (base ${config.baseUrl})`);
}

main().catch((e) => {
  console.error(`auris-mcp failed to start: ${(e as Error).message}`);
  process.exit(1);
});
