# syntax=docker/dockerfile:1.6

# ---------- Builder stage: compile shell-plugin ----------
FROM rust:1-bookworm AS builder

WORKDIR /workspace

COPY rust-toolchain.toml ./
COPY Cargo.toml Cargo.lock ./
COPY vendor ./vendor
COPY crates ./crates
COPY xtask ./xtask

RUN cargo build --release -p shell --bin shell-plugin

# ---------- Runtime stage ----------
FROM tau-plugin-base:dev

COPY --from=builder /workspace/target/release/shell-plugin /usr/local/bin/shell-plugin

USER tau

ENTRYPOINT ["/usr/local/bin/shell-plugin"]
