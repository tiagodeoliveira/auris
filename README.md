# Meeting Companion

Personal project. See [`meeting-companion-architecture.md`](meeting-companion-architecture.md)
for the system spec and [`docs/specs/`](docs/specs/) for component specs.

## Structure

- `packages/server/` — Rust WebSocket server (state owner). See [its README](packages/server/README.md).
- `packages/pwa/` — TypeScript PWA (forthcoming).
- `packages/contract/` — shared TS contract types.

## Quick start

```bash
just server-run     # starts the stub server on :7331 with token "dev"
just test           # runs all tests (Rust + TS typecheck)
```

## Plans & specs

- [`docs/specs/`](docs/specs/) — component specs (server, pwa).
- [`docs/superpowers/plans/`](docs/superpowers/plans/) — implementation plans derived from the specs.

## Development

Pre-commit hooks (husky + lint-staged + prettier + `cargo fmt --check`) run automatically on staged files. To format manually:

```bash
pnpm format        # prettier on JS/TS/JSON/MD/YAML
cargo fmt --all    # rustfmt on all .rs files
```
