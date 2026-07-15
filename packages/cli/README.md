# @auris/mcp

Local stdio MCP server exposing read-only access to your auris meetings.

## Tools

- `list_meetings` — recent meetings (newest first) as compact summaries.
- `search_meetings` — filter by `query` (title/description substring), `project`, and `since`/`until` (YYYY-MM-DD).
- `get_meeting` — one meeting's briefing: summary, highlights, actions, open questions, moments. No raw transcript.
- `get_meeting_transcript` — paginated verbatim transcript (`offset`/`limit`); speaker is embedded in each item's text as `[Speaker N] …`.

## Setup

    pnpm --filter @auris/mcp build

Register with your agent (Claude Code `mcp` config), providing your auris bearer token:

    {
      "mcpServers": {
        "auris": {
          "command": "node",
          "args": ["/absolute/path/to/auris/packages/mcp/dist/index.js"],
          "env": {
            "AURIS_MCP_TOKEN": "<your auris Auth0 access token>",
            "AURIS_BASE_URL": "https://auris.tiago.tools"
          }
        }
      }
    }

`AURIS_BASE_URL` is optional (defaults to `https://auris.tiago.tools`). The token
is an Auth0 access token; when it expires the tools return an error telling you to
refresh `AURIS_MCP_TOKEN`.
