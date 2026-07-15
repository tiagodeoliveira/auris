# @auris/cli

`auris` — your meetings from the terminal — plus `auris-mcp`, an MCP server that
exposes the same read-only tools to an agent. Both share one auth session.

## Install

Download the tarball from the latest GitHub release, then:

    npm i -g ./auris-cli-<version>.tgz   # provides `auris` and `auris-mcp`

## Auth

    auris login      # Auth0 device flow → stores ~/.auris/credentials.json (90-day refresh)
    auris whoami
    auris logout

`AURIS_TOKEN` overrides the stored session for one-offs. A from-source build
targets `http://localhost:7331`; released builds target the stamped base URL.
Override anything with `AURIS_BASE_URL` / `AURIS_AUTH0_DOMAIN` / `_AUDIENCE` /
`_CLIENT_ID`.

## CLI

    auris meetings list [--limit N] [--json]
    auris meetings search [--query ..] [--project ..] [--since YYYY-MM-DD] [--until ..] [--limit N] [--json]
    auris meetings get <id> [--json]
    auris meetings transcript <id> [--offset N] [--limit N] [--json]

## MCP (agent)

    claude mcp add auris -- node /absolute/path/to/global/auris-mcp

No token needed once you've run `auris login` — the MCP reuses the same session
and auto-refreshes. Tools: `list_meetings`, `search_meetings`, `get_meeting`
(briefing, no transcript), `get_meeting_transcript` (paginated).
