import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { describe, expect, it } from "vitest";
import type { MeetingApi } from "../core/client.js";
import { createServer } from "./server.js";

const noopClient: MeetingApi = {
  listMeetings: async () => [],
  getMeeting: async () => {
    throw new Error("unused");
  },
};

describe("createServer", () => {
  it("registers the four meeting tools over MCP", async () => {
    const server = createServer(noopClient);
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    await server.connect(serverTransport);

    const client = new Client({ name: "test", version: "0.0.0" });
    await client.connect(clientTransport);

    const { tools } = await client.listTools();
    expect(tools.map((t) => t.name).sort()).toEqual(
      ["get_meeting", "get_meeting_transcript", "list_meetings", "search_meetings"].sort(),
    );

    await client.close();
    await server.close();
  });
});
