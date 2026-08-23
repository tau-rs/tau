# syntax=docker/dockerfile:1.6

# ---------- Builder stage: compile tau-net-bridge ----------
FROM rust:1-bookworm AS builder

WORKDIR /workspace

# Copy the rust-toolchain spec first so rustup pre-installs the right channel.
COPY rust-toolchain.toml ./

# Copy minimum needed to build tau-sandbox-native's tau-net-bridge bin target.
# We pull in workspace metadata + the source crates the bridge depends on.
# vendor/ is required: the workspace [patch.crates-io] points async-stream at
# vendor/async-stream, so cargo cannot resolve the graph without it.
COPY Cargo.toml Cargo.lock ./
COPY vendor ./vendor
COPY crates ./crates
COPY xtask ./xtask

RUN cargo build --release -p tau-sandbox-native --bin tau-net-bridge

# ---------- Runtime stage ----------
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 1000 tau \
    && useradd --system --uid 1000 --gid tau --shell /usr/sbin/nologin tau

COPY --from=builder /workspace/target/release/tau-net-bridge /usr/local/bin/tau-net-bridge

# Default user is tau (non-root); per-plugin Dockerfiles can override.
USER tau
