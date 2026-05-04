# meeting-companion-server

Rust WebSocket server. Owns meeting state, captures audio, runs streaming
STT, drives parallel LLM summarizers, and pushes / recalls memories
against [mnemo](https://github.com/tiagodeoliveira/mnemo).

System overview: [`docs/ARCHITECTURE.md`](../../docs/ARCHITECTURE.md).
Wire protocol: [`docs/PROTOCOL.md`](../../docs/PROTOCOL.md).
Decisions: [`docs/adr/`](../../docs/adr/).

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

`--test-threads=1` is required because the heartbeat tests set a
process-global env var (`MEETING_COMPANION_HEARTBEAT_MS`); running other
test binaries in parallel would inherit that override.

## What it does

Connected lifecycle of one meeting:

1. PWA opens WS, server emits `Snapshot`.
2. PWA sends `extract_metadata` (or `start_meeting`); LLM extracts
   metadata; server emits `metadata_changed`.
3. PWA sends `start_meeting`; state goes Active. Server spawns:
   - audio capture (`audio/`) — ScreenCaptureKit on macOS.
   - mixer task — combines system audio + mic at 50 fps.
   - STT adapter (`stt/`) — Soniox WS streaming, or mock backend.
   - four summarizer tasks (`summarizer/`) — one per mode, each on its
     own heartbeat with its own LLM prompt.
4. mnemo recaller fires one `GET /recall` and writes
   `state.recalled_context`; emits `prior_context_changed`.
5. As STT promotes buffered tokens to sentence-flushed `Item`s:
   - server appends to `state.rolling_transcript` and broadcasts
     `items_update { mode: "transcript" }`.
   - mnemo pusher streams one `user`-role turn per sentence.
6. Each summarizer cycles: read transcript + (optional) prior context,
   call LLM, dedup, broadcast `items_update { mode }`.
7. PWA `stop_meeting` → state goes Idle. Cleanup: cancel meeting tasks,
   cancel in-flight extraction, push final `assistant`-role summary
   bundle to mnemo, clear `state`.

## Configuration

All env vars are optional except `MEETING_COMPANION_TOKEN`. Copy
`.env.example` (at the workspace root) to `.env` and fill in the keys
you need; `dotenvy` auto-loads it.

### Server

| Env / Flag                       | Default    | Description                              |
| -------------------------------- | ---------- | ---------------------------------------- |
| `MEETING_COMPANION_TOKEN`        | (required) | Shared secret for WS auth.               |
| `MEETING_COMPANION_HEARTBEAT_MS` | `10000`    | Heartbeat interval (test override only). |
| `RUST_LOG`                       | `info`     | `tracing-subscriber` filter.             |
| `--port`                         | `7331`     | TCP port.                                |
| `--bind`                         | `0.0.0.0`  | Bind address.                            |

### LLM (per [ADR-0005](../../docs/adr/0005-multi-provider-llm.md))

| Env var                              | Required when                 | Default                                          |
| ------------------------------------ | ----------------------------- | ------------------------------------------------ |
| `MEETING_COMPANION_LLM_PROVIDER`     | no                            | `bedrock` (`bedrock` \| `openai` \| `anthropic`) |
| `MEETING_COMPANION_LLM_MODEL_ID`     | no                            | provider-specific                                |
| `MEETING_COMPANION_LLM_DISABLED`     | no                            | unset (set to skip extraction entirely)          |
| **Bedrock**                          | when `LLM_PROVIDER=bedrock`   |                                                  |
| AWS credentials (any standard chain) | yes                           | —                                                |
| `MEETING_COMPANION_LLM_REGION`       | no                            | `us-west-2`                                      |
| **OpenAI**                           | when `LLM_PROVIDER=openai`    |                                                  |
| `OPENAI_API_KEY`                     | yes                           | —                                                |
| **Anthropic**                        | when `LLM_PROVIDER=anthropic` |                                                  |
| `ANTHROPIC_API_KEY`                  | yes                           | —                                                |

### STT and audio (per [ADR-0006](../../docs/adr/0006-live-audio-stt-pipeline.md))

| Env var                                  | Default                                                                         |
| ---------------------------------------- | ------------------------------------------------------------------------------- |
| `MEETING_COMPANION_STT_PROVIDER`         | `soniox` (`soniox` \| `mock`)                                                   |
| `SONIOX_API_KEY`                         | — (required when provider is `soniox`)                                          |
| `MEETING_COMPANION_SONIOX_MODEL`         | `stt-rt-preview`                                                                |
| `MEETING_COMPANION_AUDIO_DISABLED`       | unset (set to skip audio capture; mock STT still emits canned chunks if active) |
| `MEETING_COMPANION_STT_MOCK_INTERVAL_MS` | `3000`                                                                          |

### Summarizer cadences (per [ADR-0007](../../docs/adr/0007-summarizer-architecture.md))

| Env var                                        | Default |
| ---------------------------------------------- | ------- |
| `MEETING_COMPANION_HIGHLIGHTS_INTERVAL_MS`     | `20000` |
| `MEETING_COMPANION_ACTIONS_INTERVAL_MS`        | `15000` |
| `MEETING_COMPANION_OPEN_QUESTIONS_INTERVAL_MS` | `15000` |

### mnemo (per [ADR-0008](../../docs/adr/0008-mnemo-memory-integration.md))

| Env var                               | Default                                   |
| ------------------------------------- | ----------------------------------------- |
| `MEETING_COMPANION_MNEMO_URL`         | unset (integration disabled when missing) |
| `MEETING_COMPANION_MNEMO_API_KEY`     | unset (integration disabled when missing) |
| `MEETING_COMPANION_MNEMO_WORKSTATION` | `gethostname()`                           |

## macOS one-time setup

Two TCC permissions are required by the parent terminal process:

1. **Screen Recording** — System Settings → Privacy & Security →
   Screen Recording → enable your terminal app (Ghostty, iTerm2,
   Terminal.app).
2. **Microphone** — System Settings → Privacy & Security →
   Microphone → enable your terminal app.

After granting, **restart your terminal**. macOS doesn't propagate
permission changes to already-running processes.

If a build fails with `Library not loaded: @rpath/libswift_Concurrency.dylib`,
the workspace `.cargo/config.toml` rpath fix isn't being applied —
check that file exists and points at `/usr/lib/swift`.

## Sanity-check the audio path

Before debugging anything else with audio:

```bash
cargo run -p meeting-companion-server --example screencapturekit_spike
afplay /tmp/spike-audio.wav
```

If you hear yourself clearly at the correct speed, the audio capture

- format conversion path is healthy and any transcription issues lie
  downstream (Soniox API, summarizer prompts).

## Live smoke without external services

```bash
just live-smoke
```

Boots the server with `MEETING_COMPANION_STT_PROVIDER=mock` +
`MEETING_COMPANION_LLM_DISABLED=1`. Mock STT emits canned chunks; LLM
calls return empty. Useful for iterating on the PWA UI without burning
Soniox / LLM credits.

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
{"type":"extract_metadata","description":"Q1 budget review with Helix team"}
{"type":"start_meeting","description":"Q1 budget review"}
{"type":"set_mode","mode":"transcript"}
{"type":"set_metadata","key":"owner","value":"tiago"}
{"type":"stop_meeting"}
```

## Known limitations

- **macOS-only audio capture.** ScreenCaptureKit is the only documented
  Apple API for system audio + mic without a virtual audio device.
  Linux / Windows are not supported.
- **Audio mixing is naive.** System audio + mic are sample-summed and
  clamped; loudness can clip if both peak simultaneously. STT quality
  is unaffected.
- **Single connected client.** The server accepts one WS at a time.
  Reconnection works (the `Snapshot` is full-state); concurrent
  multi-client sessions don't.
- **Stateless across restarts.** A meeting in flight when the server
  crashes is lost. Acceptable for the personal-project scope.
