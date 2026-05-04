# Meeting Companion

Real-time meeting summarization for Even Realities G2 glasses, driven by a
laptop server, a phone PWA, and the glasses as a thin display.

For the system design and component contracts, see
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).
For the WebSocket wire protocol, see [`docs/PROTOCOL.md`](docs/PROTOCOL.md).
For the PWA design system and interaction patterns, see
[`docs/UX.md`](docs/UX.md).
For non-obvious decisions, see [`docs/adr/`](docs/adr/).

## Status

| Component                 | Status                                                                 | Tests                                        |
| ------------------------- | ---------------------------------------------------------------------- | -------------------------------------------- |
| `packages/server/` (Rust) | Phase 2 complete — live audio, STT, parallel summarizers, mnemo memory | 110                                          |
| `packages/pwa/` (TS)      | Phase 2 complete — full UX redesign, mnemo memory badge                | 64 unit + 2 simulator-gated                  |
| Glasses display           | Renders via the PWA inside the EvenHub simulator                       | manual checklist in `packages/pwa/README.md` |

End-to-end: the server captures system audio + microphone via macOS
ScreenCaptureKit, streams it to Soniox for STT, and runs four parallel
LLM-backed summarizers (transcript, highlights, actions, open questions)
against the rolling transcript. The PWA mirrors all of it, drives the
glasses display, and integrates with [mnemo](https://github.com/tiagodeoliveira/mnemo)
to push transcripts and recall prior context across meetings.

## Repository layout

```
packages/
  server/      Rust WebSocket server (state owner). See packages/server/README.md.
  pwa/         TypeScript PWA — Even Hub plugin. See packages/pwa/README.md.
docs/
  ARCHITECTURE.md   System topology, component split, end-to-end flow.
  PROTOCOL.md       WebSocket contract — intents, events, error codes.
  UX.md             PWA design system and interaction patterns.
  adr/              Architecture Decision Records — the durable "why" record.
```

## Prerequisites

- Rust (stable, 2021 edition).
- Node 20+ with pnpm 9+.
- [`just`](https://github.com/casey/just) for the task runner (recommended).
- macOS for live audio capture (Phase 2 / ScreenCaptureKit). Other
  platforms run with audio disabled.
- A [Soniox](https://soniox.com/) API key for live STT and the
  description-dictation flow (optional — the PWA shows an error toast
  if missing).
- One of: AWS credentials (Bedrock), an OpenAI API key, or an
  Anthropic API key for LLM extraction (optional — set
  `MEETING_COMPANION_LLM_DISABLED=1` to skip).
- Optionally: a [mnemo](https://github.com/tiagodeoliveira/mnemo)
  deployment URL + API key for the memory layer.

## Install

```bash
pnpm install
```

`pnpm install` also wires the husky pre-commit hook (prettier on staged
JS/TS/JSON/MD/YAML; `cargo fmt --check` on staged Rust files).

## Configure

Copy `.env.example` to `.env` and fill in the keys you need. The
server binary, the `llm_smoke` example, and the env-gated integration
test all auto-load `.env` via `dotenvy`. `.env` is gitignored;
`.env.example` is not.

Sections in `.env.example`:

- **Server token** — `MEETING_COMPANION_TOKEN` (the PWA passes this on
  the WS query string).
- **LLM provider** — `MEETING_COMPANION_LLM_PROVIDER` (`bedrock` |
  `openai` | `anthropic`) plus provider-specific keys.
- **STT** — `SONIOX_API_KEY`. Or `MEETING_COMPANION_STT_PROVIDER=mock`
  for offline dev with canned chunks. `MEETING_COMPANION_AUDIO_DISABLED=1`
  to skip audio capture (e.g., on Linux).
- **mnemo** — `MEETING_COMPANION_MNEMO_URL` +
  `MEETING_COMPANION_MNEMO_API_KEY`. Unset to disable.

## Run the integrated stack

The integrated stack is three processes:

1. **Laptop server** on `:7331` — owns meeting state, serves WebSocket.
2. **PWA dev server (Vite)** on `:5173` — serves the PWA.
3. **EvenHub simulator** — renders the glasses display, points at the PWA.

`just stack` prints the recommended terminal layout. The simplest path is two
terminals:

**Terminal 1 — server:**

```bash
just server-run
```

**Terminal 2 — PWA + simulator (combined):**

```bash
just pwa-sim
```

(The simulator window pops up; click "Open simulator" or wait for the
`app-ready` log line.)

On first run, open the PWA settings (gear icon, top-right) and enter:

- **Server URL:** `ws://localhost:7331`
- **Server token:** `dev`
- **Soniox API key:** your key (or leave blank to skip dictation)

These persist via `bridge.setLocalStorage` with browser `localStorage`
fallback (per [ADR-0003](docs/adr/0003-persistence-via-bridge.md)).

To skip retyping in development, copy `packages/pwa/.env.example` to
`packages/pwa/.env.local` and fill in the `VITE_DEFAULT_*` variables —
the PWA seeds first-run defaults from these.

### Run the server alone

If you want to poke the server directly without the PWA:

```bash
just server-run
just smoke-instructions   # prints websocat one-liners
```

In a second terminal:

```bash
websocat 'ws://localhost:7331/?token=dev'
# Paste intents like:
#   {"type":"start_meeting","description":"My meeting"}
#   {"type":"set_mode","mode":"transcript"}
#   {"type":"extract_metadata","description":"Helix demo with Tiago"}
#   {"type":"stop_meeting"}
```

## Test

```bash
just test
```

Runs the Rust server tests (with `--test-threads=1` because heartbeat
tests set a process-global env var), plus the PWA Vitest suite, plus a
strict TypeScript typecheck.

The PWA's integration tests in `packages/pwa/tests/integration/`
require a running EvenHub simulator and are skipped by default. To run
them:

```bash
# In one terminal:
just pwa-sim

# In another:
pnpm -F @meeting-companion/pwa test:integration
```

## Format

Pre-commit hook runs prettier on staged JS/TS/JSON/MD/YAML and gates on
`cargo fmt --check` for staged Rust files. To run manually:

```bash
pnpm format        # prettier on JS/TS/JSON/MD/YAML
cargo fmt --all    # rustfmt on every .rs file
```

## Next steps

Phase 0 and Phase 2 are complete. The remaining tracks:

1. **Phase 1 hardware sideload** — the manual checklist in
   [`packages/pwa/README.md`](packages/pwa/README.md). Notable items:
   the [ADR-0001](docs/adr/0001-gesture-map.md) source-field discovery
   on real glasses, [ADR-0003](docs/adr/0003-persistence-via-bridge.md)
   persistence-across-app-restart verification, and font-metrics
   recalibration for the active-list layout.
2. **Production deployment** — TLS termination (Tailscale Funnel /
   cloudflared), `evenhub pack` to a `.ehpk`, upload to the Even Hub
   developer portal.
3. **Future** — meeting-specific mnemo namespace
   (`/meetings/{actorId}/{meetingId}/`) when mnemo's strategy layer
   can support it; multi-meeting browse / recap UI; calendar
   integration.

## More

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — system topology and end-to-end flow.
- [`docs/PROTOCOL.md`](docs/PROTOCOL.md) — WebSocket wire protocol reference.
- [`docs/UX.md`](docs/UX.md) — PWA design system and interaction patterns.
- [`docs/adr/`](docs/adr/) — Architecture Decision Records.
