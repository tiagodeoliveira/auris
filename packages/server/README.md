# meeting-companion-server

Phase 0 stub server. See [`docs/specs/server.md`](../../docs/specs/server.md) for the spec.

## Run

```bash
MEETING_COMPANION_TOKEN=dev cargo run -p meeting-companion-server -- --port 7331
```

Or via `just`:

```bash
just server-run
```

## Test

```bash
cargo test -p meeting-companion-server -- --test-threads=1
```

`--test-threads=1` is required because the heartbeat tests set a process-global env var (`MEETING_COMPANION_HEARTBEAT_MS`) to accelerate ticks; running other test binaries in parallel would inherit that override.

## LLM-based metadata extraction

Phase 2 step 16 wires real LLM-based metadata extraction via [rig](https://github.com/0xPlaygrounds/rig). The server supports three providers as of v3: **AWS Bedrock** (default — Anthropic Claude Sonnet 4.7), **OpenAI** (gpt-4.1-mini by default), and **Anthropic-direct** (Claude Sonnet 4.5 by default — set `MEETING_COMPANION_LLM_MODEL_ID=claude-sonnet-4-7` to use 4.7). Provider chosen at boot via env var.

### Configuration

For local dev, copy `.env.example` (at the workspace root) to `.env` and fill in the keys you need. The server binary, the `llm_smoke` example, and the env-gated integration test all auto-load `.env` via `dotenvy`. `.env` is gitignored; `.env.example` is not.

| Env var                              | Required when                 | Default           |
| ------------------------------------ | ----------------------------- | ----------------- |
| `MEETING_COMPANION_LLM_PROVIDER`     | no                            | `bedrock`         |
| `MEETING_COMPANION_LLM_MODEL_ID`     | no                            | provider-specific |
| `MEETING_COMPANION_LLM_DISABLED`     | no                            | unset             |
| **Bedrock-only**                     |                               |                   |
| AWS credentials (any standard chain) | when `LLM_PROVIDER=bedrock`   | —                 |
| `MEETING_COMPANION_LLM_REGION`       | no                            | `us-west-2`       |
| **OpenAI-only**                      |                               |                   |
| `OPENAI_API_KEY`                     | when `LLM_PROVIDER=openai`    | —                 |
| **Anthropic-only**                   |                               |                   |
| `ANTHROPIC_API_KEY`                  | when `LLM_PROVIDER=anthropic` | —                 |

Bedrock provider-specific notes: the cross-region inference profile (model id starting with `us.`) must be enabled in your AWS Bedrock console — one-time setup per account.

`MEETING_COMPANION_LLM_DISABLED=1` skips extraction entirely. Default in the test suite. Useful for offline dev.

### Smoke

```bash
just llm-smoke "your meeting description"          # uses currently-configured provider
just llm-smoke-bedrock "your description"          # forces bedrock
just llm-smoke-openai "your description"           # forces openai
just llm-smoke-anthropic "your description"        # forces anthropic-direct
```

### Comparing providers

To compare extractions side by side, run the same description against multiple:

```bash
just llm-smoke-bedrock "Q1 budget review for helix"
just llm-smoke-openai "Q1 budget review for helix"
just llm-smoke-anthropic "Q1 budget review for helix"
```

### Integration test

```bash
just llm-integration
```

(Requires `RUN_LLM_INTEGRATION=1` + the matching credentials for the selected provider. Provider selected via `MEETING_COMPANION_LLM_PROVIDER` env var; defaults to bedrock.)

### Why rig

rig was chosen over direct provider SDKs for: provider-pluggable via env var (v3 ships Bedrock + OpenAI + Anthropic-direct; adding more rig-supported providers is a one-arm-on-the-enum change), agent abstractions for future Phase 2 step 18 work, retry/backoff embedded in rig's transport, and `cortex-mem` for the memory layer (Phase 2 step 18 wires this to mnemo).

## Manual smoke

In one terminal:

```bash
just server-run
```

In another:

```bash
websocat 'ws://localhost:7331/?token=dev'
```

Paste intents to interact:

```json
{"type":"start_meeting","description":"Q1 budget review","metadata":{"project":"helix"}}
{"type":"set_mode","mode":"transcript"}
{"type":"mark_moment","t":12345}
{"type":"stop_meeting"}
```

## Configuration

| Env / Flag                       | Default    | Description                              |
| -------------------------------- | ---------- | ---------------------------------------- |
| `MEETING_COMPANION_TOKEN`        | (required) | Shared secret for WS auth.               |
| `MEETING_COMPANION_HEARTBEAT_MS` | `10000`    | Heartbeat interval (test override only). |
| `RUST_LOG`                       | `info`     | tracing-subscriber filter.               |
| `--port`                         | `7331`     | TCP port.                                |
| `--bind`                         | `0.0.0.0`  | Bind address.                            |

See [`docs/specs/server.md`](../../docs/specs/server.md) for the full protocol.
