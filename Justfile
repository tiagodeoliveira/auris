default:
    @just --list

# --- Run -------------------------------------------------------------------

# Run the server with a development token (port 7331).
server-run:
    MEETING_COMPANION_TOKEN=dev cargo run -p meeting-companion-server -- --port 7331

# Run the PWA dev server (port 5173).
pwa-dev:
    pnpm -F @meeting-companion/pwa dev

# Run the PWA dev server + the EvenHub simulator pointed at it.
pwa-sim:
    pnpm -F @meeting-companion/pwa dev:sim

# Print integrated-stack run instructions (three terminals).
stack:
    @echo "Open three terminals and run, in order:"
    @echo "  Terminal 1:  just server-run"
    @echo "  Terminal 2:  just pwa-dev"
    @echo "  Terminal 3:  just pwa-sim    (this also starts the simulator)"
    @echo ""
    @echo "Or for terminals 2+3 combined:  just pwa-sim   (it runs vite + simulator concurrently)"
    @echo "PWA settings modal opens on first run; enter ws://localhost:7331 + token 'dev' + your Soniox key."

# Build the Mac app (debug).
mac-build:
    cd packages/mac && swift build

# Run the Mac app (build + launch). Menu bar icon appears top-right.
mac-run:
    cd packages/mac && swift run

# --- Test ------------------------------------------------------------------

# Run the full test suite (server + PWA).
test:
    cargo test -p meeting-companion-server -- --test-threads=1
    pnpm -F @meeting-companion/pwa test
    pnpm -F @meeting-companion/pwa typecheck

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

# --- LLM -------------------------------------------------------------------

# Smoke-test the LLM extraction with a sample description.
llm-smoke description="Q1 budget review for helix product launch":
    cargo run -p meeting-companion-server --example llm_smoke -- "{{description}}"

# Smoke-test against Bedrock specifically.
llm-smoke-bedrock description="Q1 budget review for helix product launch":
    MEETING_COMPANION_LLM_PROVIDER=bedrock cargo run -p meeting-companion-server --example llm_smoke -- "{{description}}"

# Smoke-test against OpenAI specifically.
llm-smoke-openai description="Q1 budget review for helix product launch":
    MEETING_COMPANION_LLM_PROVIDER=openai cargo run -p meeting-companion-server --example llm_smoke -- "{{description}}"

# Smoke-test against Anthropic-direct specifically.
llm-smoke-anthropic description="Q1 budget review for helix product launch":
    MEETING_COMPANION_LLM_PROVIDER=anthropic cargo run -p meeting-companion-server --example llm_smoke -- "{{description}}"

# Run the env-gated LLM integration test (provider selected via MEETING_COMPANION_LLM_PROVIDER, defaults to bedrock).
llm-integration:
    RUN_LLM_INTEGRATION=1 cargo test -p meeting-companion-server --test llm_integration -- --nocapture

# --- Live pipeline ---------------------------------------------------------

# Run the server with mock STT + LLM disabled — full pipeline shape, no
# external services. Mock STT emits canned chunks every 2s; transcript
# items appear; highlights/actions stay empty (LLM disabled).
live-smoke:
    MEETING_COMPANION_STT_MOCK=1 \
    MEETING_COMPANION_STT_MOCK_INTERVAL_MS=2000 \
    MEETING_COMPANION_LLM_DISABLED=1 \
    MEETING_COMPANION_AUDIO_DISABLED=1 \
    MEETING_COMPANION_TOKEN=dev \
    cargo run -p meeting-companion-server -- --port 7331

# Sanity-check SCKit audio capture + format conversion. Captures 5s of
# microphone audio and writes /tmp/spike-audio.wav. Listen with
# `afplay /tmp/spike-audio.wav` to verify the audio path is healthy.
audio-spike:
    cargo run -p meeting-companion-server --example screencapturekit_spike
