# EPIC 5.2 — `tau embed --host c | rust | js` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend `tau embed` so `--host rust` and `--host c` emit host-side embedding scaffolds (glue that drives the artifacts EPIC 5.1 produces), keeping the shipped `--host js` path working, routed through one `EmbedArgs.host` match.

**Architecture:** Two new pure string-template renderers in `tau-sdk-codegen` — `render_embed_rust` (a native host crate that links the 5.1 rust-lib artifact and calls `run_ir`) and `render_embed_c` (a wasmtime-C-API host stub that loads the 5.1 wasm-guest component). Both embed the cap-derived WIT world verbatim; neither shells any external tool at emit time (consistent with `emit_rust_lib`/`embed_js`/`emit_ts`/`emit_python`). The CLI shim (`cmd/embed.rs`) derives IR bytes + hash + WIT from the project (reusing `build_wasm::{lower_to_wasm_ir, world_from_module}`, exactly as `build.rs::emit_rust_lib_to` does) for the `rust`/`c` hosts, and needs no project for `js`.

**Tech Stack:** Rust (workspace crates `tau-cli`, `tau-sdk-codegen`), clap, thiserror (codegen boundary) / anyhow (CLI shim), cargo nextest. No external codegen toolchain.

**Spec:** This plan is self-contained (task derived from `docs/superpowers/plans/vision-roadmap.md` EPIC 5 story 5.2 + the in-chat Option-A design). No separate spec file.

## Global Constraints

