# auris-server

Multi-tenant Rust WebSocket + REST server. Owns per-user meeting
state, ingests audio (local ScreenCaptureKit or remote `/audio` WS),
runs streaming STT, drives a tool-calling agent loop (Claude Opus
4.7) plus moment / artifact / summary summarizers, persists to
Postgres + filesystem blobs, and pushes / recalls memories against
[mnemo](https://github.com/tiagodeoliveira/mnemo).

System overview: [`docs/ARCHITECTURE.md`](../../docs/ARCHITECTURE.md).
Wire protocol: [`docs/PROTOCOL.md`](../../docs/PROTOCOL.md).
Decisions: [`docs/adr/`](../../docs/adr/).

## Run

```bash
just db-up              # Postgres in Docker (one-shot, leave running)
just server-run         # with Auth0 (uses the dev tenant baked into justfile)
```

For local dev without Auth0:

```bash
just server-run-noauth  # AURIS_AUTH_DISABLED=1
```

## Test

```bash
cargo test -p auris-server -- --test-threads=1
```

`--test-threads=1` is required because the heartbeat tests set a
process-global env var (`AURIS_HEARTBEAT_MS`); running
other test binaries in parallel would inherit that override.

## What it does

Per-user meeting lifecycle:

1. Client opens a WS with a JWT in the query string. `auth.rs`
   validates against Auth0's JWKS and binds the connection to the
   user's `UserState`. Server emits `Snapshot` (full per-user state).
2. Client sends `start_meeting` with optional `description` and
   `audio_source_device_id`. Server starts the meeting immediately
   and, if `description` is non-empty, spawns an async LLM extraction
   task that broadcasts `metadata_changed` whenever tags arrive (no
   client-side wait required). `spawn_live_pipeline` brings up:
   - audio source — local ScreenCaptureKit OR a remote
     `/audio` WS-bound device (Mac client streaming PCM frames).
   - STT adapter — Soniox WS streaming, or `mock` for offline dev.
   - **transcript pass-through** — each finalized chunk emits as a
     transcript-mode item (no LLM).
   - **agent loop** — single tool-calling LLM task per active
     meeting, stateful `Vec<rig::Message>` history, prompt-cached.
     Tools: `push_highlight`, `replace_highlights`, `push_action`,
     `push_open_question`, `fetch_artifact_summary`,
     `fetch_artifact`. Replies in chat mode via Q+A bubble pairs.
   - **summary loop** — running 3-5 sentence summary, single-item
     Replace; hybrid trigger (token threshold OR 5-min ceiling).
3. mnemo recaller fires one `GET /recall` and writes
   `state.recalled_context`; emits `prior_context_changed`.
4. Each transcript sentence:
   - appends to `rolling_transcript`,
   - broadcasts `items_update { mode: "transcript" }`,
   - streams a `user`-role turn to mnemo,
   - appends to `<DATA_DIR>/blobs/meetings/<meeting_id>/transcription.jsonl`.
5. The agent fires when any of: ~200 new tokens accumulate, 4
   sentences accumulate, 4s of silence, 30s since last fire, or a
   kick (artifact attached, chat message, moment marked, moment
   summarized). Each fire drains the chunk buffer, calls
   `agent.prompt(...).with_history(history.clone())`, executes the
   resulting tool calls (`push_*`/`replace_highlights` mutate state
   and broadcast), and appends the new agent turns onto history.
6. `mark_moment` → server inserts the row, asks the bound
   `screen_capture` device for a screenshot via
   `capture_moment_screenshot`, persists to disk, schedules a
   vision-LLM moment summarizer. The summarizer kicks the agent on
   completion so chat questions about the moment have full context.
7. `stop_meeting` → state goes Idle. Cleanup: cancel meeting tasks,
   cancel in-flight extraction, push final `assistant`-role summary
   bundle to mnemo, clear in-memory `state`.

**Boot recovery.** A meeting Active when the server crashed remains
in Postgres with `ended_at IS NULL`. On startup, `db.rs` scans for
these rows (cheap via the partial index `idx_meetings_active`),
respawns the live pipeline for each, and broadcasts a synthetic
state-change event so reconnecting clients see Active. The previous
WS audio source is gone — the recovered meeting sits idle until a
capture-capable client reconnects and binds.

## Configuration

All env vars below are optional unless noted. Copy `.env.example`
(at the workspace root) to `.env` and fill in the keys you need;
`dotenvy` auto-loads it.

### Server

| Env / Flag                       | Default    | Description                                                                                                                                                                                                                                        |
| -------------------------------- | ---------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `DATABASE_URL`                   | local Pg   | Postgres connection URL.                                                                                                                                                                                                                           |
| `AURIS_DATA_DIR`                 | `./data`   | Root for blob storage (`<DATA_DIR>/blobs/...`).                                                                                                                                                                                                    |
| `AUTH0_DOMAIN`                   | (required) | Auth0 tenant domain (e.g. `your-tenant.us.auth0.com`).                                                                                                                                                                                             |
| `AUTH0_API_AUDIENCE`             | (required) | Auth0 API identifier the JWT must `aud`-match.                                                                                                                                                                                                     |
| `AURIS_AUTH_DISABLED`            | unset      | When set, bypasses Auth0 and uses a synthetic dev user.                                                                                                                                                                                            |
| `AURIS_TRUST_PROXY`              | unset      | When set, rate-limit the public `/pair/*` endpoints by the left-most `X-Forwarded-For` entry (auris is behind the kleos proxy). Leave unset for direct exposure — the socket peer is used instead, and the global ceiling still bounds total work. |
| `AURIS_SKIP_BOOT_RECOVERY`       | unset      | Skip boot recovery: respawning Active meetings, the sweep that fails restart-orphaned `wrap_up_status='running'` rows, AND re-kicking interrupted wrap-ups.                                                                                        |
| `AURIS_SHUTDOWN_GRACE_MS`        | `20000`    | Bounded shutdown wait for in-flight finalize / wrap-up tasks (after the 2 s drain). `0` skips the wait; the boot re-kick still recovers.                                                                                                           |
| `AURIS_HEARTBEAT_MS`             | `10000`    | Heartbeat interval (test override only).                                                                                                                                                                                                           |
| `AURIS_SWEEP_GRACE_SECS`         | `3600`     | Orphan-blob sweep: minimum file age before an unmatched blob is reaped.                                                                                                                                                                            |
| `AURIS_SWEEP_INTERVAL_SECS`      | `86400`    | Orphan-blob sweep: seconds between runs.                                                                                                                                                                                                           |
| `AURIS_SWEEP_INITIAL_DELAY_SECS` | `600`      | Orphan-blob sweep: delay after boot before the first run.                                                                                                                                                                                          |
| `AURIS_SWEEP_DRY_RUN`            | unset      | When set, the sweep only logs its `SweepReport`; nothing is deleted or healed.                                                                                                                                                                     |
| `RUST_LOG`                       | `info`     | `tracing-subscriber` filter.                                                                                                                                                                                                                       |
| `--port`                         | `7331`     | TCP port.                                                                                                                                                                                                                                          |
| `--bind`                         | `0.0.0.0`  | Bind address.                                                                                                                                                                                                                                      |

> **Deploy note (kleos):** the shutdown sequence now waits up to
> `AURIS_SHUTDOWN_GRACE_MS` (default 20 s) for an in-flight meeting
> finalize before exiting. Docker's default `stop_grace_period` is
> 10 s, after which it SIGKILLs the container — set
> `stop_grace_period: 30s` on the `auris` service in the kleos compose
> file so the wait can actually complete. If a deploy does get killed
> mid-finalize anyway (SIGKILL, OOM, panic), the next boot re-kicks the
> interrupted wrap-up from the persisted transcript; only the last few
> seconds of drained STT tail are lost.

### LLM (per [ADR-0005](../../docs/adr/0005-multi-provider-llm.md), [ADR-0011](../../docs/adr/0011-agentic-summarizer-loop.md))

| Env var                              | Required when                            | Default                                                                                                                                                         |
| ------------------------------------ | ---------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `AURIS_LLM_CHAT_PROVIDER`            | always                                   | none — `bedrock` \| `openai` \| `anthropic` \| `gemini` \| `xai` (server fails to start if unset)                                                               |
| `AURIS_LLM_CHAT_MODEL_ID`            | always                                   | none — model id for the chat agent loop (recommended: `claude-opus-4-7`)                                                                                        |
| `AURIS_LLM_BACKGROUND_PROVIDER`      | always                                   | none — same provider set as chat (recommended: `anthropic`)                                                                                                     |
| `AURIS_LLM_BACKGROUND_MODEL_ID`      | always                                   | none — model id for summary/moment/artifact/wrap-up/metadata (recommended: `claude-haiku-4-5-20251001`)                                                         |
| `AURIS_LLM_DISABLED`                 | no                                       | unset (set to skip every LLM call across both pools)                                                                                                            |
| `AGENT_LOG_PROMPT`                   | no                                       | unset (set to `1` to log the full agent prompt on each fire)                                                                                                    |
| **Bedrock**                          | when either pool's `_PROVIDER=bedrock`   |                                                                                                                                                                 |
| AWS credentials (any standard chain) | yes                                      | —                                                                                                                                                               |
| `AURIS_LLM_REGION`                   | no                                       | `us-west-2` (global across pools)                                                                                                                               |
| **OpenAI**                           | when either pool's `_PROVIDER=openai`    |                                                                                                                                                                 |
| `OPENAI_API_KEY`                     | yes                                      | —                                                                                                                                                               |
| **Anthropic**                        | when either pool's `_PROVIDER=anthropic` |                                                                                                                                                                 |
| `ANTHROPIC_API_KEY`                  | yes                                      | —                                                                                                                                                               |
| **Gemini**                           | when either pool's `_PROVIDER=gemini`    |                                                                                                                                                                 |
| `GEMINI_API_KEY`                     | yes                                      | —                                                                                                                                                               |
| **xAI / Grok**                       | when either pool's `_PROVIDER=xai`       | No native PDF support — if the background pool is `xai`, PDF artifact uploads fail at call time. Use `anthropic` / `bedrock` / `gemini` for PDF-heavy meetings. |
| `XAI_API_KEY`                        | yes                                      | —                                                                                                                                                               |

### STT and audio (per [ADR-0006](../../docs/adr/0006-live-audio-stt-pipeline.md))

| Env var                      | Default                                                                         |
| ---------------------------- | ------------------------------------------------------------------------------- |
| `AURIS_STT_PROVIDER`         | `soniox` (`soniox` \| `mock`). Legacy: `AURIS_STT_MOCK=1` is also accepted.     |
| `SONIOX_API_KEY`             | — (required when provider is `soniox`)                                          |
| `AURIS_SONIOX_MODEL`         | `stt-rt-preview`                                                                |
| `AURIS_AUDIO_DISABLED`       | unset (set to skip audio capture; mock STT still emits canned chunks if active) |
| `AURIS_STT_MOCK_INTERVAL_MS` | `3000`                                                                          |

The audio source per meeting (local ScreenCaptureKit vs. a remote
`/audio` WS-bound device) is selected at runtime via the
`audio_source_device_id` field on `start_meeting` — there's no
global env-var switch for it.

### Agent / summary cadences (per [ADR-0011](../../docs/adr/0011-agentic-summarizer-loop.md))

The agent fires hybrid: token threshold OR sentence count OR silence
window OR hard cap (whichever first). Each is independently tunable;
defaults work well in practice.

| Env var                    | Default  | Notes                                                     |
| -------------------------- | -------- | --------------------------------------------------------- |
| `AGENT_TRIGGER_TOKENS`     | `200`    | New transcript tokens that trigger a fire.                |
| `AGENT_TRIGGER_SENTENCES`  | `4`      | New sentences that trigger a fire.                        |
| `AGENT_TRIGGER_SILENCE_MS` | `4000`   | Silence (no new chunks) that triggers a fire.             |
| `AGENT_TRIGGER_MAX_MS`     | `30000`  | Hard cap between fires while a meeting is active.         |
| `SUMMARY_TRIGGER_TOKENS`   | `500`    | Tokens since last summary fire.                           |
| `SUMMARY_BOOTSTRAP_TOKENS` | `100`    | Min tokens before the first summary fires.                |
| `SUMMARY_TRIGGER_MAX_MS`   | `300000` | Hard ceiling for summary refresh (5 min).                 |
| `AURIS_MOMENT_WINDOW_MS`   | `60000`  | Window of transcript context the moment summarizer reads. |
| `AURIS_MOMENT_GRACE_MS`    | `5000`   | Grace after the moment timestamp to accumulate context.   |

### mnemo (per [ADR-0008](../../docs/adr/0008-mnemo-memory-integration.md))

| Env var                   | Default                                   |
| ------------------------- | ----------------------------------------- |
| `AURIS_MNEMO_URL`         | unset (integration disabled when missing) |
| `AURIS_MNEMO_API_KEY`     | unset (integration disabled when missing) |
| `AURIS_MNEMO_WORKSTATION` | `gethostname()`                           |

## Persistence

- **Postgres** — `users`, `meetings`, `moments`, `artifacts`,
  `meeting_artifacts`. Migrations run on startup (`migrations/`).
- **Filesystem blobs** — `<DATA_DIR>/blobs/meetings/<meeting_id>/`
  (`transcription.jsonl`, `moments/<moment_id>.jpg`) and
  `<DATA_DIR>/blobs/artifacts/<artifact_id>/<filename>`. The shape
  is intentionally S3-compatible for an additive `S3BlobStore` swap
  when horizontal scale arrives.
- **mnemo** — cross-meeting memory.
- **In-memory only** — items per mode, devices, recalled context.
  Re-derivable from the transcript JSONL if we ever need a "review"
  feature.

## macOS one-time setup (local audio capture)

Two TCC permissions are required by the parent terminal process:

1. **Screen Recording** — System Settings → Privacy & Security →
   Screen Recording → enable your terminal app.
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
cargo run -p auris-server --example screencapturekit_spike
afplay /tmp/spike-audio.wav
```

If you hear yourself clearly at the correct speed, the audio capture

- format conversion path is healthy and any transcription issues lie
  downstream (Soniox API, agent prompts, etc.).

## Live smoke without external services

```bash
just live-smoke
```

Boots the server with mock STT (`AURIS_STT_MOCK=1`) +
`AURIS_LLM_DISABLED=1` + `AURIS_AUDIO_DISABLED=1`.
Mock STT emits canned chunks; LLM calls return empty. Useful for
iterating on the UI without burning Soniox / LLM credits. Note that
this recipe leaves Auth0 enabled — set `AURIS_AUTH_DISABLED=1`
manually if you want to skip auth too.

## Manual smoke

In one terminal:

```bash
just server-run-noauth     # AURIS_AUTH_DISABLED=1
```

In another:

```bash
websocat 'ws://localhost:7331/?token=ignored-when-auth-disabled'
```

Paste intents to interact:

```json
{"type":"start_meeting","description":"Q1 budget review with Helix team"}
{"type":"set_mode","mode":"summary"}
{"type":"chat","text":"What are the open questions so far?"}
{"type":"set_metadata","key":"owner","value":"tiago"}
{"type":"stop_meeting"}
```

## Container image

Built and pushed to GHCR by `.github/workflows/server-image.yml` on
every push to main and on tags. Pull from a deploy host:

```bash
docker login ghcr.io   # PAT classic with read:packages
docker pull ghcr.io/tiagodeoliveira/auris-server:latest
```

Deployment topology — Postgres, TLS, backups — lives in the
[kleos repo](https://github.com/tiagodeoliveira/kleos); this image
is one of the moving parts kleos wires up alongside mnemo. See
[`.github/workflows/README.md`](../../.github/workflows/README.md)
for build-time variables and deploy secrets.

## Known limitations

- **Local audio capture is macOS-only.** ScreenCaptureKit is the
  only documented Apple API for system audio + mic without a virtual
  audio device. The remote `/audio` WS path makes the server itself
  OS-agnostic when a capture-capable client (Mac, mobile-with-mic)
  is attached.
- **Audio mixing is naive.** System audio + mic are sample-summed
  and clamped; loudness can clip if both peak simultaneously. STT
  quality is unaffected.
- **Single audio source per meeting.** Only one device's `/audio`
  stream feeds a meeting at a time. Re-binding requires
  stop+restart.
