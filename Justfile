default:
    @just --list

# Run the server with a development token
server-run:
    MEETING_COMPANION_TOKEN=dev cargo run -p meeting-companion-server -- --port 7331

# Run all tests (Rust + TS typecheck). --test-threads=1 because of heartbeat env-var seam.
test:
    cargo test -p meeting-companion-server -- --test-threads=1
    pnpm -F @meeting-companion/contract typecheck

# Print manual smoke instructions
smoke-instructions:
    @echo "Terminal 1: just server-run"
    @echo "Terminal 2:"
    @echo "  websocat 'ws://localhost:7331/?token=dev'"
    @echo "  Then paste intents like:"
    @echo "  {\"type\":\"start_meeting\"}"
    @echo "  {\"type\":\"set_mode\",\"mode\":\"transcript\"}"
    @echo "  {\"type\":\"stop_meeting\"}"