- **Cargo discipline (CLAUDE.md):** every cargo command is `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e52 cargo <cmd> -p <crate>`. Test timeout 300s, build/check 180s, clippy 240s, fmt 30s. Never bare cargo, never `--workspace`, always `-p`.
- **Gate crates:** `tau-cli` and `tau-sdk-codegen` only.
- **Error handling:** `thiserror` at the `tau-sdk-codegen` boundary; `anyhow` in the `tau-cli` shim. `#![forbid(unsafe_code)]` already crate-wide via `[workspace.lints]` — do not weaken.
- **Emitter purity:** `render_embed_rust` / `render_embed_c` are pure `fn(...) -> BTreeMap<PathBuf, String>` — no filesystem, no subprocess, no external tool. Unit-tested by string assertion, never by compiling the emitted crate (mirrors `emit_rust_lib.rs`'s test).
- **Commits:** `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "..."` (lefthook pre-commit contends on the shared target lock; the corrupting integration test can rewrite git identity).
- **Output layout:** rendered `BTreeMap` keys are relative (`embed-rust/...`, `embed-c/...`); the CLI joins them under `--output` (default `.`), same write loop as today's `embed.rs`.

---

### Task 1: `EmbedArgs` gains `project`, `--host` validated + routed

**Files:**
- Modify: `crates/tau-cli/src/cli.rs:715-725` (EmbedArgs struct)
- Modify: `crates/tau-cli/src/cmd/embed.rs` (the `host` match + validation)
- Test: `crates/tau-cli/src/cmd/embed.rs` (`#[cfg(test)]` module — validation unit test)

**Interfaces:**
- Consumes: nothing new.
- Produces: `EmbedArgs { host: String, output: Option<PathBuf>, project: Option<PathBuf> }`; a `pub(crate) fn validate_host(host: &str) -> anyhow::Result<()>` returning `Ok` for `"js"|"rust"|"c"` and an `Err` naming the three valid hosts otherwise.

- [ ] **Step 1: Write the failing test** — in `crates/tau-cli/src/cmd/embed.rs` `#[cfg(test)]`:

```rust
#[cfg(test)]
mod tests {
    use super::validate_host;

    #[test]
    fn validate_host_accepts_the_three_hosts_and_rejects_others() {
        for h in ["js", "rust", "c"] {
            assert!(validate_host(h).is_ok(), "{h} should be valid");
        }
        let err = validate_host("go").unwrap_err().to_string();
        assert!(err.contains("unsupported --host 'go'"), "{err}");
        assert!(err.contains("js"), "{err}");
        assert!(err.contains("rust"), "{err}");
        assert!(err.contains("c"), "{err}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e52 cargo nextest run -p tau-cli validate_host`
Expected: FAIL — `validate_host` not found.

- [ ] **Step 3: Add the `project` field to `EmbedArgs`** (`crates/tau-cli/src/cli.rs`), replacing the struct body:

```rust
/// Arguments for `tau embed`.
#[derive(clap::Args, Debug)]
pub struct EmbedArgs {
    /// Host language for the generated glue: `js` (shipped), `rust`, or `c`.
    #[arg(long, value_name = "HOST")]
    pub host: String,
    /// Project to derive IR + WIT from (directory with `tau.toml`, or a
    /// `.ts` file). Required for `--host rust|c`; ignored for `--host js`
    /// (that scaffold is project-independent). Defaults to the CWD.
    #[arg(value_name = "PROJECT")]
    pub project: Option<PathBuf>,
    /// Output directory (default: current directory). Files land under
    /// `<dir>/embed-{rust,c}/` or `<dir>/sdk/embed-js/`.
    #[arg(long, short = 'o', value_name = "DIR")]
    pub output: Option<PathBuf>,
}
```

- [ ] **Step 4: Add `validate_host` + route the match** in `crates/tau-cli/src/cmd/embed.rs`, replacing the current `if args.host != "js"` guard:

```rust
/// Accept only the three supported hosts, with a message that names them.
pub(crate) fn validate_host(host: &str) -> Result<()> {
    match host {
        "js" | "rust" | "c" => Ok(()),
        other => bail!("unsupported --host '{other}': expected one of js, rust, c"),
    }
}
```

Keep the existing `js` body reachable through a `match args.host.as_str()` added in Task 4; for now call `validate_host(&args.host)?` at the top of `run` and leave the `js` write loop intact for non-`js` via a temporary `_ =>` arm that `bail!`s "not yet wired" (removed in Task 4). This keeps the crate compiling between tasks.

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e52 cargo nextest run -p tau-cli validate_host`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-cli/src/cli.rs crates/tau-cli/src/cmd/embed.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(cli): tau embed accepts host=rust|c + project arg (EPIC 5.2)"
```

---

### Task 2: `render_embed_rust` — native host crate (drives rust-lib)

**Files:**
- Create: `crates/tau-sdk-codegen/src/embed_rust.rs`
- Modify: `crates/tau-sdk-codegen/src/lib.rs:14-21` (add `pub mod embed_rust;` + re-export)
- Test: `crates/tau-sdk-codegen/src/embed_rust.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: nothing (pure renderer).
- Produces:
  ```rust
  pub struct EmbedRustInput<'a> {
      pub crate_name: &'a str,   // sanitized project stem (host crate name)
      pub lib_crate_name: &'a str, // the 5.1 rust-lib crate name it links
      pub ir_hash: &'a str,      // lowercase-hex, for README provenance
      pub wit: &'a str,          // cap-derived WIT world (embedded verbatim)
      pub tau_version: &'a str,  // pinned tau-runtime-core / tau-ir version
  }
  pub fn render_embed_rust(input: EmbedRustInput) -> BTreeMap<PathBuf, String>;
  ```
  Returned keys: `embed-rust/Cargo.toml`, `embed-rust/src/main.rs`, `embed-rust/tau.wit`, `embed-rust/README.md`.

- [ ] **Step 1: Write the failing test** (in the new file's `#[cfg(test)]`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn render_embed_rust_emits_host_crate_that_drives_run_ir() {
        let out = render_embed_rust(EmbedRustInput {
            crate_name: "trivial_host",
            lib_crate_name: "trivial",
            ir_hash: "abc123",
            wit: "package tau:generated@0.1.0;\nworld runner {\n    import tau:host/host@0.1.0;\n}\n",
            tau_version: "0.0.0",
        });

        for f in ["Cargo.toml", "src/main.rs", "tau.wit", "README.md"] {
            assert!(
                out.contains_key(&PathBuf::from(format!("embed-rust/{f}"))),
                "missing embed-rust/{f}"
            );
        }

        let main = &out[&PathBuf::from("embed-rust/src/main.rs")];
        // Links the 5.1 rust-lib crate and drives run_ir with a stub dispatcher.
        assert!(main.contains("use trivial::{run_ir, TAU_IR}"), "{main}");
        assert!(main.contains("impl ToolDispatcher for StubDispatcher"), "{main}");
        assert!(main.contains("todo!("), "port bodies must be todo!() stubs: {main}");
        assert!(main.contains("run_ir("), "{main}");

        let cargo = &out[&PathBuf::from("embed-rust/Cargo.toml")];
        assert!(cargo.contains(r#"trivial = { path = ".." }"#), "{cargo}");
        assert!(cargo.contains(r#"tau-runtime-core = { version = "0.0.0""#), "{cargo}");

        assert_eq!(
            out[&PathBuf::from("embed-rust/tau.wit")],
            "package tau:generated@0.1.0;\nworld runner {\n    import tau:host/host@0.1.0;\n}\n"
        );
        assert!(out[&PathBuf::from("embed-rust/README.md")].contains("abc123"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e52 cargo nextest run -p tau-sdk-codegen render_embed_rust`
Expected: FAIL — module/function not found.

- [ ] **Step 3: Write `render_embed_rust`** in `crates/tau-sdk-codegen/src/embed_rust.rs`:

```rust
//! `embed_rust` — render the native HOST crate that drives the EPIC 5.1
//! rust-lib (Variant B) artifact.
//!
//! Sibling to `emit_rust_lib` (which emits the *library* crate baking the IR).
//! This emits the *host* crate that links that library, supplies port impls,
//! and calls `run_ir(TAU_IR, …)`. Bodies are `todo!()` stubs: real port impls
//! are the product's job (EPIC 7.1). Pure string renderer — never compiled here.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Inputs for [`render_embed_rust`].
pub struct EmbedRustInput<'a> {
    /// Sanitized project stem used as the host crate name.
    pub crate_name: &'a str,
    /// Name of the EPIC 5.1 rust-lib crate this host links (path dep `..`).
    pub lib_crate_name: &'a str,
    /// Lowercase-hex IR module hash (README provenance).
    pub ir_hash: &'a str,
    /// Cap-derived WIT world text, embedded verbatim for reference.
    pub wit: &'a str,
    /// Pinned `tau-runtime-core` / `tau-ir` dependency version.
    pub tau_version: &'a str,
}

/// Render the rust HOST scaffold as crate-relative-path -> contents under
/// `embed-rust/`. A product drops this beside the rust-lib crate, fills in the
/// `StubDispatcher` port bodies (EPIC 7.1), and runs it to drive the workflow.
pub fn render_embed_rust(input: EmbedRustInput) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();

    out.insert(
        PathBuf::from("embed-rust/Cargo.toml"),
        format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"
publish = false

# Generated by `tau embed --host rust` (EPIC 5.2). Native host scaffold: links
# the sibling rust-lib crate, supplies port impls (EPIC 7.1), drives run_ir.
[dependencies]
{lib} = {{ path = ".." }}
tau-runtime-core = {{ version = "{ver}" }}
tau-ir = {{ version = "{ver}" }}
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}
"#,
            name = input.crate_name,
            lib = input.lib_crate_name,
            ver = input.tau_version,
        ),
    );

    out.insert(
        PathBuf::from("embed-rust/src/main.rs"),
        format!(
            r#"//! Generated by `tau embed --host rust` (EPIC 5.2) — a scaffold.
//!
//! Links the rust-lib crate (`{lib}`), decodes its baked `TAU_IR`, and drives
//! `run_ir` with a dispatcher whose port bodies are `todo!()` stubs. Fill them
//! in (EPIC 7.1) to bridge tools + inference to your product, then run.
//!
//! Capabilities the dispatcher must service are the WIT imports in `tau.wit`.

use std::sync::Arc;

use tau_ir::{{from_canonical_bytes, ToolId, Value}};
use tau_runtime_core::builder::DynLlmBackend;
use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::tool_dispatch::{{ToolDispatcher, ToolInvocationResult}};
use {lib}::{{run_ir, TAU_IR}};

/// Product port surface. Every method is a `todo!()` stub — EPIC 7.1 supplies
/// the real bodies (tool execution + LLM backend resolution). The default trait
/// methods (clock/random/assets/…) are inherited; override as your host needs.
struct StubDispatcher;

impl ToolDispatcher for StubDispatcher {{
    fn invoke<'a>(
        &'a self,
        _tool_id: &'a ToolId,
        _args: &'a Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>,
    > {{
        todo!("EPIC 7.1: execute the tool identified by tool_id")
    }}

    fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {{
        todo!("EPIC 7.1: resolve the named LLM backend")
    }}
}}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    let module = Arc::new(from_canonical_bytes(TAU_IR)?);
    let entry = module.workflow.entry_agent().clone();
    let outcome = run_ir(module, &entry, Arc::new(StubDispatcher), Vec::new()).await?;
    println!("{{outcome:?}}");
    Ok(())
}}
"#,
            lib = input.lib_crate_name,
        ),
    );

    out.insert(PathBuf::from("embed-rust/tau.wit"), input.wit.to_string());

    out.insert(
        PathBuf::from("embed-rust/README.md"),
        format!(
            "# {name} (rust host scaffold)\n\n\
             Generated by `tau embed --host rust` (EPIC 5.2). Baked IR hash: \
             `{hash}`.\n\n\
             Native host for the sibling rust-lib crate (`{lib}`). Link it, fill \
             in `StubDispatcher`'s `todo!()` port bodies (tool execution + LLM \
             backend — EPIC 7.1), and `cargo run`. The capabilities your \
             dispatcher must service are the WIT imports in `tau.wit`.\n",
            name = input.crate_name,
            hash = input.ir_hash,
            lib = input.lib_crate_name,
        ),
    );

    out
}
```

- [ ] **Step 4: Register the module** in `crates/tau-sdk-codegen/src/lib.rs` — add `pub mod embed_rust;` in the module block and `pub use embed_rust::{render_embed_rust, EmbedRustInput};` in the re-export block.

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e52 cargo nextest run -p tau-sdk-codegen render_embed_rust`
Expected: PASS.

