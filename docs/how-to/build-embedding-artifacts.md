# Build embedding artifacts

`tau build` compiles one workflow into whichever deployment artifact you need.
The `--target` flag selects what it emits: a hardware bundle (the default), the
fully-linked wasm component, or a no_std Rust embedding crate.

```bash
tau build                      # .tau bundle for the host (default)
tau build --target wasm-guest  # fully-linked wasm component (<name>.wasm + .wit)
tau build --target rust-lib    # generated no_std Rust crate (Variant B embedding)
```

`--target` accepts either an **artifact kind** (`wasm-guest`, `rust-lib`) or an
Available **hardware triple** (see [ADR-0034](../decisions/0034-target-triple-registry.md)).
Both artifact kinds run the same governed-by-default gate and the same
capability-fit check the wasm path uses — a workflow that needs `process-exec`
or `agent-spawn`, or that has un-flattened control-flow steps, is refused.

## `--target rust-lib`

`tau build --target rust-lib ./my-workflow` writes a generated crate to
`./my-workflow/my-workflow-rust-lib/`:

```text
Cargo.toml   no_std lib; depends on tau-runtime-core
src/lib.rs   pub const TAU_IR + `pub use tau_runtime_core::run_ir`
tau.wit      capability-derived world
README.md    how to link + supply ports
```

The crate is **source**, not a compiled library: your product links it,
implements the ports for the capabilities listed in `tau.wit`, and drives the
baked workflow with `run_ir(TAU_IR, …)`. Use `-o <dir>` to choose the output
directory.

This is the Variant B embedding surface — see EPIC 7.1 for the full embedding
API and a worked example.

## JSON output

`--json` prints one object per artifact, carrying a `kind` discriminator:

```json
{ "kind": "rust-lib", "path": "…/my-workflow-rust-lib", "ir_hash": "…", "files": 4 }
```
