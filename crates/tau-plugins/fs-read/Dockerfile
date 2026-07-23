# syntax=docker/dockerfile:1.6

# ---------- Builder stage: compile fs-read-plugin ----------
FROM rust:1-bookworm AS builder

WORKDIR /workspace

COPY rust-toolchain.toml ./
COPY Cargo.toml Cargo.lock ./
COPY vendor ./vendor
COPY crates ./crates
COPY xtask ./xtask

RUN cargo build --release -p fs-read --bin fs-read-plugin

# ---------- Runtime stage ----------
FROM tau-plugin-base:dev

COPY --from=builder /workspace/target/release/fs-read-plugin /usr/local/bin/fs-read-plugin

USER tau

ENTRYPOINT ["/usr/local/bin/fs-read-plugin"]
