# syntax=docker/dockerfile:1.7
#
# Auris server image. Multi-stage:
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

# Git short SHA, passed in by CI via `--build-arg AURIS_BUILD_SHA=...`.
# Read at `cargo build` time via `option_env!` in `main.rs`'s banner;
# local `docker build` without the arg falls back to "dev". CalVer in
# `Cargo.toml` is the human-readable version; this pins the exact
# commit when the version string is stale.
ARG AURIS_BUILD_SHA=dev
ENV AURIS_BUILD_SHA=$AURIS_BUILD_SHA

# Build-time system deps. `libssl-dev`+`pkg-config` for tokio-tungstenite
# (native-tls).
RUN apt-get update \
 && apt-get install -y --no-install-recommends libssl-dev pkg-config \
 && rm -rf /var/lib/apt/lists/*

# Copy the whole crate in one shot. No source-file stubbing needed —
# BuildKit cache mounts below persist cargo's registry + target dir
# across builds, so changing one .rs file doesn't recompile every
# dependency. The `migrations/` dir must be present before this build
# because `sqlx::migrate!("./migrations")` reads + embeds it at
# compile time.
COPY Cargo.toml Cargo.lock ./
COPY packages/server/Cargo.toml packages/server/Cargo.toml
COPY packages/server/migrations packages/server/migrations
COPY packages/server/src packages/server/src

# Single build step. Cache mounts:
#   - /usr/local/cargo/registry — the downloaded crate index + sources
#   - /work/target              — cargo's incremental build artifacts
# Neither lives in the image, so we copy the binary out of the
# target/ cache mount before the layer is committed; otherwise the
# runtime stage's COPY --from=builder would find an empty path.
# `--bin auris-server` skips compiling the recover-meeting helper
# (not shipped in the runtime image; available via `cargo run` in dev).
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/work/target \
    cargo build --release -p auris-server --bin auris-server \
 && cp target/release/auris-server /tmp/auris-server \
 && strip /tmp/auris-server

# ---------- Stage 2: runtime ----------
FROM debian:bookworm-slim AS runtime
WORKDIR /app

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        libssl3 ca-certificates tzdata \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /tmp/auris-server /usr/local/bin/

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
# AUTH0_API_AUDIENCE=https://auris.api

# ─── Server defaults ──────────────────────────────────────────────────────
# `/data` intended as a mounted volume so Postgres metadata + transcript
# JSONL + moment screenshots + artifact blobs survive container restarts.
ENV AURIS_DATA_DIR=/data
ENV RUST_LOG=info
ENV AURIS_HEARTBEAT_MS=10000

# ─── Toggles (set to 1 to enable; unset = off) ───────────────────────────
# ENV AURIS_AUTH_DISABLED=1
# ENV AURIS_LLM_DISABLED=1
# ENV AURIS_AUDIO_DISABLED=1
# ENV AURIS_SKIP_BOOT_RECOVERY=1
# ENV AGENT_LOG_PROMPT=1

# ─── LLM ──────────────────────────────────────────────────────────────────
ENV AURIS_LLM_PROVIDER=bedrock
ENV AURIS_LLM_REGION=us-west-2
# Provider-specific defaults are coded in src/llm.rs and apply when this
# is unset:
#   bedrock  → us.anthropic.claude-sonnet-4-7-20251015-v1:0
#   openai   → gpt-4o
#   anthropic→ claude-opus-4-7
# ENV AURIS_LLM_MODEL_ID=
# ENV OPENAI_API_KEY=
# ENV ANTHROPIC_API_KEY=
# ENV AWS_REGION=us-west-2
# ENV AWS_ACCESS_KEY_ID=
# ENV AWS_SECRET_ACCESS_KEY=

# ─── STT (speech-to-text) ────────────────────────────────────────────────
ENV AURIS_STT_PROVIDER=soniox
ENV AURIS_SONIOX_MODEL=stt-rt-v5
ENV AURIS_STT_MOCK_INTERVAL_MS=3000
# ENV SONIOX_API_KEY=
# Legacy switch — `STT_PROVIDER=mock` is the canonical path.
# ENV AURIS_STT_MOCK=1

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
ENV AURIS_MOMENT_WINDOW_MS=60000
ENV AURIS_MOMENT_GRACE_MS=5000

# ─── mnemo memory layer (optional integration) ───────────────────────────
# Unset = integration disabled. Both URL and API_KEY required to enable.
# ENV AURIS_MNEMO_URL=
# ENV AURIS_MNEMO_API_KEY=
# ENV AURIS_MNEMO_WORKSTATION=

RUN mkdir -p /data
VOLUME ["/data"]

# 7331 serves both WebSocket (PWA + Mac + mobile control plane + /audio
# + /stt) and REST (/meetings, /artifacts, /moments) via axum on a
# single port.
EXPOSE 7331

# The binary is its own healthcheck probe via the `healthz` subcommand
# (see packages/server/src/main.rs). The runtime is debian-slim today
# — it has wget — but keeping the probe inside the binary makes the
# image self-describing: downstream compose can rely on
# `condition: service_healthy` without compose-side test wiring, and
# the contract survives a future shrink to distroless. Matches the
# pattern mnemo adopted in mnemo@647fb82.
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=5 \
    CMD ["auris-server", "healthz"]

# `--port 7331` matches the existing `just server-run` recipe and the
# Mac/mobile clients' baked-in defaults. Override at run time with
# extra args, e.g. `docker run ... ghcr.io/.../server --port 8080`.
ENTRYPOINT ["auris-server"]
CMD ["--port", "7331"]
