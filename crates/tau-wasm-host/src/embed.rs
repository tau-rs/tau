//! Curated Variant A embedding surface (EPIC 7.2).
//!
//! A product embeds tau by (1) building a component (`tau build --target
//! wasm`), (2) implementing [`EmbedPorts`] — live inference, wall clock,
//! entropy, and a live [`on_event`](EmbedPorts::on_event) sink — and
//! (3) calling [`run_component_with_ports`]. Capabilities granted to the
//! component ([`Capability`]) are enforced at the wasm boundary: fs/net the
//! caps don't grant is physically unreachable from the workflow.
//!
//! This module re-exports [`Capability`] so the type is nameable, but not
//! a way to construct a non-empty one: `Capability`'s variants are
//! per-variant `#[non_exhaustive]`, and the builder helpers live behind
//! tau-domain's test-only `test-fixtures` feature. Today the supported way
//! to obtain a real `caps` slice is deserializing it from a package
//! manifest via `tau-pkg` (the same manifest `tau build` reads). The
//! shipped reference host (`crates/tau-wasm-embed-example`) grants none —
//! it calls [`run_component_with_ports`] with `&[]`, so the workflow gets no
//! fs/net regardless of what it asks for.
//!
//! Everything here is a re-export: this module pins *which* items form the
//! supported embedding API, mirroring `tau_runtime_core::embed` (Variant B,
//! EPIC 7.1). See `docs/how-to/embed-wasm-component.md` for the worked
//! example (`crates/tau-wasm-embed-example`). The deterministic conformance
//! entrypoints (`run_component`, `run_component_with_caps`) stay at the
//! crate root — they are not part of the embedding surface.

pub use crate::{run_component_with_ports, EmbedPorts, WasmHostError};
pub use tau_domain::Capability;
pub use tau_ports::llm::{CompletionRequest, CompletionResponse, StopReason, TokenUsage, ToolUse};
