# Framing C″ — MCU target strategy

**Status:** Scoping document. Not a spec. Must land before any embedded /
firmware target work begins.

**Date:** 2026-05-29.

**Relates to:** [`docs/explanation/tau-philosophy.md`](../../explanation/tau-philosophy.md)
acknowledged risk C″.

---

## Why this needs framing first

The philosophy commits to "wasm-primary, portable to embedded." Industry
reality (May 2026):

- **WAMR on ESP32-class MCUs is production**, but still on WASI Preview 1.
- **WASI 0.2 Component Model on microcontrollers is not delivered** by any
  shipping runtime, and no public roadmap entry confirms it for 2026.

This means "wasm component on MCU" — the cleanest version of the
portability story — **is not deliverable in 2026 without doing
component-model-on-MCU runtime work ourselves**, which is outside tau's
scope.

The honest framing avoids two failure modes:
1. Promising wasm-component MCU support that can't be delivered.
2. Dropping embedded entirely because the cleanest path isn't ready.

The middle path: ship embedded via WAMR/Preview-1 wasm **or** native
firmware, and graduate to component-model-on-MCU when the runtime catches up.

---

## Decisions this framing must reach

### C″-1. The two embedded tiers tau commits to today

| tier | runtime substrate | enforcement | what runs |
|---|---|---|---|
| **wasm-mcu-preview1** | WAMR (Preview 1) on ESP32-class | tau-managed (no WASI capability model on Preview 1) | the harness compiled to `wasm32-wasip1`, native tools as wasm host functions |
| **bare-metal-passthrough** | no wasm; native firmware (Rust → `xtensa-esp32-none-elf` or similar) | tau-managed `passthrough` (no isolation needed; single trust domain) | the harness compiled directly to firmware, native tools statically linked |

Both are valid `tau build` targets and both deliver the portability
philosophy. The component-model-on-MCU tier (`wasm-mcu-component`) is
explicitly **deferred to a future phase**, not on the Phase 1/2 plan.

### C″-2. Target triple naming

The target triple registry already exists (Phase 2 §B shipped 2026-05-19,
PR #190). The two new tiers slot in as:

- `wasm-mcu-preview1-tau-managed` — wasm via WAMR Preview 1
- `bare-metal-xtensa-passthrough` (and siblings: `bare-metal-cortex-m-…`,
  `bare-metal-riscv32-…`)

Both should land as `Reserved` initially (Phase 2 §B's stability discipline)
and graduate to `Available` when their CI lane is green.

### C″-3. The `tau-runtime-core` extraction

Both tiers require the executor-agnostic, `no_std` + `alloc` core extraction
described earlier in design discussion: the agent loop generic over
`LlmBackend` / `Tool` / `Storage`, no tokio, no `std::process`.

Decide before engine work:
- The crate split: `tau-runtime` (current, tokio shell, std) →
  `tau-runtime-core` (`no_std`, executor-agnostic) + `tau-runtime-tokio`
  (the current host shell, unchanged behavior) + future
  `tau-runtime-embassy` (the MCU shell).
- The `tau-ports` feature-gating: `Sandbox::wrap_spawn` and
  `apply_post_spawn` move behind a `std`/`process` feature.
- The DefinitionOfDone for the extraction: every existing test in
  `tau-runtime` stays green; no behavior change observable on the host.

This is the prerequisite refactor that **must precede any new target
work**. It is also a host-side, hardware-free task — verifiable on a dev
machine without procuring boards.

### C″-4. Plugin model on MCU

Per the philosophy: **native tools compiled in, MCP servers contracted at
runtime.** On MCU this maps to:

- **native tools** = compiled-in Rust handlers, registered into the
  trait-object registry via a per-target `static` builder. No process spawn,
  no proxy.
- **MCP contracts** = an outbound MCP client over whatever transport is
  available (HTTP over WiFi/cellular for ESP32; absent on truly offline
  devices). Optional at runtime — if unreachable, contracts gracefully
  unavailable.

The credential chain's `DeviceIdentity` provider (per-device secure-element
key) is the recommended fit for MCU. Shared bearer keys on a fleet are
explicitly out of scope.

### C″-5. The LLM endpoint question

Both MCU tiers delegate inference. The honest framing of the wire:

- **WiFi-class devices (ESP32-S3 + PSRAM)**: HTTPS to a cloud or LAN
  inference endpoint via `reqwless` + `embedded-tls`. RAM budget is tight
  (TLS records ~16 KB × 2; conversation history bound; PSRAM helps a lot).
- **Always-offline devices**: no path to a remote model → the harness
  cannot complete a turn → these are not real targets in Phase 1.
  Acknowledge explicitly.

The bare-metal target does **not** mean "the agent runs without a network."
It means "the orchestration runs locally; inference still routes out."

### C″-6. Capability enforcement on the MCU tiers

WASI Preview 1 has no capability model in the Component-Model sense; the
firmware tier is `passthrough` by definition (single trust domain, no
process boundary). The honest claim:

- **MCU tiers are passthrough.** Capability enforcement at the OS / wasm
  boundary is not available. The capability gate still runs in the harness
  (declared capabilities are recorded and surfaced), but **enforcement is
  advisory**, not OS-level.

`tau check --target` must refuse to build a workflow requiring `strict`
enforcement for these targets. The user must explicitly re-declare the
workflow as `passthrough`-acceptable to proceed. This preserves the
"capability-safe by construction" principle by making the weakening
visible at build time, not silent at runtime.

### C″-7. CI strategy for MCU targets

A real MCU CI lane is expensive and slow. Phase-1 minimum:

- `cargo check --target` on `wasm32-wasip1` and `xtensa-esp32-none-elf` (no
  device needed; catches drift cheaply).
- Manual hardware verification on a single canonical board (ESP32-S3 dev
  kit with PSRAM) before promoting either tier from `Reserved` to
  `Available`.

Hardware-in-the-loop CI is deferred.

---

## What this framing rules out (deliberately)

- **No claim that wasm components run on MCUs in 2026.** The runtime isn't
  there. Don't ship the claim.
- **No tau-built wasm runtime.** WAMR exists; we use it.
- **No on-device inference.** Not in Phase 1, possibly never. Even
  ESP-Claw (Espressif's own MCU agent) calls cloud LLMs.
- **No native target without `tau-runtime-core` extraction.** Skipping the
  core extraction and writing a parallel firmware harness is the works-
  in-dev-breaks-in-prod failure mode the philosophy explicitly forbids.

---

## Deliverable shape

The framing is complete when:

1. The two embedded-tier target triples (`wasm-mcu-preview1-tau-managed`,
   `bare-metal-xtensa-passthrough`) are added to the registry as `Reserved`
   per the existing Phase 2 §B stability discipline (no ADR needed; this is
   registry extension, recorded in the lockfile-of-targets schema).
2. A design spec at `docs/superpowers/specs/<date>-tau-runtime-core-design.md`
   covers C″-3 (the extraction) — same prerequisite work the engine
   itself needs.
3. A short ADR records C″-1 + C″-6: "MCU tiers are passthrough by
   architecture; component-model-on-MCU deferred until WAMR or successor
   ships it."

Until that exists, no embedded-target implementation lands.

---

## Risk acknowledgment

The biggest risk in this framing is **promising MCU support that erodes
trust if real workflows can't actually run there**. The two tiers committed
to here are honest: WAMR Preview 1 works (production-grade runtime), native
firmware works (Rust toolchain is mature). What's softened from the
original vision is the *capability-enforced wasm component on MCU* — that
claim is held in reserve, marked as a research horizon, not promised as
deliverable. This is the right trade.
