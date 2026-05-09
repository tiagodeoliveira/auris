# syntax=docker/dockerfile:1.7
#
# Meeting Companion server image. Multi-stage:
#   1. `builder` — Rust toolchain on Debian bookworm, compiles a release
#      binary. Caches the dependency build separately from app source so
#      iterating on .rs files doesn't re-pull 200 crates.
#   2. `runtime` — Debian bookworm-slim, just the binary + the system
#      libs it links against (libssl for tokio-tungstenite's native-tls
#      backend, ca-certificates for HTTPS-out to Soniox / OpenAI / mnemo).
#
# The migrations directory is *not* copied into the runtime image —
# `sqlx::migrate!` embeds the SQL into the binary at compile time. The
# only on-disk state at runtime is `<DATA_DIR>/server.db` (+ blobs), and
# `<DATA_DIR>` is meant to be a mounted volume so it survives restarts.

# ---------- Stage 1: builder ----------
FROM rust:1-bookworm AS builder
WORKDIR /work

# Build-time system deps. `libssl-dev`+`pkg-config` for tokio-tungstenite
# (native-tls). Cleaning the apt lists keeps the layer small (this layer
# is thrown away anyway, but smaller layers = faster builds).
RUN apt-get update \
 && apt-get install -y --no-install-recommends libssl-dev pkg-config \
 && rm -rf /var/lib/apt/lists/*

# Dependency layer: copy *only* the manifests + an empty crate, build
# deps once, cache the result. Subsequent builds that change just .rs
# files reuse this cached layer.
COPY Cargo.toml Cargo.lock ./
COPY packages/server/Cargo.toml packages/server/Cargo.toml
RUN mkdir -p packages/server/src \
 && echo "fn main() {}" > packages/server/src/main.rs \
 && echo "" > packages/server/src/lib.rs \
 && cargo build --release -p meeting-companion-server

# Real source. The `migrations/` dir must be present before this build
# because `sqlx::migrate!("./migrations")` reads + embeds it at compile
# time.
COPY packages/server/migrations packages/server/migrations
COPY packages/server/src packages/server/src
# Touch so cargo definitely notices the source change vs. the stubs.
RUN touch packages/server/src/main.rs packages/server/src/lib.rs \
 && cargo build --release -p meeting-companion-server \
 && strip target/release/meeting-companion-server

# ---------- Stage 2: runtime ----------
FROM debian:bookworm-slim AS runtime
WORKDIR /app

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        libssl3 ca-certificates tzdata \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /work/target/release/meeting-companion-server /usr/local/bin/

# ──────────────────────────────────────────────────────────────────────────
# Environment surface. The full list of env vars the server reads, with
# defaults baked in for the ones that have one. Values without a default
# are listed as commented placeholders so `docker run` operators can
# discover what's available without grepping the source.
#
# Override at runtime with `-e VAR=value` or via docker-compose's
# `environment:` block. See `.env.example` for fuller commentary on each.
# ──────────────────────────────────────────────────────────────────────────

# ─── Required (server fails to start without these unless AUTH_DISABLED=1) ─
# DATABASE_URL=postgres://user:pass@host:5432/db
# AUTH0_DOMAIN=your-tenant.us.auth0.com
# AUTH0_API_AUDIENCE=https://meeting-companion.api

# ─── Server defaults ──────────────────────────────────────────────────────
# `/data` intended as a mounted volume so Postgres metadata + transcript
# JSONL + moment screenshots + artifact blobs survive container restarts.
ENV MEETING_COMPANION_DATA_DIR=/data
ENV RUST_LOG=info
ENV MEETING_COMPANION_HEARTBEAT_MS=10000

# ─── Toggles (set to 1 to enable; unset = off) ───────────────────────────
# ENV MEETING_COMPANION_AUTH_DISABLED=1
# ENV MEETING_COMPANION_LLM_DISABLED=1
# ENV MEETING_COMPANION_AUDIO_DISABLED=1
# ENV MEETING_COMPANION_SKIP_BOOT_RECOVERY=1
# ENV AGENT_LOG_PROMPT=1

# ─── LLM ──────────────────────────────────────────────────────────────────
ENV MEETING_COMPANION_LLM_PROVIDER=bedrock
ENV MEETING_COMPANION_LLM_REGION=us-west-2
# Provider-specific defaults are coded in src/llm.rs and apply when this
# is unset:
#   bedrock  → us.anthropic.claude-sonnet-4-7-20251015-v1:0
#   openai   → gpt-4o
#   anthropic→ claude-opus-4-7
# ENV MEETING_COMPANION_LLM_MODEL_ID=
# ENV OPENAI_API_KEY=
# ENV ANTHROPIC_API_KEY=
# ENV AWS_REGION=us-west-2
# ENV AWS_ACCESS_KEY_ID=
# ENV AWS_SECRET_ACCESS_KEY=

# ─── STT (speech-to-text) ────────────────────────────────────────────────
ENV MEETING_COMPANION_STT_PROVIDER=soniox
ENV MEETING_COMPANION_SONIOX_MODEL=stt-rt-preview
ENV MEETING_COMPANION_STT_MOCK_INTERVAL_MS=3000
# ENV SONIOX_API_KEY=
# Legacy switch — `STT_PROVIDER=mock` is the canonical path.
# ENV MEETING_COMPANION_STT_MOCK=1

# ─── Agent loop (per ADR-0011) ────────────────────────────────────────────
ENV AGENT_TRIGGER_TOKENS=200
ENV AGENT_TRIGGER_SENTENCES=4
ENV AGENT_TRIGGER_SILENCE_MS=4000
ENV AGENT_TRIGGER_MAX_MS=30000

# ─── Summary mode ────────────────────────────────────────────────────────
ENV SUMMARY_TRIGGER_TOKENS=500
ENV SUMMARY_BOOTSTRAP_TOKENS=100
ENV SUMMARY_TRIGGER_MAX_MS=300000

# ─── Moment summarizer (vision LLM context window) ───────────────────────
ENV MEETING_COMPANION_MOMENT_WINDOW_MS=60000
ENV MEETING_COMPANION_MOMENT_GRACE_MS=5000

# ─── mnemo memory layer (optional integration) ───────────────────────────
# Unset = integration disabled. Both URL and API_KEY required to enable.
# ENV MEETING_COMPANION_MNEMO_URL=
# ENV MEETING_COMPANION_MNEMO_API_KEY=
# ENV MEETING_COMPANION_MNEMO_WORKSTATION=

RUN mkdir -p /data
VOLUME ["/data"]

# 7331 serves both WebSocket (PWA + Mac + mobile control plane + /audio
# + /stt) and REST (/meetings, /artifacts, /moments) via axum on a
# single port.
EXPOSE 7331

# `--port 7331` matches the existing `just server-run` recipe and the
# Mac/mobile clients' baked-in defaults. Override at run time with
# extra args, e.g. `docker run ... ghcr.io/.../server --port 8080`.
ENTRYPOINT ["meeting-companion-server"]
CMD ["--port", "7331"]
