# Browser capabilities profile

This page records which tau capabilities work when a compiled workflow runs
in a **browser host** (a `wasm32-wasip2` component loaded via `jco`, as the
`@tau/embed-js` / `@tau/react` / `@tau/angular` SDKs do), and the measured
size of the wasm bundle a browser downloads.

The profile is a projection of the capability model onto the browser
substrate. It changes no enforcement: the source of truth is the per-capability
`Disposition` in `tau-ports` (`crates/tau-ports/src/target/wasi_map.rs`). See
also [Capabilities and consent](../explanation/capabilities-and-consent.md),
[Target triple reference](target-triples.md), and the
[WIT host world](wit-host-world.md).

## Capability profile

Each capability maps to a `Disposition` (how tau dispatches it) and, from that,
a **browser status**:

- **Available** — works in the browser with no host-side loss.
- **Emulated** — works only through a browser shim whose semantics differ from
  the native / wasmtime host (the divergence is documented below).
- **Forbidden** — cannot be expressed in a browser; a build-time error on the
  wasm target.

| Capability | `Disposition` | Browser status | Notes |
|---|---|---|---|
| `task_list` | `InGuest` | Available | Enforced inside the guest; no host surface. |
| `plan` | `InGuest` | Available | As above. |
| `agent.spawn` | `InGuest` | Available | Sub-agent recursion runs entirely in-guest. |
| `skill.spawn` | `InGuest` | Available | As above. |
| `net.http` | `Wasi` | Emulated | `wasi:http` → browser `fetch`: CORS applies, no raw sockets, preflight for non-simple requests. Live calls need a JSPI build (`jco --async-mode jspi`); the shipped 5.4 demo uses a synchronous cassette instead. |
| `fs.read` | `Wasi` | Emulated | `wasi:filesystem` → OPFS / in-memory; there is no host filesystem, and preopen scoping differs from the wasmtime host. |
| `fs.write` | `Wasi` | Emulated | As `fs.read`; writes land in OPFS / memory, not on a host disk. |
| `fs.exec` | `Unsupported` | Forbidden | No exec surface on wasm (already a build-time error). |
| `process.spawn` | `Unsupported` | Forbidden | A browser cannot spawn OS processes. |
| `custom.*` | `HostMediated` | Forbidden (default) | Needs a host mediator outside the WASI ABI; no browser default exists. |
| clock, random | — (ports) | Available | Not capabilities; resolved to `Date.now()` / `crypto.getRandomValues`. |

`Available` and `Forbidden` are the same decisions the wasmtime host enforces.
The `Emulated` rows are where a browser's `jco` WASI-0.2 implementation
constrains a capability that is otherwise grantable — the capability compiles
into the guest, but the browser substrate limits its behaviour.

## Bundle size

The shipped browser artifact is a **WASI-0.2 component**. The number below is
the **minimal guest floor**: `tau-wasm-guest` built for `wasm32-wasip2` in
release against the committed empty-cap baseline world with the built-in
default IR — deterministic with no environment variables (the same build CI's
link-gate runs). A real workflow scales above this floor with its baked IR and
declared tools (the 5.4 `streaming-demo` fixture is ~1.6 MB).

**This floor measures the component shell, not a workflow.** That build sets no
`TAU_IR_BYTES`, so `BAKED_IR` is empty, the guest's empty-IR early return
const-folds, and LTO drops everything behind it — the pipeline interpreter, the
agent loop and the goal-predicate registry (with its regex engine) are all dead
code in this number. It is a useful regression tripwire for the shell and the
capability world; it bounds nothing about a component that carries real IR. The
in-guest control-flow work (ADR-0068) added ~530 KiB to components that do
(measured there), and this gate did not move.

| Measurement | Bytes | KiB | Tool |
|---|--:|--:|---|
| Shipped component (browser download) | 15,686 | 15.3 | `wasm-tools` |
| Core module after `wasm-metadce` | 10,651 | 10.4 | Binaryen (v132) |
| + `wasm-opt -Oz` | 9,878 | 9.6 | Binaryen (v132) |

The **published headline number is the shipped component: 15.3 KiB** — the real
download. The `wasm-metadce` floor is the tree-shaking demonstration: rooting
the component's real exports and garbage-collecting the rest removes the custom
`name` / `producers` sections and already-dead code the browser loader never
needs, showing a ~32 % code floor below the shipped bytes. The metadce / `-Oz`
figures are Binaryen-version-dependent (pinned to v132 in CI); the shipped-size
figure is toolchain-independent and is the CI-gated number.

### Why two tools

Binaryen (`wasm-metadce`, `wasm-opt`) [cannot yet parse a
component](https://github.com/WebAssembly/binaryen/issues/6728), so it cannot
measure the shipped artifact directly. The pipeline therefore measures the
component with `wasm-tools` (which understands components) and extracts the
embedded core module with `wasm-tools component unbundle` before running
`wasm-metadce` on it.

## Reproducing

```bash
# Requires: rustup target wasm32-wasip2, wasm-tools; Binaryen optional
# (for the tree-shaken floor). Supply CARGO_TARGET_DIR per the CARGO RULES.
env CARGO_TARGET_DIR=target/main scripts/wasm-guest-size.sh
just wasm-guest-size            # same, via the workspace task runner
```

CI runs this in the `runtime-core-no-std` job on every PR: it appends the table
above to the run summary and fails if the shipped component exceeds
`TAU_WASM_SIZE_BUDGET` (a regression tripwire, default 32 KiB — ~2× the floor).

## Stability

This page is descriptive, not an enforcement contract: no target triple
currently gates a workflow against the browser profile. A first-class
`any-browser-strict` triple that turns the `Emulated` / `Forbidden` rows into a
`tau check --target` gate is future work (it would need its own ADR and a
`tau-ports` registry entry). Until then, treat the `Forbidden` rows as the hard
guarantees (they are already `Disposition::Unsupported` build errors) and the
`Emulated` rows as documented browser caveats.
