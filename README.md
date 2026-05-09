# Meeting Companion

Real-time meeting summarization across **four surfaces**: a Rust
server that owns audio capture / STT / agent reasoning, a native Mac
menu-bar app, a browser PWA (which also drives Even Realities G2
glasses), and a native iOS/Android mobile app via Expo. All four
clients talk the same WebSocket protocol against the same server,
and all four use the same Auth0 tenant for identity.

For the system design and component contracts, see
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).
For the WebSocket wire protocol, see [`docs/PROTOCOL.md`](docs/PROTOCOL.md).
For the PWA design system and interaction patterns, see
[`docs/UX.md`](docs/UX.md).
For the mobile port plan, see [`docs/MOBILE-PLAN.md`](docs/MOBILE-PLAN.md).
For the live roadmap and current status, see [`docs/PLAN.md`](docs/PLAN.md).
For non-obvious decisions, see [`docs/adr/`](docs/adr/).

## Status

| Component                 | Status                                                                                                  |
| ------------------------- | ------------------------------------------------------------------------------------------------------- |
| `packages/server/` (Rust) | Multi-tenant, Postgres-backed, Auth0-authenticated. Agent loop + 4 modes + moments + artifacts + mnemo. |
| `packages/mac/` (Swift)   | Menu-bar app with floating overlay. Sparkle auto-update. Drives glasses when active.                    |
| `packages/pwa/` (TS)      | Browser PWA + EvenHub glasses bridge. Auth0 SPA flow.                                                   |
| `packages/mobile/` (Expo) | iOS + Android. Phases 0–5 of MOBILE-PLAN shipped (control + browse). Audio streaming deferred.          |
| `.github/workflows/` (CI) | 4 workflows: server image → GHCR, Mac bundle + Sparkle, EAS Build, EAS Update.                          |

End-to-end: a client (Mac, PWA, or mobile) starts a meeting and
streams audio. The server runs Soniox STT against the rolling audio,
feeds the transcript through a single tool-calling agent (Claude
Opus 4.7, 1M context, prompt-cached, stateful per meeting), and
broadcasts mode buffers (transcript / highlights / actions /
open_questions / summary / chat) back to every connected client for
that user. Moments and artifacts inject as `[event]` blocks into the
agent's history so chat questions about a just-snapped moment work
end-to-end. mnemo memory streams in (sentence-by-sentence push) and
out (recall at meeting start).

## Repository layout

```
packages/
  server/      Rust WebSocket server (state owner). See packages/server/README.md.
  mac/         Native macOS menu-bar app (SwiftUI). See packages/mac/README.md.
  pwa/         TypeScript PWA + Even Hub glasses bridge. See packages/pwa/README.md.
  mobile/      Expo iOS/Android app. See packages/mobile/README.md.
docs/
  ARCHITECTURE.md   System topology, component split, end-to-end flow.
  PROTOCOL.md       WebSocket contract — intents, events, error codes.
  UX.md             PWA design system and interaction patterns.
  MOBILE-PLAN.md    The mobile port — phases, scope, deferred buckets.
  PLAN.md           Status snapshot + forward roadmap.
  adr/              Architecture Decision Records — the durable "why" record.
.github/workflows/   CI/CD (server-image, mac-bundle, mobile-build, mobile-update).
docker-compose.deploy.yml  Single-VM production deploy (Postgres + GHCR-pulled server).
```

## Prerequisites

Common:

