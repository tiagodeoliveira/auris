# Smoke test harness

End-to-end smoke for auris-server. Brings up an isolated `auris-smoke`
docker-compose stack (postgres + auris-server, both as containers),
replays a committed WAV fixture through the `/audio` WebSocket, and
asserts on DB state + server log lines + LLM-produced item content.

Designed to run **before a kleos deploy** that touches the server or
LLM config. Not wired into `cargo test`, `just`, or CI.

See `docs/superpowers/specs/2026-05-24-smoke-test-design.md` for the
full design.

## Prerequisites

1. **Python 3.11+** with `pip`.
2. **Docker** with the Compose v2 plugin (`docker compose ...`).
   No host `cargo` or `psql` required — the server and DB both run as
   containers, queries run via `docker exec`.
3. **API keys for the LLM providers and Soniox.** See the cost note below.
4. **Install the Python deps once:**
   ```
   pip install -r scripts/smoke/requirements.txt
   ```
5. **Copy and fill in the env file:**
   ```
   cp scripts/smoke/.env.test.example scripts/smoke/.env.test
   $EDITOR scripts/smoke/.env.test
   ```
   `.env.test` is gitignored — your real keys never enter the repo.

## Recording the audio fixture

The repo ships a 2-minute placeholder `fixture.wav` of pure silence so
the harness can be developed and smoke-tested mechanically. Before
the first **real** smoke run, replace it with a recording of you (or
anyone) reading `script.txt` aloud. Why: the keyword-presence
assertion (A9) needs real speech to match against.

1. Record ~2 minutes reading `script.txt` aloud. Use any voice memo
   app (QuickTime on macOS, Voice Recorder on Windows, `arecord` on
   Linux). Output format doesn't matter at this stage.
2. Convert to the required PCM format:
   ```
   ffmpeg -i input.m4a -ac 1 -ar 16000 -sample_fmt s16 scripts/smoke/fixture.wav
   ```
   Required output: 16-bit signed PCM, 16 kHz sample rate, mono, WAV
   container. Verify with `file scripts/smoke/fixture.wav` — it must
   say `WAVE audio, Microsoft PCM, 16 bit, mono 16000 Hz`.
3. Commit the new `fixture.wav`.

## Running the smoke

From the repo root:

```
python scripts/smoke/smoke.py
```

The harness will:

1. Load `scripts/smoke/.env.test` (exits 1 if missing).
2. Tear down any existing `auris-smoke` compose stack and bring up a
   fresh postgres container (`smoke-postgres`).
3. Build the server image via `docker compose build server` — first
   call compiles the Rust release binary (~5-10 min on a cold
   BuildKit cache); subsequent calls hit the cache and return in
   seconds.
4. `docker compose up -d server` — depends_on with `condition:
   service_healthy` gates this on postgres + the server's built-in
   `auris-server healthz` healthcheck. Polls
   `docker inspect smoke-server` until healthy (60 s timeout).
5. Tails `docker logs -f smoke-server` to `scripts/smoke/artifacts/server.log`
   in the background.
6. Run a meeting: register a device, start the meeting, replay
   `fixture.wav` at realtime cadence (~2 min), stop the meeting,
   drain events.
7. Run 9 assertions on DB state (via `docker exec smoke-postgres
   psql ...`) + server log + LLM-produced items.
8. Print `PASS: <name>` / `FAIL: <name>` per assertion.
9. Exit 0 if all PASS, 1 if any FAIL.

Total runtime: ~3 minutes (2 min audio + 15 s STT tail + 30 s
post-stop drain + assertion queries). First run adds the Rust release
build time (~5-10 min on a cold cache).

### Flags

- `--persist` — KEEP the auris-smoke compose stack running and leave
  `artifacts/` on disk after the run, so you can `docker exec
  smoke-postgres psql ...` ad-hoc and inspect `server.log` /
  `events.jsonl` / `db_dump.txt`. **Default (without this flag):
  tear the stack down (`docker compose down -v`) and wipe the
  artifacts dir** so every run starts from a clean slate.
