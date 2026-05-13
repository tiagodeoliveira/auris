default:
    @just --list

# --- Database --------------------------------------------------------------

# Bring up the local Postgres container in the background.
# Connects via DATABASE_URL=postgres://auris:dev@localhost:5432.
db-up:
    docker compose up -d postgres

# Stop the Postgres container (data persists in the named volume).
db-down:
    docker compose stop postgres

# Wipe the database — drops the volume so the next `db-up` starts fresh.
db-reset:
    docker compose down -v

# Open `psql` against the local instance.
db-shell:
    docker compose exec postgres psql -U auris auris

# --- Run -------------------------------------------------------------------

# Run the server with real Auth0 JWT validation (port 7331).
# Both Mac and PWA must present a valid access token for the
# `https://auris.api` audience.
#
# Injects `DATABASE_URL` matching the `just db-up` container so a
# stray `.env` pointing at a hosted DB doesn't get picked up by
# dotenvy.
server-run:
    AUTH0_DOMAIN=dev-jrva0wzk3qkdxcar.us.auth0.com \
    AUTH0_API_AUDIENCE=https://auris.api \
    DATABASE_URL=postgres://auris:dev@localhost:5432/auris \
    cargo run -p auris-server -- --port 7331

# Run the server with auth disabled — every request is attributed to a
# synthetic dev user. Useful for poking the server with `websocat` /
# `curl` without launching a browser flow.
server-run-noauth:
    AURIS_AUTH_DISABLED=1 \
    DATABASE_URL=postgres://auris:dev@localhost:5432/auris \
    cargo run -p auris-server -- --port 7331

# Run the PWA dev server (port 5173). Injects local-targeted
# `VITE_*` vars so a `.env.local` with a remote URL doesn't redirect
# the dev bundle. Auth0 still points at the shared dev tenant; flip
# to `server-run-noauth` + remove the Auth0 vars below if you want
# a fully-offline path.
pwa-dev:
    VITE_SERVER_URL=ws://localhost:7331 \
    VITE_AUTH0_DOMAIN=dev-jrva0wzk3qkdxcar.us.auth0.com \
    VITE_AUTH0_PWA_CLIENT_ID=IPKnV1gX91eYYnX5Uc6142bQpnuA9n3G \
    VITE_AUTH0_API_AUDIENCE=https://auris.api \
    pnpm -F @auris/pwa dev

# Run the PWA dev server + the EvenHub simulator pointed at it.
# Same local-dev env injection as `pwa-dev`.
pwa-sim:
    VITE_SERVER_URL=ws://localhost:7331 \
    VITE_AUTH0_DOMAIN=dev-jrva0wzk3qkdxcar.us.auth0.com \
    VITE_AUTH0_PWA_CLIENT_ID=IPKnV1gX91eYYnX5Uc6142bQpnuA9n3G \
    VITE_AUTH0_API_AUDIENCE=https://auris.api \
    pnpm -F @auris/pwa dev:sim

# Print integrated-stack run instructions (three terminals).
stack:
    @echo "First-time setup:  just db-up        (brings up the Postgres container)"
    @echo ""
    @echo "Open three terminals and run, in order:"
    @echo "  Terminal 1:  just server-run"
    @echo "  Terminal 2:  just pwa-dev"
    @echo "  Terminal 3:  just pwa-sim    (this also starts the simulator)"
    @echo ""
    @echo "Or for terminals 2+3 combined:  just pwa-sim   (it runs vite + simulator concurrently)"
    @echo "Server URL is baked at build time; you only need to sign in via Auth0."

# Build the Mac app (debug).
mac-build:
    cd packages/mac && swift build

# Run the Mac app (build + launch). Menu bar icon appears top-right.
#
# Injects local-dev env vars that `AppSettings` and `Auth0Client`
# read at runtime — same precedence order as the bundled Info.plist
# values, but env wins so this recipe overrides any prior bundle.
# Without them an unbundled `swift run` would fall back to the
# hardcoded dev-tenant defaults (which happen to point at the same
# tenant today, but being explicit makes the recipe self-contained).
mac-run:
    cd packages/mac && \
    AURIS_SERVER_URL=ws://localhost:7331 \
    AUTH0_DOMAIN=dev-jrva0wzk3qkdxcar.us.auth0.com \
    AUTH0_MAC_CLIENT_ID=YDK0XoDAIRhp2uORlfk8TijQkcqRzjsi \
    AUTH0_API_AUDIENCE=https://auris.api \
    swift run

