# syntax=docker/dockerfile:1.6

# ---------- Builder stage: compile the tau CLI ----------
FROM rust:1-bookworm AS builder

WORKDIR /workspace

COPY rust-toolchain.toml ./
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY xtask ./xtask
COPY vendor ./vendor

RUN cargo build --release -p tau-cli --bin tau

# ---------- Runtime stage ----------
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 1000 tau \
    && useradd --system --uid 1000 --gid tau --create-home --shell /bin/bash tau

COPY --from=builder /workspace/target/release/tau /usr/local/bin/tau

WORKDIR /home/tau
USER tau

ENTRYPOINT ["/usr/local/bin/tau"]
