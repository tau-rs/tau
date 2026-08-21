# EPIC 5.1 — `tau build --target wasm-guest | rust-lib`

**Status:** approved (design) · **Date:** 2026-08-21 · **Roadmap:** EPIC 5, story 5.1
**DoD:** both artifacts build from one workflow. Unblocks 5.2 (`tau embed`) and 7.1 (no_std Variant B embedding).

## Goal

From one project (`tau.toml`/`.ts`), let `tau build` emit either of two embedding
artifacts in addition to the default `.tau` bundle:

- `--target wasm-guest` → the fully-linked wasm guest component (`.wasm` + `.wit`), via
  the existing β.7.5 AOT pipeline.
- `--target rust-lib` → a generated **no_std Rust library crate** (Variant B embedding
  surface): the canonical IR baked as linkable Rust source, re-exporting the runtime-core
  entrypoint for a product to link + drive with its own port impls.

Out of scope: `tau embed` host glue (5.2), compiling the rust-lib into an `.rlib`/`.a`
(7.1 — needs the product's port impls).

## Surface

`--target` gains two artifact-kind keywords, resolved **before** hardware-triple parsing.
The keyword namespace and the ADR-0034 triple namespace do not collide.

```
tau build                          → .tau bundle (host)             [unchanged]
tau build --target x86_64-…-linux  → .tau bundle (cross-triple)     [unchanged]
tau build --target wasm-guest      → <name>.wasm + <name>.wit       [routes to wasm pipeline]
tau build --target rust-lib        → <out>/ generated no_std crate  [NEW]
tau build wasm <proj>              → wasm-guest (existing subcommand, retained)
```

`resolve_target` becomes a resolver returning a `BuildTarget` enum rather than a bare
`TargetTriple`:

```rust
enum BuildTarget {
    Bundle(TargetTriple),   // default / hardware triple → .tau bundle
    WasmGuest,              // --target wasm-guest
    RustLib,                // --target rust-lib
}
```

Resolution: `Some("wasm-guest") => WasmGuest`, `Some("rust-lib") => RustLib`,
`None => Bundle(host())`, `Some(other) => Bundle(parse+Available-check)`. Invalid input
→ exit 2 with a message listing both artifact kinds **and** the Available triples.

`tau build --help` documents the two keywords alongside the triple form.

## Artifacts

### wasm-guest
Reuses `cmd::build_wasm` end-to-end (no new pipeline). `--target wasm-guest` on the
bundle command dispatches to the same `lower_to_wasm_ir → world_from_module →
build_guest_with_ir` path the `tau build wasm` subcommand already uses. Output default:
`<project>/<stem>.wasm` + sibling `.wit`. `-o` overrides the `.wasm` path; the `.wit`
follows via `with_extension("wit")`.

### rust-lib (new)
Emits a **generated source crate** (not a compiled artifact) into an output directory
(default `<project>/<stem>-rust-lib/`, `-o` overrides the dir):

```
<out>/Cargo.toml        # no_std lib crate; dep: tau-runtime-core (default-features=false),
                        #   tau-ir, tau-ports — pinned to the workspace version
      src/lib.rs        # #![no_std]
                        # pub const TAU_IR: &[u8] = &[ …canonical IR bytes… ];
                        # pub const TAU_IR_HASH: &str = "<hex>";
                        # pub use tau_runtime_core::run_ir;   // product drives with its ports
      tau.wit           # cap-derived world (same generate_world as wasm-guest)
      README.md         # how to link + supply ports (points at 7.1)
```

The IR bytes are the same canonical bytes the wasm bake uses (`lower_to_wasm_ir` reused
so cap-fit + feature-fit refuse ProcessExec/AgentSpawn/control-flow identically). Codegen
lives in `tau-sdk-codegen` (new `emit_rust_lib` module, sibling to `embed_js`) so it is
unit-testable without the CLI and reused by future embed work.

## Governance
Both new targets run the **same** governed-by-default gate the wasm/bundle paths already
run (`wasm_governance_gate` reused: ADR-0057, GOV000 on absent `[allow]`). rust-lib and
wasm-guest both lower for `any-wasi-strict`, so cap-fit is identical.

## JSON output parity
`--json` emits a per-kind object, keeping the existing bundle keys and adding `kind`:

```json
// bundle   {"kind":"bundle",     "path":"…/x.tau",  "sha256":"…","size_bytes":N}
// wasm     {"kind":"wasm-guest", "path":"…/x.wasm", "sha256":"…","size_bytes":N,"wit":"…/x.wit"}
// rust-lib {"kind":"rust-lib",   "path":"…/x-rust-lib","tree_sha256":"…","files":M}
```

Human output stays one line per artifact, as today.

## Error handling
- Unknown `--target` keyword/triple → exit 2 (config), message lists both value spaces.
- Cap-fit / feature-fit refusal (wasm-guest, rust-lib) → exit 2, existing diagnostics.
- IO / codegen write failure → exit 70.
- thiserror at the `tau-sdk-codegen` boundary; anyhow in the CLI shim. `forbid(unsafe_code)`.

## Testing
- `tau-sdk-codegen`: unit test `emit_rust_lib` renders expected file set; `src/lib.rs`
  contains `TAU_IR` const with non-empty bytes matching the input; `tau.wit` non-empty.
- `tau-cli`: assert-artifact-produced test per target.
  - rust-lib: run against `examples/fan-monitor`; assert `<out>/Cargo.toml`, `src/lib.rs`,
    `tau.wit` exist and `src/lib.rs` embeds the IR const. (No cargo shell — fast.)
  - wasm-guest: `resolve_target` routes `--target wasm-guest` to the wasm path (unit-level
    routing assertion; the full 60–90s `.wasm` build stays covered by existing
    `build_wasm_e2e`/`build_wasm_world_dod` gated tests — do not duplicate).
- `resolve_target` unit tests: keyword → `WasmGuest`/`RustLib`; triple still → `Bundle`;
  invalid → error naming both spaces. Help-snapshot updated.

## Docs
One example under the build docs page: `tau build --target rust-lib ./my-workflow` and the
resulting crate layout + "link it in your product" pointer to 7.1.

## Files touched
- `crates/tau-cli/src/cli.rs` — `--target` help text (keywords documented).
- `crates/tau-cli/src/cmd/build.rs` — `BuildTarget` resolver, dispatch, JSON parity.
- `crates/tau-cli/src/cmd/build_wasm.rs` — expose the reusable entry for wasm-guest routing.
- `crates/tau-sdk-codegen/src/emit_rust_lib.rs` (new) + `lib.rs` re-export.
- `crates/tau-cli/tests/…` — per-target artifact tests; help snapshot.
- docs build page + `SUMMARY.md` if a new page.

## Conflict note
Lane A touches `tau-pkg` project.rs/lowering/typecheck. This slice stays in the CLI build
path + `tau-sdk-codegen`; no `tau-pkg` producer changes required (wasm + rust-lib both go
through `tau-ir-lower`, not `tau-pkg::bundle::build`). Rebase if a collision appears.
