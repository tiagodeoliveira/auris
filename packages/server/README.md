# meeting-companion-server

Phase 0 stub server. See `docs/specs/server.md` for the spec.

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

See `docs/specs/server.md` for the full protocol.
