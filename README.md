# Meeting Companion

Real-time meeting summarization for Even Realities G2 glasses, driven by a
laptop server, a phone PWA, and the glasses as a thin display.

For the system design and component contracts, see [`docs/`](docs/).

## Repository layout

```
packages/
  server/      Rust WebSocket server (state owner). See packages/server/README.md.
  contract/    Shared TypeScript WS contract types.
  pwa/         Phone PWA (forthcoming).
docs/
  ARCHITECTURE.md           System-level spec.
  specs/                    Per-component specs (server, pwa).
  superpowers/plans/        Implementation plans derived from the specs.
```

## Prerequisites

- Rust (stable, 2021 edition).
- Node 20+ with pnpm 9+.
- [`just`](https://github.com/casey/just) for the task runner (optional but recommended).

## Install

```bash
pnpm install
```

`pnpm install` also wires the husky pre-commit hook (formatting + `cargo fmt --check`).

## Run

```bash
just server-run
```

Starts the stub server on `:7331` with token `dev`. Connect from the PWA (or
`websocat` for manual testing) at `ws://localhost:7331/?token=dev`.

Without `just`:

```bash
MEETING_COMPANION_TOKEN=dev cargo run -p meeting-companion-server -- --port 7331
```

## Test

```bash
just test
```

Runs Rust integration + unit tests (`--test-threads=1` is required because
heartbeat tests set a process-global env var) and the TS contract typecheck.

## Format

Pre-commit hook runs prettier on staged JS/TS/JSON/MD/YAML and gates on
`cargo fmt --check` for staged Rust files. To run manually:

```bash
pnpm format        # prettier on JS/TS/JSON/MD/YAML
cargo fmt --all    # rustfmt on every .rs file
```

## More

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — system topology, component split, end-to-end flow.
- [`docs/specs/`](docs/specs/) — per-component specs (the contract for each implementation).
- [`docs/superpowers/plans/`](docs/superpowers/plans/) — implementation plans.
