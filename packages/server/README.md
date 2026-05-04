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

## Live audio + STT + parallel mode summarizers (Phase 2 step 15)

When a meeting is `Active`, the server runs four tokio tasks under
`meeting_cancel`'s child tokens:

1. **Audio capture** — macOS ScreenCaptureKit captures both system
   audio and microphone. A 50 fps tokio mixer sums them sample-by-sample
   and emits 16 kHz mono S16LE PCM frames on a bounded mpsc.
2. **STT provider** — pluggable via `SttProvider` trait. `mock` for
   offline dev, `soniox` for production.
3. **Four summarizers in parallel:**
   - `transcript`: pass-through, no LLM. One Item per Soniox utterance.
   - `highlights`: rig Extractor on a 20 s heartbeat. Replace strategy.
   - `actions`: rig Extractor on a 15 s heartbeat. Append + dedupe.
   - `open_questions`: rig Extractor on a 15 s heartbeat. Captures
     pending questions (asked but unanswered) and clarification
     opportunities (gaps user might have missed while multi-tasking).
     Append + dedupe.

PWA's `set_mode` is a _display filter_, not a producer switch — all
three modes accumulate items continuously while the meeting is active.

### Configuration (additions over §LLM)

| Env var                                        | Required when         | Default                            |
| ---------------------------------------------- | --------------------- | ---------------------------------- |
| `SONIOX_API_KEY`                               | `STT_PROVIDER=soniox` | —                                  |
| `MEETING_COMPANION_STT_PROVIDER`               | no                    | `soniox`                           |
| `MEETING_COMPANION_AUDIO_DISABLED`             | no                    | unset                              |
| `MEETING_COMPANION_STT_MOCK`                   | no                    | unset (alias: `STT_PROVIDER=mock`) |
| `MEETING_COMPANION_STT_MOCK_INTERVAL_MS`       | no                    | `3000`                             |
| `MEETING_COMPANION_HIGHLIGHTS_INTERVAL_MS`     | no                    | `20000`                            |
| `MEETING_COMPANION_ACTIONS_INTERVAL_MS`        | no                    | `15000`                            |
| `MEETING_COMPANION_OPEN_QUESTIONS_INTERVAL_MS` | no                    | `15000`                            |

### macOS one-time setup

Two TCC permissions are required by the parent terminal process:

1. **Screen Recording** — System Settings → Privacy & Security → Screen
   Recording → enable your terminal app (Ghostty, iTerm2, Terminal.app).
2. **Microphone** — System Settings → Privacy & Security → Microphone →
   enable your terminal app.

After granting, **restart your terminal**. macOS doesn't propagate
permission changes to already-running processes.

If a build fails with `Library not loaded: @rpath/libswift_Concurrency.dylib`,
the workspace `.cargo/config.toml` rpath fix isn't being applied — check
that file exists and points at `/usr/lib/swift` (it does in the repo;
just confirming for non-standard setups).

### Sanity-check the audio path

Before going through the full server flow, run the SCKit spike:

```bash
cargo run -p meeting-companion-server --example screencapturekit_spike
```

Speak for 5 seconds, then:

```bash
afplay /tmp/spike-audio.wav
```

If you can hear yourself clearly at correct speed, the entire audio
capture + format conversion path is healthy and any transcription
issues lie downstream (Soniox API, summarizer prompts).

### Live smoke (mock STT, no external services)

```bash
just live-smoke
```

Boots the server with `MEETING_COMPANION_STT_MOCK=1` +
`MEETING_COMPANION_LLM_DISABLED=1`. Mock STT emits canned chunks every
2 seconds; transcript items appear, highlights/actions stay empty.
Useful for iterating on PWA UI without burning Soniox credits.

### Known limitations

- **Audio mixing is naive.** System audio + mic are sample-summed and
  clamped; loudness can clip if both peak simultaneously. STT-quality is
  unaffected (Soniox doesn't care). Phase-correctness across the two
  sources is best-effort (per-source ring buffers absorb timing jitter
  but don't align by SCKit timestamps). For meeting-summarization use
  this is fine; if we ever expose audio playback, a proper mixer would
  matter.

- **Sub-word tokenization from Soniox.** `stt-rt-preview` emits tokens
  at sub-word granularity. The client buffers finalized tokens until a
  sentence terminator, a 240-char cap, or a 1 s idle pause, then emits
  one TranscriptChunk per utterance. As a side effect, chunk timestamps
  reflect session-elapsed time rather than per-token offsets.

- **No live interim display.** Soniox's interim (non-final) tokens are
  dropped today. Adding an `Event::TranscriptInterim` emission path
  would give "currently speaking…" feedback at the cost of plumbing
  changes through the `SttProvider` trait. Wire shape already exists
  in `contract.rs`; provider integration is a follow-up.

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
