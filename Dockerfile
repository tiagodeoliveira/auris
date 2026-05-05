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

# Default data dir = `/data`, intended as a mounted volume so SQLite +
# future blobs survive container restarts.
ENV MEETING_COMPANION_DATA_DIR=/data
RUN mkdir -p /data
VOLUME ["/data"]

# 7331 serves both WebSocket (PWA + Mac control plane + /audio) and
# REST (/meetings…) via axum on a single port.
EXPOSE 7331

# `--port 7331` matches the existing `just server-run` recipe and the
# Mac client's default. Override at run time with extra args.
ENTRYPOINT ["meeting-companion-server"]
CMD ["--port", "7331"]
