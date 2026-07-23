# syntax=docker/dockerfile:1.6

# ---------- Builder stage: compile anthropic-plugin ----------
FROM rust:1-bookworm AS builder

WORKDIR /workspace

COPY rust-toolchain.toml ./
COPY Cargo.toml Cargo.lock ./
COPY vendor ./vendor
COPY crates ./crates
COPY xtask ./xtask

RUN cargo build --release -p anthropic --bin anthropic-plugin

# ---------- Runtime stage ----------
FROM tau-plugin-base:dev

COPY --from=builder /workspace/target/release/anthropic-plugin /usr/local/bin/anthropic-plugin

USER tau

ENTRYPOINT ["/usr/local/bin/anthropic-plugin"]
