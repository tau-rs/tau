# syntax=docker/dockerfile:1.6

# ---------- Builder stage: compile ollama-plugin ----------
FROM rust:1-bookworm AS builder

WORKDIR /workspace

COPY rust-toolchain.toml ./
COPY Cargo.toml Cargo.lock ./
COPY vendor ./vendor
COPY crates ./crates
COPY xtask ./xtask

RUN cargo build --release -p ollama --bin ollama-plugin

# ---------- Runtime stage ----------
FROM tau-plugin-base:dev

COPY --from=builder /workspace/target/release/ollama-plugin /usr/local/bin/ollama-plugin

USER tau

ENTRYPOINT ["/usr/local/bin/ollama-plugin"]