- `--no-stack` — skip the down/build/up steps; assume the auris-smoke
  stack is already up. Useful when iterating on the assertion code
  without paying the build + boot cost each run. Implies `--persist`
  (the cleanup step is suppressed since we didn't bring the stack
  up in this run).

### Artifacts

After each run, `scripts/smoke/artifacts/` contains:

- `server.log` — full server stdout+stderr tailed from `docker logs -f`.
- `events.jsonl` — every WS event the smoke received, one per line.
- `db_dump.txt` — summary of the DB state checked by assertions.

The directory is gitignored.

## Cost per run

Real API calls — not free. Estimate at ~$0.05-0.20 per run depending
on which models are configured in `.env.test`:

- LLM (chat + background pools): ~$0.04-0.18 for a 2-minute meeting
  on Anthropic Opus 4.7 (chat) + Haiku 4.5 (background).
- Soniox STT: ~$0.003/min × 2 min ≈ $0.006.

Cheaper models reduce the bill proportionally. Setting
`AURIS_LLM_DISABLED=1` in `.env.test` skips LLM calls entirely (for
free harness-mechanics verification, but A4/A5/A6/A7/A9 will FAIL).

## Troubleshooting

### `smoke: env file not found`

Copy `.env.test.example` to `.env.test` and fill in real values.

### `smoke: postgres did not become healthy within 30s`

Check `docker ps` and `docker logs smoke-postgres`. Common cause:
the postgres image is still pulling on a cold daemon.

### `smoke: smoke-server did not become healthy within 60s`

The harness prints the last 20 lines of `artifacts/server.log` on
timeout. Most common causes:

- Missing or wrong API keys in `.env.test` (the server validates
  credentials at first LLM call, not at boot, but boot still fails
  if the LLM env vars are syntactically wrong — e.g. missing
  `AURIS_LLM_CHAT_PROVIDER`).
- First docker build hasn't completed yet — the build runs to
  completion before `up -d server` is even called, so this only
  matters if the build itself is still in flight.

### Assertions A3-A6 FAIL but A1, A2, A7, A8 PASS

If you're running against the silent placeholder fixture (or with
`AURIS_LLM_DISABLED=1`), this is expected — the agent and
summarizers have no transcript content to work on. Record a real
fixture (see "Recording the audio fixture" above) and unset the
disable flag.

### Assertion A9 FAILs with missing keywords

The agent / summary produced items, but none mention the expected
keywords from `script.txt`. Possible causes:

- Audio quality was poor → STT transcribed something different than
  the script. Listen to `fixture.wav` and check the transcript items
  in `artifacts/db_dump.txt`.
- Keywords in `script.txt` are too generic (e.g. "the") and got
  paraphrased away by the LLM. Pick more distinctive keywords.

### `meeting_llm_usage` has 0 rows or only 1 row

The split-llm-pool work isn't fully landed in the image you built,
OR `AURIS_LLM_DISABLED=1` snuck into `.env.test`. Check both.

## What this catches

- Both LLM pools wire correctly and produce log lines + DB rows.
- Real STT → real LLM pipeline works end-to-end inside a container.
- Per-pool drain attributes correct counts to correct (provider,
  model) pairs.
- Agent loop receives full meeting context (anchor keywords from the
  audio appear in highlight / summary / action items).
- Wrap-up extractor runs and writes actions + open_questions.

## What this does NOT catch (v1)

- Chat intent (`Intent::Chat`) — not exercised. Add a `--with-chat`
  flag if the chat surface ever regresses.
- Moment + artifact paths — not exercised. Same story.
- Concurrent users — single-user run only.
- Mac / mobile / PWA client behavior — server-side only.
- Auth0 / paired-device JWT flows — `AURIS_AUTH_DISABLED=1`.
