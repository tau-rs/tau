# ADR-0031: `tau-observe` as the canonical tracing-subscriber init crate

> **Updated 2026-05-31 (β.1.4):** `tau-observe` remains the canonical
> tracing-subscriber init crate for the tokio host shell. The
> executor-agnostic kernel (`tau-runtime-core`, β.1) keeps a
> `#[doc(hidden)]` mirror of the §3.9 vocabulary; a drift test in
> `tau-runtime-tokio` cross-checks both lists.

## Status

Accepted. Implemented in PR <number-to-fill-at-merge-time>.

## Context

`tracing` adoption (ADR-0006 §3.9, NG9) leaves subscriber install to the
caller. In practice every tau binary/library that produces logs has
re-implemented the same `tracing_subscriber::fmt()` + `EnvFilter` dance.
Two near-identical implementations exist today:

- `crates/tau-cli/src/tracing.rs` — human format to stderr, CLI-flag-to-
  filter mapping, panicking `init()`.
- `crates/tau-plugin-sdk/src/tracing_layer.rs` — JSON to stderr,
  idempotent `Once`-gated install.

Sub-projects B (§3.9 span vocabulary), C (preview helpers), D (workflow
+ recording as `Layer`s), E (`tracing-appender`), F (OTLP export) all
need a single place that owns the subscriber registry. Continuing to
add layers from two different crates with two different init policies
would lock in the divergence.

## Decision

Promote the existing `tau-observe` crate (currently a stub) to the
canonical owner of:

- `tau_observe::install::install(InstallOptions) -> Result<InstallGuard, InstallError>`
  with idempotent global init and an `InstallGuard` that future sub-
  projects can hang flush behavior off (sub-project E).
- `tau_observe::filter::env_or_directive(&str) -> EnvFilter` — the only
  place that interprets `RUST_LOG`.
- `tau_observe::vocabulary` — `&'static str` constants for every §3.9
  span and event name.

`tau-cli` and `tau-plugin-sdk` keep their public `install` functions
(signatures unchanged) but their bodies become one-liners that build
the appropriate `InstallOptions` and delegate to `tau_observe::install`.

## Consequences

- One subscriber init code path. Sub-projects B/C/D/E/F each layer onto
  this surface without further divergence.
- `tau-observe` becomes a workspace dependency. Build time impact is
  negligible (the crate has three direct deps; all are already in the
  workspace).
- Plugin authors who previously imported `tau_plugin_sdk::tracing_layer`
  see no source-level change; the layer continues to be re-exported.
- NG9 still holds: `tau-observe` exposes helpers but does not enforce
  any redaction policy on the caller.

## Trigger to revisit

A second subscriber init pattern lands that doesn't fit `InstallOptions`
(e.g. multi-sink, dynamic reconfiguration). At that point reconsider
whether `tau-observe::install` should grow or whether a layered API
(`tau_observe::build_layers() -> impl Layer<S>`) is a better surface.
