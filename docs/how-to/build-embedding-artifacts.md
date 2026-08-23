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

## Host glue with `tau embed`

`tau build` produces the artifact; `tau embed` produces the **host** that drives
it. `tau embed --host <js|rust|c>` emits host-side glue and, for `rust`/`c`,
derives the capability-WIT world from `<project>` (default: the current
directory).

```bash
tau embed --host js               # @tau/embed-js scaffold (project-independent)
tau embed --host rust ./my-flow   # native host crate → drives the rust-lib artifact
tau embed --host c    ./my-flow   # wasmtime C-API host → drives the wasm-guest component
```

The three hosts target different artifacts:

| `--host` | drives | how it embeds |
|---|---|---|
| `rust` | `rust-lib` (Variant B crate) | links the crate natively, implements the ports, calls `run_ir` |
| `c` | `wasm-guest` component | loads the `.wasm` and drives its `run` export via the [wasmtime C API](https://docs.wasmtime.dev/c-api) |
| `js` | `wasm-guest` component | jco-transpiled (see `@tau/embed-js`) |

`--host rust` writes an `embed-rust/` tree:

```text
Cargo.toml    host crate; path-deps the sibling rust-lib crate + tau-runtime-core
src/main.rs   a StubDispatcher (todo!() port bodies) that drives run_ir(TAU_IR, …)
tau.wit       capability-derived world (reference)
README.md     how to fill in the ports and run
```

`--host c` writes an `embed-c/` tree:

```text
tau_embed.h   the four tau:host/host imports + tau_embed_run() signature
tau_embed.c   wasmtime-C-API host stub with TODO(EPIC 7.1) port bodies
tau.wit       capability-derived world (reference)
README.md     how to link libwasmtime and fill in the ports
```

Both `rust` and `c` scaffolds carry `todo!()` / `TODO(EPIC 7.1)` port stubs: the
generated host is compile-ready glue, and supplying the real port bodies (tool
execution + inference) is the product's job — see EPIC 7.1 for the worked
embedding example. `-o <dir>` chooses the output directory.

## `tau embed` JSON output

`--json` prints one object with a `kind` discriminator, mirroring `tau build`:

```json
{ "kind": "embed-rust", "path": ".", "files": 4, "ir_hash": "…" }
```

`ir_hash` is present for `--host rust|c` (they derive IR from the project) and
omitted for `--host js` (project-independent).