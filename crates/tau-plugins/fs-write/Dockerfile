# syntax=docker/dockerfile:1.6

# ---------- Builder stage: compile fs-write-plugin ----------
FROM rust:1-bookworm AS builder

WORKDIR /workspace

COPY rust-toolchain.toml ./
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY xtask ./xtask

RUN cargo build --release -p fs-write --bin fs-write-plugin

# ---------- Runtime stage ----------
FROM tau-plugin-base:dev

COPY --from=builder /workspace/target/release/fs-write-plugin /usr/local/bin/fs-write-plugin

USER tau

ENTRYPOINT ["/usr/local/bin/fs-write-plugin"]