> **Note on `entry_agent()`/`from_canonical_bytes`:** these are the names the emitted *source* references; the emitter is string-only so a name mismatch will NOT fail this task's test. Before Task 4's integration test, grep `tau-ir` / `tau-runtime-core` for the real entry-agent accessor and canonical-decode fn; if they differ, fix the template string (the emitted crate is 7.1's to compile — we only keep the scaffold honest).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-sdk-codegen/src/embed_rust.rs crates/tau-sdk-codegen/src/lib.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(codegen): render_embed_rust — native host crate for rust-lib (EPIC 5.2)"
```

---

### Task 3: `render_embed_c` — wasmtime-C-API host stub (drives wasm-guest)

**Files:**
- Create: `crates/tau-sdk-codegen/src/embed_c.rs`
- Modify: `crates/tau-sdk-codegen/src/lib.rs` (add `pub mod embed_c;` + re-export)
- Test: `crates/tau-sdk-codegen/src/embed_c.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: nothing (pure renderer).
- Produces:
  ```rust
  pub struct EmbedCInput<'a> {
      pub base_name: &'a str,   // sanitized stem — file prefix + include guard
      pub ir_hash: &'a str,
      pub wit: &'a str,         // cap-derived WIT world, embedded verbatim
  }
  pub fn render_embed_c(input: EmbedCInput) -> BTreeMap<PathBuf, String>;
  ```
  Returned keys: `embed-c/tau_embed.h`, `embed-c/tau_embed.c`, `embed-c/tau.wit`, `embed-c/README.md`.

- [ ] **Step 1: Write the failing test**:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn render_embed_c_emits_wasmtime_host_stub_with_wit_imports() {
        let wit = "package tau:generated@0.1.0;\n\nworld runner {\n    \
            import tau:host/host@0.1.0;\n    export run: func(prompt: string) \
            -> result<string, string>;\n}\n";
        let out = render_embed_c(EmbedCInput { base_name: "trivial", ir_hash: "abc123", wit });

        for f in ["tau_embed.h", "tau_embed.c", "tau.wit", "README.md"] {
            assert!(out.contains_key(&PathBuf::from(format!("embed-c/{f}"))), "missing {f}");
        }

        let h = &out[&PathBuf::from("embed-c/tau_embed.h")];
        // Include guard from base_name; the four frozen tau:host/host imports.
        assert!(h.contains("#ifndef TAU_EMBED_TRIVIAL_H"), "{h}");
        for sym in ["tau_host_complete", "tau_host_now_millis", "tau_host_next_u64", "tau_host_emit_event"] {
            assert!(h.contains(sym), "header missing {sym}");
        }
        assert!(h.contains("tau_embed_run"), "{h}");

        let c = &out[&PathBuf::from("embed-c/tau_embed.c")];
        assert!(c.contains("#include \"tau_embed.h\""), "{c}");
        assert!(c.contains("wasmtime"), "must use the wasmtime C API: {c}");
        assert!(c.contains("/* TODO(EPIC 7.1)"), "port bodies must be TODO stubs: {c}");

        assert_eq!(out[&PathBuf::from("embed-c/tau.wit")], wit);
        assert!(out[&PathBuf::from("embed-c/README.md")].contains("wit-bindgen"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e52 cargo nextest run -p tau-sdk-codegen render_embed_c`
Expected: FAIL — module/function not found.

- [ ] **Step 3: Write `render_embed_c`** in `crates/tau-sdk-codegen/src/embed_c.rs`:

```rust
//! `embed_c` — render the C HOST glue that loads + drives the EPIC 5.1
//! wasm-guest component via the wasmtime C API.
//!
//! The four `tau:host/host` imports (wit/tau-host.wit) are a frozen contract,
//! so the header declares them directly; the cap-derived WASI imports in the
//! generated world are satisfied by `wasmtime_wasi` and need no host bodies.
//! Bodies are `TODO(EPIC 7.1)` stubs. Pure string renderer — no tool runs at
//! emit time. The embedded `tau.wit` lets a product regenerate typed bindings
//! with `wit-bindgen` / the wasmtime C API if it wants more than this stub.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Inputs for [`render_embed_c`].
pub struct EmbedCInput<'a> {
    /// Sanitized project stem: uppercased for the include guard, lowercased in
    /// docs/prose. File names are fixed (`tau_embed.{h,c}`).
    pub base_name: &'a str,
    /// Lowercase-hex IR module hash (README provenance).
    pub ir_hash: &'a str,
    /// Cap-derived WIT world text, embedded verbatim.
    pub wit: &'a str,
}

/// Render the C host scaffold as relative-path -> contents under `embed-c/`.
pub fn render_embed_c(input: EmbedCInput) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    let guard = format!(
        "TAU_EMBED_{}_H",
        input
            .base_name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
            .collect::<String>()
    );

    out.insert(
        PathBuf::from("embed-c/tau_embed.h"),
        format!(
            r#"/* Generated by `tau embed --host c` (EPIC 5.2) — do not edit. */
#ifndef {guard}
#define {guard}

#include <stdint.h>

/* The four `tau:host/host` imports (wit/tau-host.wit) the guest requires.
 * Wire these to your product in tau_embed.c (EPIC 7.1). `request_json` /
 * `event_json` are NUL-terminated serialized JSON; returned strings are
 * heap-allocated and owned by the caller. */
char    *tau_host_complete(const char *request_json);   /* -> CompletionResponse JSON */
uint64_t tau_host_now_millis(void);
uint64_t tau_host_next_u64(void);
void     tau_host_emit_event(const char *event_json);

/* Load the bundled wasm-guest component, wire the imports above, and drive its
 * `run(prompt)` export. Returns 0 on success. RunEvents stream through
 * tau_host_emit_event. `component_path` is the `.wasm` from `tau build
 * --target wasm-guest`. */
int tau_embed_run(const char *component_path, const char *prompt);

#endif /* {guard} */
"#,
        ),
    );

    out.insert(
        PathBuf::from("embed-c/tau_embed.c"),
        r#"/* Generated by `tau embed --host c` (EPIC 5.2) — a scaffold.
 *
 * Drives a tau wasm-guest component through the wasmtime C API. The host-import
 * bodies and the instantiate/call wiring are TODO(EPIC 7.1) stubs: this file
 * compiles the shape of the glue, not a working runtime. Link against
 * libwasmtime and fill the TODOs. */
#include "tau_embed.h"

#include <stdlib.h>
#include <string.h>

/* wasmtime C API — provided by libwasmtime (https://docs.wasmtime.dev/c-api). */
#include <wasmtime.h>

char *tau_host_complete(const char *request_json) {
    (void)request_json;
    /* TODO(EPIC 7.1): bridge to an LLM backend; return CompletionResponse JSON. */
    return NULL;
}

uint64_t tau_host_now_millis(void) {
    /* TODO(EPIC 7.1): return a wall-clock millisecond count. */
    return 0;
}

uint64_t tau_host_next_u64(void) {
    /* TODO(EPIC 7.1): return the next u64 from your RandomSource. */
    return 0;
}

void tau_host_emit_event(const char *event_json) {
    (void)event_json;
    /* TODO(EPIC 7.1): consume the streamed RunEvent JSON. */
}

int tau_embed_run(const char *component_path, const char *prompt) {
    (void)component_path;
    (void)prompt;
    /* TODO(EPIC 7.1): with the wasmtime C API —
     *   1. wasm_engine_new / wasmtime_store_new (+ wasmtime_wasi for the WASI
     *      imports listed in tau.wit),
     *   2. wasmtime_component_from_file(component_path),
     *   3. wasmtime_component_linker_define the four tau:host/host imports to
     *      the tau_host_* functions above,
     *   4. instantiate and call the `run(prompt)` export.
     * See tau.wit for the exact import/export set. */
    return -1;
}
"#
        .to_string(),
    );

    out.insert(PathBuf::from("embed-c/tau.wit"), input.wit.to_string());

    out.insert(
        PathBuf::from("embed-c/README.md"),
        format!(
            "# tau embed --host c ({base} host scaffold)\n\n\
             Generated by `tau embed --host c` (EPIC 5.2). Baked IR hash: \
             `{hash}`.\n\n\
             C host glue that loads a `tau build --target wasm-guest` component \
             through the [wasmtime C API](https://docs.wasmtime.dev/c-api) and \
             drives its `run(prompt)` export. `tau_embed.h` declares the four \
             frozen `tau:host/host` imports plus `tau_embed_run`; fill in the \
             `TODO(EPIC 7.1)` bodies in `tau_embed.c` and link `libwasmtime`.\n\n\
             `tau.wit` is the component's WIT world, embedded for reference. To \
             regenerate typed bindings instead of hand-writing them, run \
             `wit-bindgen c tau.wit` (guest side) or use the wasmtime C API's \
             component bindings (host side).\n",
            base = input.base_name,
            hash = input.ir_hash,
        ),
    );

    out
}
```

- [ ] **Step 4: Register the module** in `crates/tau-sdk-codegen/src/lib.rs` — `pub mod embed_c;` + `pub use embed_c::{render_embed_c, EmbedCInput};`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e52 cargo nextest run -p tau-sdk-codegen render_embed_c`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-sdk-codegen/src/embed_c.rs crates/tau-sdk-codegen/src/lib.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(codegen): render_embed_c — wasmtime-C-API host stub for wasm-guest (EPIC 5.2)"
```

---

### Task 4: Wire `embed.rs` to the three renderers + JSON parity

**Files:**
- Modify: `crates/tau-cli/src/cmd/embed.rs` (the `run` dispatch)
- Test: `crates/tau-cli/tests/` — new integration test file `embed_hosts.rs`

**Interfaces:**
- Consumes: `validate_host` (Task 1); `render_embed_rust`/`EmbedRustInput` (Task 2); `render_embed_c`/`EmbedCInput` (Task 3); `build_wasm::{lower_to_wasm_ir, world_from_module}` and `build::{sanitize_crate_name, hex_lower}` (existing — make the latter two `pub(crate)` if not already; `hex_lower` is `pub(crate)`, `sanitize_crate_name` is private → widen to `pub(crate)`).
- Produces: `run` writes the selected scaffold and prints a human line + (under `--json`) a `{ "kind": "embed-<host>", "path": <out_root>, "files": <n> }` object matching `build`'s emitter shape.

- [ ] **Step 1: Write the failing integration test** in `crates/tau-cli/tests/embed_hosts.rs`. Use the existing fixture pattern from the rust-lib emit test (find it first: `rg -l "emit_rust_lib_to|--target rust-lib|rust-lib" crates/tau-cli/tests`). Mirror that fixture's project setup, then:

```rust
// Shape-check: `tau embed --host rust|c <project>` writes the scaffold.
// Follows the same tempdir+fixture pattern as the rust-lib emit test.

#[test]
fn embed_host_rust_writes_native_host_crate() {
    let tmp = /* fixture project dir (copy from rust-lib test) */;
    let out = tempfile::tempdir().unwrap();
    // invoke embed::run via the same helper the rust-lib test uses, or shell
    // the built binary with: embed --host rust <project> -o <out>
    // Assert the files exist and carry the WIT-derived shape:
    let main = std::fs::read_to_string(out.path().join("embed-rust/src/main.rs")).unwrap();
    assert!(main.contains("run_ir("));
    assert!(main.contains("impl ToolDispatcher"));
    assert!(out.path().join("embed-rust/tau.wit").exists());
    assert!(out.path().join("embed-rust/Cargo.toml").exists());
}

#[test]
fn embed_host_c_writes_wasmtime_host_stub() {
    let out = tempfile::tempdir().unwrap();
    // embed --host c <project> -o <out>
    let header = std::fs::read_to_string(out.path().join("embed-c/tau_embed.h")).unwrap();
    assert!(header.contains("tau_host_complete"));
    assert!(header.contains("tau_embed_run"));
    let wit = std::fs::read_to_string(out.path().join("embed-c/tau.wit")).unwrap();
    assert!(wit.contains("world runner"));
}
```

> Before writing this, `rg -n "fn .*embed|embed --host js|render_embed_js" crates/tau-cli/tests` to reuse whatever harness the shipped `js` test uses (assert-cmd vs direct `cmd::embed::run`). Match it exactly so this file compiles against the existing test deps.

- [ ] **Step 2: Run to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e52 cargo nextest run -p tau-cli embed_host`
Expected: FAIL — `rust`/`c` still hit the temporary `bail!` arm from Task 1.

- [ ] **Step 3: Implement the dispatch** in `crates/tau-cli/src/cmd/embed.rs` — replace the temporary arm:

```rust
pub async fn run(args: &EmbedArgs, output: &mut Output) -> Result<()> {
    validate_host(&args.host)?;
    let out_root = args.output.clone().unwrap_or_else(|| PathBuf::from("."));

    let (kind, rendered) = match args.host.as_str() {
        "js" => ("embed-js", tau_sdk_codegen::embed_js::render_embed_js()),
        "rust" => ("embed-rust", render_rust_host(args)?),
        "c" => ("embed-c", render_c_host(args)?),
        other => bail!("unsupported --host '{other}': expected one of js, rust, c"),
    };

    for (rel, contents) in &rendered {
        let path = out_root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents)?;
    }

    output.emit_embed(kind, &out_root, rendered.len())?; // human + JSON parity
    Ok(())
}
```

Add two private helpers in `embed.rs` that reuse the build path (project defaults to CWD):

```rust
fn project_path(args: &EmbedArgs) -> PathBuf {
    args.project.clone().unwrap_or_else(|| PathBuf::from("."))
}

fn render_rust_host(args: &EmbedArgs) -> Result<BTreeMap<PathBuf, String>> {
    use crate::cmd::build_wasm::{lower_to_wasm_ir, world_from_module};
    use crate::cmd::build::{hex_lower, sanitize_crate_name};
    let project = project_path(args);
    let (module, _bytes) = lower_to_wasm_ir(&project)?;
    let ir_hash = hex_lower(&tau_ir::compute_hash(&module));
    let wit = world_from_module(&module)?;
    let stem = project.file_name().and_then(|n| n.to_str()).unwrap_or("workflow");
    let lib = sanitize_crate_name(stem);
    let host = format!("{lib}_host");
    Ok(tau_sdk_codegen::render_embed_rust(tau_sdk_codegen::EmbedRustInput {
        crate_name: &host,
        lib_crate_name: &lib,
        ir_hash: &ir_hash,
        wit: &wit,
        tau_version: env!("CARGO_PKG_VERSION"),
    }))
}

fn render_c_host(args: &EmbedArgs) -> Result<BTreeMap<PathBuf, String>> {
    use crate::cmd::build_wasm::{lower_to_wasm_ir, world_from_module};
    use crate::cmd::build::{hex_lower, sanitize_crate_name};
    let project = project_path(args);
    let (module, _bytes) = lower_to_wasm_ir(&project)?;
    let ir_hash = hex_lower(&tau_ir::compute_hash(&module));
    let wit = world_from_module(&module)?;
    let stem = project.file_name().and_then(|n| n.to_str()).unwrap_or("workflow");
    let base = sanitize_crate_name(stem);
    Ok(tau_sdk_codegen::render_embed_c(tau_sdk_codegen::EmbedCInput {
        base_name: &base,
        ir_hash: &ir_hash,
        wit: &wit,
    }))
}
```

Widen `build::sanitize_crate_name` to `pub(crate)`. Add an `emit_embed(kind, path, files)` method to `Output` mirroring how the rust-lib artifact is reported (find it: `rg -n "RustLibArtifact|emit_.*artifact|\"kind\"" crates/tau-cli/src/output.rs crates/tau-cli/src/cmd/build.rs`) — reuse the exact JSON field names (`kind`/`path`/`files`). If `build` inlines its JSON rather than using an `Output` method, inline the same shape here instead of adding a method.

- [ ] **Step 4: Run the integration tests + the full crate suite**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e52 cargo nextest run -p tau-cli`
Expected: PASS (new `embed_host_*` tests + the shipped `js` test still green).

- [ ] **Step 5: Verify the emitted rust template names are real** — `rg -n "pub fn from_canonical_bytes|fn entry_agent|pub fn compute_hash" crates/tau-ir/src`. If `from_canonical_bytes`/`entry_agent` differ from the template in Task 2, fix the `embed_rust.rs` template string and re-run Task 2's unit test. (The scaffold need not compile in CI, but keep symbol names honest.)

- [ ] **Step 6: Commit**

```bash
git add crates/tau-cli/src/cmd/embed.rs crates/tau-cli/src/cmd/build.rs \
        crates/tau-cli/src/output.rs crates/tau-cli/tests/embed_hosts.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(cli): route tau embed to rust/c host renderers + JSON parity (EPIC 5.2)"
```

---

### Task 5: Docs — one example per host

**Files:**
- Modify: the existing `tau embed` reference/how-to page (find: `rg -rln "tau embed|embed-js|--host js" docs`) — add `rust` + `c` examples.
- Modify: `docs/SUMMARY.md` only if a NEW page is created (prefer extending the existing embed page — no new SUMMARY entry needed then).

- [ ] **Step 1: Add the examples** to the existing embed doc page. Show all three invocations and the output tree per host:

````markdown
## Host targets

`tau embed --host <js|rust|c>` emits host-side glue that drives a `tau build`
artifact. `rust` and `c` derive their WIT world from `<project>` (default: CWD).

```sh
tau embed --host js               # @tau/embed-js scaffold (project-independent)
tau embed --host rust ./my-flow   # native host crate → drives the rust-lib artifact
tau embed --host c    ./my-flow   # wasmtime C-API host → drives the wasm-guest component
```

`--host rust` writes `embed-rust/` (a `Cargo.toml` + `src/main.rs` linking the
5.1 rust-lib crate, `tau.wit`, `README.md`); `--host c` writes `embed-c/`
(`tau_embed.h` + `tau_embed.c` wasmtime host stub, `tau.wit`, `README.md`).
Port bodies are `todo!()` / `TODO(EPIC 7.1)` stubs — the product fills them in.
````

- [ ] **Step 2: Build the book** (docs rules — from `docs/`):

Run: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build`
Expected: only `[INFO]` lines, no linkcheck errors. Then `rm -rf docs/book`.

- [ ] **Step 3: Commit**

```bash
git add docs/
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "docs(epic-5.2): tau embed --host rust|c examples"
```

---

## Pre-PR gate (run after all tasks)

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e52 cargo nextest run -p tau-sdk-codegen
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e52 cargo nextest run -p tau-cli
timeout 30  env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e52 cargo fmt -p tau-sdk-codegen -p tau-cli -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e52 cargo clippy -p tau-sdk-codegen -p tau-cli --all-targets -- -D warnings
```

All green → `git push -u origin feat/epic-5-2-tau-embed` → `gh pr create --base main` →
`gh pr merge <N> --squash --auto` (NO `--delete-branch`; merge queue is healthy).

## Self-Review notes

- **Spec coverage:** DoD 1 (rust host)→Task 2+4; DoD 2 (c host)→Task 3+4; DoD 3 (js unchanged, one match)→Task 4; DoD 4 (validation+help+JSON parity)→Task 1+4; DoD 5 (one test/host + one docs example)→Tasks 2/3/4 tests + Task 5 docs.
- **Type consistency:** `render_embed_rust`/`EmbedRustInput`, `render_embed_c`/`EmbedCInput`, `validate_host`, `emit_embed` used identically across tasks.
- **Known soft spot:** the emitted Rust `main.rs` references `from_canonical_bytes`/`entry_agent()` — Task 4 Step 5 verifies these names before PR. The scaffold is not compiled in CI (5.1's rust-lib set the precedent: string-check only), so a name slip is a doc-quality bug, not a test failure; 7.1's example is where it's compiled end-to-end.