# --- Test ------------------------------------------------------------------

# Run the full test suite (server + PWA).
test:
    cargo test -p auris-server -- --test-threads=1
    pnpm -F @auris/pwa test
    pnpm -F @auris/pwa typecheck

# --- Smoke -----------------------------------------------------------------

# Print manual websocat smoke instructions for poking the server directly.
smoke-instructions:
    @echo "Terminal 1: just server-run"
    @echo "Terminal 2:"
    @echo "  websocat 'ws://localhost:7331/?token=dev'"
    @echo "  Then paste intents like:"
    @echo "  {\"type\":\"start_meeting\"}"
    @echo "  {\"type\":\"set_mode\",\"mode\":\"transcript\"}"
    @echo "  {\"type\":\"stop_meeting\"}"

# --- Contract (proto codegen) ---------------------------------------------

# Regenerate TS + Swift contract types from proto sources.
# Rust regen happens automatically via cargo's build.rs — no separate
# step needed there. Run this after editing any .proto file.
contract-gen:
    cd packages/contract && buf generate

# Lint proto files (style, naming, breaking-change checks).
contract-lint:
    cd packages/contract && buf lint

# Format proto files in place + verify against committed shape.
contract-format:
    cd packages/contract && buf format --write

# Verify TS + Rust + Swift contract builds compile against the
# generated types. Useful as a CI canary that proto edits flow through.
# The Rust crate is standalone (not in the root cargo workspace),
# so we cd into it instead of using `-p`.
contract-check:
    cd packages/contract && buf lint && buf format --diff --exit-code
    cd packages/contract/rust && cargo build
    pnpm -F @auris/contract typecheck
    cd packages/contract/swift && swift build

# --- LLM -------------------------------------------------------------------

# Smoke-test the LLM extraction with a sample description.
llm-smoke description="Q1 budget review for helix product launch":
    cargo run -p auris-server --example llm_smoke -- "{{description}}"

# Smoke-test against Bedrock specifically.
llm-smoke-bedrock description="Q1 budget review for helix product launch":
    AURIS_LLM_PROVIDER=bedrock cargo run -p auris-server --example llm_smoke -- "{{description}}"

# Smoke-test against OpenAI specifically.
llm-smoke-openai description="Q1 budget review for helix product launch":
    AURIS_LLM_PROVIDER=openai cargo run -p auris-server --example llm_smoke -- "{{description}}"

# Smoke-test against Anthropic-direct specifically.
llm-smoke-anthropic description="Q1 budget review for helix product launch":
    AURIS_LLM_PROVIDER=anthropic cargo run -p auris-server --example llm_smoke -- "{{description}}"

# Run the env-gated LLM integration test (provider selected via AURIS_LLM_PROVIDER, defaults to bedrock).
llm-integration:
    RUN_LLM_INTEGRATION=1 cargo test -p auris-server --test llm_integration -- --nocapture

# --- Live pipeline ---------------------------------------------------------

# Run the server with mock STT + LLM disabled — full pipeline shape, no
# external services. Mock STT emits canned chunks every 2s; transcript
# items appear; highlights/actions stay empty (LLM disabled).
live-smoke:
    AURIS_STT_MOCK=1 \
    AURIS_STT_MOCK_INTERVAL_MS=2000 \
    AURIS_LLM_DISABLED=1 \
    AURIS_AUDIO_DISABLED=1 \
    AURIS_TOKEN=dev \
    cargo run -p auris-server -- --port 7331

# Sanity-check SCKit audio capture + format conversion. Captures 5s of
# microphone audio and writes /tmp/spike-audio.wav. Listen with
# `afplay /tmp/spike-audio.wav` to verify the audio path is healthy.
audio-spike:
    cargo run -p auris-server --example screencapturekit_spike
