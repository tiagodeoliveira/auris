import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import type { MeetingApi } from "../core/client.js";
import { makeTools } from "./handlers.js";

/** Build an McpServer with the five read-only meeting tools registered. */
export function createServer(client: MeetingApi): McpServer {
  const server = new McpServer({ name: "auris-mcp", version: "0.1.0" });
  for (const tool of makeTools(client)) {
    server.registerTool(
      tool.name,
      { description: tool.description, inputSchema: tool.schema },
      async (args) => {
        const result = await tool.handler(args as Record<string, unknown>);
        return { ...result };
      },
    );
  }
  return server;
}
