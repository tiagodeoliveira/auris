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

Phase 2 step 16 wires real LLM-based metadata extraction via [rig](https://github.com/0xPlaygrounds/rig) + AWS Bedrock (Anthropic Claude Sonnet 4.7). The server requires AWS credentials at boot to construct the LLM client.

### Configuration

| Env var                              | Required | Default                                        |
| ------------------------------------ | -------- | ---------------------------------------------- |
| AWS credentials (any standard chain) | yes      | —                                              |
| `MEETING_COMPANION_LLM_REGION`       | no       | `us-west-2`                                    |
| `MEETING_COMPANION_LLM_MODEL_ID`     | no       | `us.anthropic.claude-sonnet-4-7-20251015-v1:0` |
| `MEETING_COMPANION_LLM_DISABLED`     | no       | unset (extraction enabled)                     |

The cross-region inference profile (model id starting with `us.`) must be enabled in your AWS Bedrock console — one-time setup per account.

`MEETING_COMPANION_LLM_DISABLED=1` skips extraction entirely. Default in the test suite. Useful for offline dev.

### Smoke

```bash
just llm-smoke "your meeting description"
```

### Integration test

```bash
just llm-integration
```

(Requires `RUN_LLM_INTEGRATION=1` + working AWS credentials + Sonnet 4.7 enabled.)

### Why rig

rig was chosen over a direct AWS SDK integration for: 20+ provider support (we ship Bedrock; switching to Anthropic-direct or OpenAI is a constructor change), agent abstractions for future Phase 2 step 18 work, retry/backoff embedded in rig's transport, and `cortex-mem` for the memory layer (Phase 2 step 18 wires this to mnemo).

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
