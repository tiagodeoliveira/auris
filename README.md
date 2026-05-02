# Meeting Companion

Real-time meeting summarization for Even Realities G2 glasses, driven by a
laptop server, a phone PWA, and the glasses as a thin display.

For the system design and component contracts, see [`docs/`](docs/).

## Status — Phase 0 complete

| Component                 | Status                                           | Tests                                                                               |
| ------------------------- | ------------------------------------------------ | ----------------------------------------------------------------------------------- |
| `packages/server/` (Rust) | Functional                                       | 79 unit + integration tests, all passing                                            |
| `packages/pwa/` (TS)      | Functional                                       | 64 unit tests, all passing; 2 integration tests skipped (require running simulator) |
| Glasses display           | Renders via the PWA inside the EvenHub simulator | manual smoke checklist in `packages/pwa/README.md`                                  |

End-to-end behavior: the PWA boots inside the EvenHub simulator (or on real
G2 glasses), renders the four glasses layouts, captures audio for the meeting
description via Soniox STT, and stays in sync with the laptop server's mock
content over WebSocket. See [`docs/specs/`](docs/specs/) for the full
contract; see [`docs/superpowers/plans/`](docs/superpowers/plans/) for what
each task did.

## Repository layout

```
packages/
  server/      Rust WebSocket server (state owner). See packages/server/README.md.
  pwa/         TypeScript PWA — Even Hub plugin. See packages/pwa/README.md.
docs/
  ARCHITECTURE.md           System-level spec.
  adr/                      Architecture Decision Records (load-bearing decisions).
  specs/                    Per-component specs (server.md, pwa.md).
  superpowers/plans/        Implementation plans derived from the specs.
```

## Prerequisites

- Rust (stable, 2021 edition).
- Node 20+ with pnpm 9+.
- [`just`](https://github.com/casey/just) for the task runner (recommended).
- A [Soniox](https://soniox.com/) API key for the meeting-description STT
  flow (optional — the PWA shows an error toast if missing and falls back to
  empty-description meetings).

## Install

```bash
pnpm install
```

`pnpm install` also wires the husky pre-commit hook (prettier on staged
JS/TS/JSON/MD/YAML; `cargo fmt --check` on staged Rust files).

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

(This sets `MEETING_COMPANION_TOKEN=dev` and runs the Rust server on `:7331`.)

**Terminal 2 — PWA + simulator (combined):**

```bash
just pwa-sim
```

(This runs the Vite dev server + boots the EvenHub simulator pointed at it,
both via `concurrently`. The simulator window pops up; click "Open simulator"
or wait for the `app-ready` log line.)

On first run, the PWA settings modal opens. Enter:

- **Server URL:** `ws://localhost:7331`
- **Server token:** `dev`
- **Soniox API key:** your Soniox key (or leave blank to skip the
  description-listening flow)

These persist across restarts via `bridge.setLocalStorage` (per
[ADR-0003](docs/adr/0003-persistence-via-bridge.md)).

To skip retyping in development, copy `packages/pwa/.env.example` to
`packages/pwa/.env.local` and fill in the `VITE_DEFAULT_*` variables — the
PWA seeds first-run defaults from these.

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
#   {"type":"start_meeting"}
#   {"type":"set_mode","mode":"transcript"}
#   {"type":"stop_meeting"}
```

## Test

```bash
just test
```

Runs the Rust server tests (with `--test-threads=1` because heartbeat tests
set a process-global env var) plus the PWA Vitest suite plus a strict
TypeScript typecheck.

The PWA's integration tests in `packages/pwa/tests/integration/` require a
running EvenHub simulator and are skipped by default. To run them:

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

Phase 0 is complete. The natural follow-ups, in rough order:

1. **Phase 1 hardware sideload** — the manual checklist in
   [`packages/pwa/README.md`](packages/pwa/README.md). Walk through the
   eleven items, including:
   - **ADR-0001 follow-up**: log raw `bridge.onEvenHubEvent` payloads on
     temple tap, ring tap, and swipe; identify which field carries the
     [`EventSourceType`](docs/specs/pwa.md) source distinction (G2 left vs
     right vs R1 ring); document findings in
     [`docs/specs/pwa.md`](docs/specs/pwa.md) §10.4.
   - **ADR-0003 follow-up**: confirm `bridge.setLocalStorage` actually
     persists across a full Even Realities App restart (not just an in-app
     reload).
   - **Font-measurement recalibration** for `LINES_PER_SCREEN` and
     `CHARS_PER_LINE` in `packages/pwa/src/glasses/layout-active-list.ts`,
     using the [`everything-evenhub:font-measurement`](https://hub.evenrealities.com/docs/) skill.
2. **Phase 2 real audio + extraction pipeline** —
   [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) §10 steps 15-18:
   - Wire `screencapturekit` audio capture in the Rust server.
   - Wire real STT and summarization (current pipeline is mocked items every 3s).
   - Wire real LLM metadata extraction (current is the deterministic stub
     in [`docs/specs/server.md`](docs/specs/server.md) §8.4).
   - Memory-system integration for project-aware modes.
3. **Production deployment** —
   [`docs/specs/pwa.md`](docs/specs/pwa.md) §11.4: TLS termination on the
   server (Tailscale Funnel / cloudflared), `evenhub pack` to a `.ehpk`,
   upload to the Even Hub developer portal.
4. **Optional Phase 1 UX improvements** —
   [ADR-0001](docs/adr/0001-gesture-map.md) leaves open whether to promote
   `DOUBLE_CLICK_EVENT` to a lifecycle gesture (start meeting / confirm
   stop) once hardware is in hand and accidental-trigger rates can be
   characterized.

## More

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — system topology, component split, end-to-end flow.
- [`docs/specs/`](docs/specs/) — per-component specs (the contract for each implementation).
- [`docs/adr/`](docs/adr/) — Architecture Decision Records.
- [`docs/superpowers/plans/`](docs/superpowers/plans/) — implementation plans (executed).