- Rust (stable, 2021 edition).
- Node 20+ with pnpm 9+.
- [`just`](https://github.com/casey/just) for the task runner.
- Postgres 14+ (the server runs migrations on startup; for local dev
  the `just db-up` recipe brings up a Docker container).

Per-surface (only what you actually want to build):

- **Server live audio**: macOS for the legacy local-capture path
  (ScreenCaptureKit). The remote `/audio` WS path makes the server
  itself OS-agnostic when a capture-capable client is attached.
- **Mac app**: Xcode 16+ (macOS 15 SDK; required for the `SCStream`
  microphone path).
- **Mobile app**: Expo SDK 51 (no Xcode needed for builds — EAS
  builds in the cloud), iOS Simulator and/or Android emulator for
  local dev. EAS account if you want OTA updates.

External services (any of which you can skip and the server will
gracefully disable that capability):

- A [Soniox](https://soniox.com/) API key — live STT and dictation.
  Set `MEETING_COMPANION_STT_PROVIDER=mock` for offline dev.
- One of: AWS credentials (Bedrock), an OpenAI API key, or an
  Anthropic API key — agent + summarizers + extraction. Set
  `MEETING_COMPANION_LLM_DISABLED=1` to skip.
- An Auth0 tenant with a SPA client (PWA), a Native client
  (Mac + mobile), and an API audience. Set
  `MEETING_COMPANION_AUTH_DISABLED=1` for local dev with a
  synthetic user.
- A [mnemo](https://github.com/tiagodeoliveira/mnemo) deployment URL
  - API key for cross-meeting memory.

## Install

```bash
pnpm install
```

`pnpm install` also wires the husky pre-commit hook (prettier on
staged JS/TS/JSON/MD/YAML; `cargo fmt --check` on staged Rust files).

## Configure

Copy `.env.example` to `.env` and fill in the keys you need. The
server binary, the `llm_smoke` example, and env-gated integration
tests all auto-load `.env` via `dotenvy`. `.env` is gitignored;
`.env.example` is not.

Key sections:

- **Auth0** — `AUTH0_DOMAIN` + `AUTH0_API_AUDIENCE`. The server
  validates JWTs against Auth0's JWKS; the clients (Mac / PWA /
  mobile) drive their own OAuth flows against the same tenant. Set
  `MEETING_COMPANION_AUTH_DISABLED=1` for local dev without Auth0.
- **LLM provider** — `MEETING_COMPANION_LLM_PROVIDER` (`bedrock` |
  `openai` | `anthropic`) plus provider-specific keys.
- **STT** — `SONIOX_API_KEY`. Or `MEETING_COMPANION_STT_PROVIDER=mock`
  for offline dev.
- **mnemo** — `MEETING_COMPANION_MNEMO_URL` +
  `MEETING_COMPANION_MNEMO_API_KEY`. Unset to disable.
- **Database** — `DATABASE_URL` (Postgres). The local default is
  `postgres://meeting_companion:dev@localhost:5432/meeting_companion`,
  matching the `docker compose up -d postgres` container.

For the PWA's runtime defaults, copy `packages/pwa/.env.example` to
`packages/pwa/.env.local` and fill in `VITE_SERVER_URL` plus the
Auth0 SPA client ID / domain / audience.

For the mobile app, copy `packages/mobile/.env.example` to
`packages/mobile/.env.local` (or use EAS env vars in CI). Same
shape: `EXPO_PUBLIC_SERVER_URL`, `EXPO_PUBLIC_AUTH0_*`.

## Run the integrated stack

The dev stack is the server plus whichever client(s) you want to
exercise:

```bash
# 1. Postgres (one-shot — leave running)
just db-up

# 2. Server (terminal 1)
just server-run        # with Auth0
# or:
just server-run-noauth # MEETING_COMPANION_AUTH_DISABLED=1, synthetic dev user

# 3a. PWA + EvenHub simulator (terminal 2)
just pwa-sim

# 3b. Mac app (terminal 2 alt)
cd packages/mac && swift run

# 3c. Mobile app (terminal 2 alt)
cd packages/mobile && pnpm dev   # opens Expo dev server; scan QR
```

`just stack` prints all the recommended terminal layouts in one
place.

On first run of any client, sign in with Auth0. The clients persist
the refresh token (Mac → Keychain; PWA → bridge / localStorage;
mobile → expo-secure-store) and reconnect the WS automatically on
identity changes.

### Run the server alone

If you just want to poke at the server with `websocat` for protocol
work:

```bash
just server-run-noauth      # MEETING_COMPANION_AUTH_DISABLED=1
just smoke-instructions     # prints websocat one-liners
```

## Test

```bash
just test
```

Runs the Rust server tests (with `--test-threads=1` because heartbeat
tests set process-global env vars), the PWA Vitest suite, and a
strict TypeScript typecheck.

The PWA's integration tests in `packages/pwa/tests/integration/`
require a running EvenHub simulator and are skipped by default. To
run them:

```bash
just pwa-sim                                    # terminal 1
pnpm -F @meeting-companion/pwa test:integration # terminal 2
```

## Format

Pre-commit hook runs prettier on staged JS/TS/JSON/MD/YAML and gates
on `cargo fmt --check` for staged Rust files. To run manually:

```bash
pnpm format        # prettier on JS/TS/JSON/MD/YAML
cargo fmt --all    # rustfmt on every .rs file
```

## Deploy

Production deploys to a single VM via `docker-compose.deploy.yml`:

```bash
# On the VM:
docker login ghcr.io   # PAT classic with read:packages
cp .env.deploy.example .env.deploy && $EDITOR .env.deploy
docker compose -f docker-compose.deploy.yml --env-file .env.deploy up -d
```

The server image is published to GHCR by `.github/workflows/server-image.yml`
on every push to main and on tags. Update flow on the host:

```bash
docker compose -f docker-compose.deploy.yml --env-file .env.deploy pull server
docker compose -f docker-compose.deploy.yml --env-file .env.deploy up -d
```

Mac auto-updates via Sparkle from GitHub Releases — see
[`packages/mac/README.md`](packages/mac/README.md). Mobile updates
via EAS Update — see [`packages/mobile/README.md`](packages/mobile/README.md).

## More

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — system topology and end-to-end flow.
- [`docs/PROTOCOL.md`](docs/PROTOCOL.md) — WebSocket wire protocol reference.
- [`docs/UX.md`](docs/UX.md) — PWA design system and interaction patterns.
- [`docs/MOBILE-PLAN.md`](docs/MOBILE-PLAN.md) — mobile port phases and deferred buckets.
- [`docs/PLAN.md`](docs/PLAN.md) — current status + forward roadmap.
- [`docs/adr/`](docs/adr/) — Architecture Decision Records.
- [`.github/workflows/README.md`](.github/workflows/README.md) — CI/CD secrets, variables, EAS env vars.
