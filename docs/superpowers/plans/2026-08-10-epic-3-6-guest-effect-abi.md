# EPIC 3.6 — Guest Effect ABI (net-only) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route the guest's `net.http` effect through `wasi:http` using the guest's own cap-derived generated bindings, making EPIC 3's DoD binary-observable and the host WasiCtx the live runtime gate.

**Architecture:** `tau build wasm` already generates a capability-exact WIT world (3.2) and the host already builds a WasiCtx + egress gate from caps (3.3). This plan closes the loop: `build.rs` emits a `tau_cap_net_http` cfg from the generated world text; a `#[cfg(tau_cap_net_http)]` arm in `GuestDispatcher::invoke` calls the guest's `generate_all`-generated `wasi:http` bindings; the granted import then survives `wasm-ld` DCE (binary-observable) and is enforced live by the host at runtime.

**Tech Stack:** Rust, `wasm32-wasip2`, `wit-bindgen` (generated bindings — NOT the external `wasi` crate), `wasmtime`/`wasmtime-wasi` (host), `wit_component::decode` (DoD assertion).

**Spec:** `docs/superpowers/specs/2026-08-10-epic-3-6-guest-effect-abi-design.md`

## Global Constraints

- **CARGO_TARGET_DIR always set** — per-role isolated target dir; main agent uses `target/main`, subagents use `target/agent-<role>`. Never bare `cargo`.
- **Scope to a single crate** — always `-p <crate>`; never `--workspace`.
- **Wrap with `timeout`** — test 300s, build/check 180s, clippy 240s, fmt 30s.
- **`CARGO_INCREMENTAL=0`** on every cargo invocation (sccache dedup).
- **Prefer `cargo nextest run`** for tests; `cargo test --doc` for doctests.
- **Run cargo FOREGROUND** — commands carry `timeout`; do not background + yield.
- **`cargo fmt --all --check`** before every push — rustfmt is a SEPARATE required CI gate; clippy/nextest green ≠ fmt-clean.
- **Wasm target build:** `cargo build -p tau-wasm-guest --target wasm32-wasip2` (target already installed).
- **Never commit fixture `Cargo.lock` files** (the `http-probe`/`fs-probe` fixtures are standalone non-workspace roots; their `Cargo.lock` is a build artifact).
- **Remote is `tau-rs/tau`.** Merge queue: enroll bare `gh pr merge <n> --auto` (`--squash`/`-d` rejected while queue enabled). `gh pr update-branch <n>` if BEHIND (strict protection).
- **Keep 3.4 sound:** do NOT re-add an in-guest cap gate; `InGuest` caps stay gated; `AttenuatedDispatcher` and `tau_native_tools` untouched.
- **No external `wasi` crate** in the production guest — it would emit a second `cabi_realloc` (dup-symbol link error). Generated `generate_all` bindings only.

---

## File Structure

- `crates/tau-wasm-guest/build.rs` — emit `tau_cap_net_http` cfg + `check-cfg` from the world text (already reads the world into a `world` byte vec).
- `crates/tau-wasm-guest/src/guest.rs` — re-export the generated `wasi` bindings (cfg-gated); reorder `Arc::new(module)` before dispatcher construction; pass `module` into `GuestDispatcher::new`.
- `crates/tau-wasm-guest/src/lib.rs` — re-export `guest::wit_wasi` (cfg-gated) so `dispatcher.rs` can reach it as `crate::wit_wasi`.
- `crates/tau-wasm-guest/src/dispatcher.rs` — `GuestDispatcher` gains `module: Arc<tau_ir::Module>` + a `native_fn_name` resolver + the `#[cfg(tau_cap_net_http)]` `Fetch` arm over generated `wasi:http` bindings.
- `crates/tau-cli/tests/build_wasm_world_dod.rs` — add the positive binary-observable assertion (net-http's compiled component imports `wasi:http`).
- `crates/tau-cli/tests/wasi_http_roundtrip.rs` (NEW) — live host-enforcement round-trip: `tau build wasm` a net.http project → `run_component_with_caps` with a cassette firing `Fetch` at an ungranted host → assert exact `HttpRequestDenied`.
- `crates/tau-cli/Cargo.toml` — add `tau-wasm-host` as a dev-dependency if not already present (for `run_component_with_caps`).
- `docs/superpowers/plans/vision-roadmap.md` — tick story 3.6.

---

## Task 1: Route net.http through wasi:http (mechanism + binary-observable DoD)

This task is also the **blocking spike** (spec §Spike gate): if `generate_all` cannot produce callable no_std `wasi:http` bindings that compile+link, this task fails at Step 6 and the epic falls back to Tier-2 (world-text DoD only — stop and report).

**Files:**
- Modify: `crates/tau-wasm-guest/build.rs` (after the `world` vec is computed, ~line 78)
- Modify: `crates/tau-wasm-guest/src/guest.rs` (re-export block near `wit_host`, ~line 28; dispatcher construction, lines 138–147)
- Modify: `crates/tau-wasm-guest/src/lib.rs` (re-export, near line 22)
- Modify: `crates/tau-wasm-guest/src/dispatcher.rs` (struct + `new` + `invoke`)
- Test: `crates/tau-cli/tests/build_wasm_world_dod.rs` (add positive assertion)

**Interfaces:**
- Consumes: `tau_ir::{Module, ToolId, ToolImpl, NativeFnRef}`; `module.workflow.tools: BTreeMap<ToolId, Tool>` where `Tool.impl_: ToolImpl` and `ToolImpl::Native { fn_ref: NativeFnRef { name: String, .. } }`; generated `wasi::http::{outgoing_handler, types}` and `wasi::io::streams` (via `generate_all`).
- Produces:
  - `GuestDispatcher::new(backend, clock, random, module: Arc<tau_ir::Module>)` (new 4th arg).
  - cfg `tau_cap_net_http` set by `build.rs` iff the world text contains `wasi:http`.
  - `crate::wit_wasi` re-export (cfg-gated) exposing generated `http`/`io` modules to `dispatcher.rs`.

- [ ] **Step 1: Write the failing DoD assertion**

In `crates/tau-cli/tests/build_wasm_world_dod.rs`, inside `dod_guest_compiles_against_cap_exact_world`, after the existing `net_imports` block (after line 153), add the positive binary-observable assertion:

```rust
    // 3.6 binary-observable DoD: net.http granted AND routed through wasi:http
    // (GuestDispatcher's cfg-gated `Fetch` arm) → the compiled component's
    // ACTUAL imports now include wasi:http. This is the assertion that was
    // vacuous before 3.6 (DCE stripped every WASI import); it is now the live
    // proof that a granted cap is importable at the ABI.
    assert!(
        net_imports.iter().any(|i| i.starts_with("wasi:http/")),
        "net granted AND routed → wasi:http MUST be present in the compiled \
         component's actual imports (3.6 binary-observable DoD): {net_imports:?}"
    );
```

- [ ] **Step 2: Run the DoD test to verify it fails**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-cli --run-ignored all -E 'test(dod_guest_compiles_against_cap_exact_world)'
```
Expected: FAIL on the new assertion — `wasi:http` absent from `net_imports` (no guest source calls it yet; DCE strips it).

- [ ] **Step 3: Emit the `tau_cap_net_http` cfg from `build.rs`**

In `crates/tau-wasm-guest/build.rs`, immediately after `std::fs::write(wit_gen.join("runner.wit"), world)...` (line 79), add:

```rust
    // 3.6: the guest's net-effect arm (dispatcher.rs) is cfg-gated on whether
    // the capability-derived world grants wasi:http. When it does, the arm is
    // compiled and statically reachable from `run`, so the wasi:http import
    // survives wasm-ld DCE (binary-observable). When it doesn't, the arm is
    // absent and no wasi:http binding is referenced (the world has no wasi:http
    // to generate bindings from anyway). The check-cfg is unconditional so the
    // guest compiles cleanly on every target/world without an `unexpected cfg`
    // warning (workspace lints are -D warnings).
    println!("cargo:rustc-check-cfg=cfg(tau_cap_net_http)");
    if String::from_utf8_lossy(&world).contains("wasi:http") {
        println!("cargo:rustc-cfg=tau_cap_net_http");
    }
```

- [ ] **Step 4: Add the cfg-gated `wasi` re-export in `guest.rs` and `lib.rs`**

In `crates/tau-wasm-guest/src/guest.rs`, after the existing `wit_host` module (after line 30), add:

```rust
/// Re-export the `generate_all`-generated WASI bindings so sibling modules
/// (dispatcher.rs) can reach them without knowing the exact wit-bindgen path.
/// Only present when the capability-derived world granted wasi:http — the
/// `tau_cap_net_http` cfg (set by build.rs) gates both this re-export and the
/// effect arm that uses it, so the two are compiled in lockstep.
#[cfg(tau_cap_net_http)]
pub(crate) mod wit_wasi {
    pub(crate) use super::wasi::*;
}
```

In `crates/tau-wasm-guest/src/lib.rs`, after the `wit_host` re-export (after line 22), add:

```rust
#[cfg(all(target_arch = "wasm32", tau_cap_net_http))]
pub(crate) use guest::wit_wasi;
```

- [ ] **Step 5: Add `module` + the `Fetch` arm to `GuestDispatcher`**

In `crates/tau-wasm-guest/src/dispatcher.rs`:

Add imports at the top (with the existing `use` block):

```rust
use tau_ir::{Module, ToolImpl};
```

Add the field to the struct and constructor:

```rust
pub struct GuestDispatcher {
    backend: Arc<dyn DynLlmBackend>,
    clock: Arc<dyn Clock>,
    random: Arc<dyn RandomSource>,
    module: Arc<Module>,
}

impl GuestDispatcher {
    pub fn new(
        backend: Arc<dyn DynLlmBackend>,
        clock: Arc<dyn Clock>,
        random: Arc<dyn RandomSource>,
        module: Arc<Module>,
    ) -> Self {
        Self {
            backend,
            clock,
            random,
            module,
        }
    }

    /// Resolve a tool-ref id to its declared native fn name (the stable
    /// contract), e.g. `[tools.fetch] native = "Fetch"` → `"Fetch"`. The
    /// wasi-backed effect arm keys on THIS, not the arbitrary tool-ref key.
    fn native_fn_name(&self, tool_id: &ToolId) -> Option<&str> {
        match &self.module.workflow.tools.get(tool_id)?.impl_ {
            ToolImpl::Native { fn_ref } => Some(fn_ref.name.as_str()),
            _ => None,
        }
    }
}
```

Replace the body of `invoke`'s `async move` block so the wasi arm is tried first:

```rust
        let name = tool_id.0.clone();
        let native = self.native_fn_name(tool_id).map(|s| s.to_string());
        let args_owned = args.clone();
        Box::pin(async move {
            // 3.6 net effect: a tool declared `native = "Fetch"` routes through
            // wasi:http when net.http was granted (the cfg gate). Enforcement is
            // the HOST WasiCtx/EgressPolicy (3.3/3.4) — NOT an in-guest gate.
            #[cfg(tau_cap_net_http)]
            if native.as_deref() == Some("Fetch") {
                return match fetch_via_wasi(&args_owned) {
                    Ok(body) => Ok(ToolInvocationResult {
                        body: Some(body),
                        error: None,
                    }),
                    Err(msg) => Ok(ToolInvocationResult {
                        body: None,
                        error: Some(msg),
                    }),
                };
            }
            let _ = &native; // silence unused when the cfg arm is compiled out

            match tau_native_tools::invoke(&name, &args_owned) {
                Some(body) => Ok(ToolInvocationResult {
                    body: Some(body),
                    error: None,
                }),
                None => Err(RuntimeError::Internal {
                    message: format!("tau-wasm-guest: unknown native tool `{name}`"),
                }),
            }
        })
```

Add the effect fn at the bottom of `dispatcher.rs` (the response-body read path is offline-untested — only reachable with real connectivity, same limitation as the `http-probe` positive case; it is covered by "compiles + import survives DCE"):

```rust
/// Issue one outgoing HTTP request through the generated wasi:http bindings.
/// A host `EgressPolicy` denial (ungranted host/method) surfaces as
/// `Err("<ErrorCode>")` carrying the exact wasi:http error code (e.g.
/// `HttpRequestDenied`) — asserted by the round-trip test. Never panics.
#[cfg(tau_cap_net_http)]
fn fetch_via_wasi(args: &Value) -> Result<Value, alloc::string::String> {
    use crate::wit_wasi::http::outgoing_handler;
    use crate::wit_wasi::http::types::{Fields, Method, OutgoingRequest, Scheme};
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    let url = args
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| "Fetch: missing string arg `url`".to_string())?;
    let method_str = args.get("method").and_then(Value::as_str).unwrap_or("GET");

    let (scheme, rest) = if let Some(r) = url.strip_prefix("https://") {
        (Scheme::Https, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (Scheme::Http, r)
    } else {
        return Err(format!("Fetch: unsupported url scheme: {url}"));
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let method = match method_str {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "DELETE" => Method::Delete,
        "PATCH" => Method::Patch,
        other => Method::Other(other.to_string()),
    };

    let request = OutgoingRequest::new(Fields::new());
    request
        .set_method(&method)
        .map_err(|()| "Fetch: set_method rejected".to_string())?;
    request
        .set_scheme(Some(&scheme))
        .map_err(|()| "Fetch: set_scheme rejected".to_string())?;
    request
        .set_authority(Some(authority))
        .map_err(|()| "Fetch: set_authority rejected".to_string())?;
    request
        .set_path_with_query(Some(path))
        .map_err(|()| "Fetch: set_path rejected".to_string())?;

    // Host WasiHttpHooks::send_request runs here; a denied host/method returns
    // before any socket is opened.
    let future = outgoing_handler::handle(request, None).map_err(|code| format!("{code:?}"))?;
    let pollable = future.subscribe();
    pollable.block();
    let response = match future.get() {
        Some(Ok(Ok(resp))) => resp,
        Some(Ok(Err(code))) => return Err(format!("{code:?}")),
        Some(Err(())) => return Err("Fetch: future already consumed".to_string()),
        None => return Err("Fetch: no result after block".to_string()),
    };
    let status = response.status();

    // Response-body read (offline-untested; needs real connectivity).
    let body = response
        .consume()
        .map_err(|()| "Fetch: consume body".to_string())?;
    let stream = body
        .stream()
        .map_err(|()| "Fetch: body stream".to_string())?;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        match stream.blocking_read(8192) {
            Ok(chunk) => buf.extend_from_slice(&chunk),
            Err(_) => break, // Closed / stream error → end of body
        }
    }
    let body_str = String::from_utf8_lossy(&buf).into_owned();

    Ok(serde_json::json!({ "status": status, "body": body_str }))
}
```

In `crates/tau-wasm-guest/src/guest.rs`, reorder so `module` is an `Arc` before the dispatcher is built, and pass it in. Replace lines 142–146:

```rust
        let module = Arc::new(module);
        let dispatcher = Arc::new(crate::dispatcher::GuestDispatcher::new(
            backend,
            clock,
            random,
            module.clone(),
        ));
```

(Delete the now-duplicate `let module = Arc::new(module);` that previously sat at line 146.)

- [ ] **Step 6: Build the guest for wasm (SPIKE gate — both worlds)**

Confirm the generated bindings compile+link with the cfg ON (net.http world) and that the crate still builds with the cfg OFF (baseline world). Use the DoD test's build path indirectly by running the DoD test (Step 7), but first a fast standalone check of the cfg-OFF path:

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo build -p tau-wasm-guest --target wasm32-wasip2 --release
```
Expected: PASS (baseline world → cfg off → no wasi arm; existing behavior). If the crate fails to compile here, the re-export/arm is not correctly cfg-gated — fix before proceeding.

**If Step 7 fails to LINK the wasi:http bindings (not merely an assertion mismatch), the spike has FAILED — stop and report for the Tier-2 fallback decision.**

- [ ] **Step 7: Run the DoD test to verify it passes**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-cli --run-ignored all -E 'test(dod_guest_compiles_against_cap_exact_world)'
```
Expected: PASS — `net-http` guest now imports `wasi:http/*` (cfg on, arm reachable, import survives DCE); `trivial` still imports no `wasi:` (cfg off). This simultaneously confirms the spike (the net.http guest compiled AND linked the wasi:http bindings).

- [ ] **Step 8: fmt + clippy**

```bash
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt --all --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo clippy -p tau-wasm-guest --target wasm32-wasip2 --release -- -D warnings
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo clippy -p tau-cli -- -D warnings
```
Expected: all clean.

- [ ] **Step 9: Commit**

```bash
git add crates/tau-wasm-guest/build.rs crates/tau-wasm-guest/src/{guest.rs,lib.rs,dispatcher.rs} \
        crates/tau-cli/tests/build_wasm_world_dod.rs
git commit -m "feat(epic-3-6): route guest net.http through wasi:http (binary-observable DoD)"
```

---

## Task 2: Live host-enforcement round-trip (denial-only)

Proves the host WasiCtx/EgressPolicy is the live runtime gate through the REAL production guest driven by real IR — the 3.6 delta over the offline `http-probe`. Denial-only: offline a granted host can't open a socket (same limitation as `http-probe`'s omitted positive case).

**Files:**
- Create: `crates/tau-cli/tests/wasi_http_roundtrip.rs`
- Modify: `crates/tau-cli/Cargo.toml` (add `tau-wasm-host` dev-dependency if absent)

**Interfaces:**
- Consumes: `tau_cli::cmd::build_wasm::{lower_to_wasm_ir, wasm_world_for_project}` (returns `(Module, Vec<u8>)` and `String` respectively, per `build_wasm_world_dod.rs`); `tau_wasm_host::run_component_with_caps(wasm_bytes: &[u8], prompt: &str, llm_responses: Vec<String>, caps: &[tau_domain::Capability], sandbox_root: &Path) -> Result<(String, Vec<String>), _>`; `tau_domain::Capability`.
- Produces: nothing consumed downstream (leaf acceptance test).

- [ ] **Step 1: Ensure the `tau-wasm-host` dev-dependency exists**

Check `crates/tau-cli/Cargo.toml` `[dev-dependencies]`. If `tau-wasm-host` is absent, add:

```toml
tau-wasm-host = { workspace = true }
```
Verify it resolves:
```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo check -p tau-cli --tests
```
Expected: PASS.

- [ ] **Step 2: Write the failing round-trip test**

Create `crates/tau-cli/tests/wasi_http_roundtrip.rs`. It builds the `net-http` fixture guest (reusing the DoD build recipe), then runs it through the host with net.http granted to `api.anthropic.com` and a cassette that fires `Fetch` at the ungranted `blocked.invalid`, asserting the exact `HttpRequestDenied` code surfaces in the emitted events.

```rust
//! EPIC 3.6 live host-enforcement round-trip: the REAL production guest,
//! built by `tau build wasm`, driven by real IR, issues a `Fetch` through
//! wasi:http at an UNGRANTED host — the host `EgressPolicy` must deny it at
//! the WasiCtx before any socket, and the exact `HttpRequestDenied` code must
//! surface through the guest. Denial-only (offline; a granted host cannot open
//! a socket without a network, same as the `http-probe` positive case). Builds
//! the wasm32-wasip2 guest, so it is #[ignore]d (run with --run-ignored).

use std::path::{Path, PathBuf};
use std::process::Command;

use tau_cli::cmd::build_wasm::{lower_to_wasm_ir, wasm_world_for_project};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

/// Build the `net-http` guest and return its component bytes.
fn build_guest(fixture_name: &str) -> Vec<u8> {
    let (_module, ir_bytes) = lower_to_wasm_ir(&fixture(fixture_name)).unwrap();
    let world = wasm_world_for_project(&fixture(fixture_name)).unwrap();
    let ir = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(ir.path(), &ir_bytes).unwrap();
    let wit = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(wit.path(), world.as_bytes()).unwrap();

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let out = Command::new(env!("CARGO"))
        .current_dir(&root)
        .args([
            "build",
            "-p",
            "tau-wasm-guest",
            "--target",
            "wasm32-wasip2",
            "--release",
            "--message-format=json",
        ])
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", root.join("target/tau-build-wasm"))
        .env("TAU_IR_BYTES", ir.path())
        .env("TAU_WORLD_WIT", wit.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "guest build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let wasm_path = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact")
        .filter(|m| {
            m["target"]["name"]
                .as_str()
                .is_some_and(|n| n == "tau-wasm-guest" || n == "tau_wasm_guest")
        })
        .flat_map(|m| {
            m["filenames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .find(|f| f.ends_with(".wasm"))
        .unwrap();
    std::fs::read(&wasm_path).unwrap()
}

/// A cassette turn that calls the `fetch` tool, then a turn that ends.
fn cassette() -> Vec<String> {
    let tool_use = serde_json::json!({
        "text": "",
        "tool_uses": [{
            "id": "call_1",
            "name": "fetch",
            "input": { "url": "https://blocked.invalid/", "method": "GET" }
        }],
        "stop_reason": "ToolUse",
        "usage": null
    });
    let end = serde_json::json!({
        "text": "done",
        "tool_uses": [],
        "stop_reason": "EndTurn",
        "usage": null
    });
    vec![tool_use.to_string(), end.to_string()]
}

#[test]
#[ignore = "builds the wasm32-wasip2 guest; run with --run-ignored"]
fn ungranted_host_is_denied_at_runtime_through_real_guest() {
    let wasm = build_guest("net-http");

    // Grant net.http to a DIFFERENT host than the cassette fetches.
    let caps = vec![tau_domain::Capability::network_http(
        ["api.anthropic.com"],
        None,
    )];

    let sandbox = tempfile::tempdir().unwrap();
    let (_payload, emitted) = tau_wasm_host::run_component_with_caps(
        &wasm,
        "go",
        cassette(),
        &caps,
        sandbox.path(),
    )
    .expect("run completes: the denial is a tool-result error, not a host trap");

    // The exact wasi:http ErrorCode the EgressPolicy returns before any socket.
    // (#546 lesson: assert the exact code, never a bare `contains("denied")`.)
    assert!(
        emitted.iter().any(|e| e.contains("HttpRequestDenied")),
        "ungranted host must be denied with HttpRequestDenied at the host \
         WasiCtx; emitted events:\n{emitted:#?}"
    );
}

// Keep `Path` used regardless of feature wiring.
const _: fn() = || {
    let _: &dyn Fn(&Path) = &|_p| {};
};
```

> **Implementer note — resolve during Step 3, do not guess:** the exact spelling of (a) the `Capability` net.http constructor (the code above uses a placeholder `Capability::network_http([hosts], methods)`; find the real constructor/variant in `tau-domain` — grep `net.http`/`Network`/`HostSet` and mirror how the fixtures/`resolve_wasi_config` build it) and (b) the `CompletionResponse` field names/`stop_reason` enum spelling (mirror `crates/tau-wasm-host/src/lib.rs`'s `canned_response()` and `tau_ports::llm`). Fix the test to compile against the real types; the assertion (exact `HttpRequestDenied`) is the fixed contract.

- [ ] **Step 3: Make the test compile against real types, then run to verify it fails correctly**

First adjust the `Capability` constructor and cassette JSON to the real signatures (see the implementer note). Then:

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-cli --run-ignored all -E 'test(ungranted_host_is_denied_at_runtime_through_real_guest)'
```
Expected outcome analysis:
- If Task 1 is correct, this test **passes on first green run** — it is an acceptance test proving a distinct property (runtime denial, not binary import). That is acceptable for an acceptance test.
- If it FAILS because no `HttpRequestDenied` appears, diagnose: is the `Fetch` arm reached (native fn name resolved to `"Fetch"`)? Is net.http granted to a *different* host so `blocked.invalid` is denied? Does the guest format the code as `{code:?}` → `HttpRequestDenied`? Fix the mechanism (Task 1) or the cassette, not the assertion.

- [ ] **Step 4: fmt + clippy**

```bash
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt --all --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo clippy -p tau-cli --tests -- -D warnings
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli/tests/wasi_http_roundtrip.rs crates/tau-cli/Cargo.toml
git commit -m "test(epic-3-6): live host-enforcement round-trip (ungranted host denied through real guest)"
```

---

## Task 3: Roadmap tick + PR

**Files:**
- Modify: `docs/superpowers/plans/vision-roadmap.md` (story 3.6)

- [ ] **Step 1: Tick story 3.6 in the roadmap**

Open `docs/superpowers/plans/vision-roadmap.md`, find EPIC 3 story 3.6, and mark it done (match the existing done-marker convention used for 3.2/3.4 — e.g. checkbox ticked / `MERGED` note). Keep the phrasing consistent with 3.4's entry.

- [ ] **Step 2: Full pre-push verification**

```bash
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt --all --check
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-cli --run-ignored all -E 'test(dod_guest_compiles_against_cap_exact_world) + test(ungranted_host_is_denied_at_runtime_through_real_guest)'
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-wasm-host
```
Expected: all green. Confirm no stray `Cargo.lock` under any fixture dir (`git status` clean except intended files).

- [ ] **Step 3: Commit + push + open PR**

```bash
git add docs/superpowers/plans/vision-roadmap.md
git commit -m "docs(epic-3-6): mark story 3.6 done in roadmap"
git push -u origin epic-3-6-guest-effect-abi
gh pr create --base main --title "feat(epic-3-6): guest effect ABI — route net.http through wasi:http" \
  --body "$(cat <<'EOF'
Closes EPIC 3's binary-observable DoD for net.

3.2/3.4 met "ungranted cap un-importable at the ABI" at the world-text +
host-WasiCtx layers only; wasm-ld DCE stripped all WASI imports because the
guest routed no effects through WASI. This routes the guest's net.http effect
through wasi:http via the guest's own `generate_all` bindings, cfg-gated on the
cap-derived world, so:

- a granted net.http import survives DCE (binary-observable — `wit_component::decode`);
- the host WasiCtx/EgressPolicy is the LIVE runtime gate (round-trip test denies an ungranted host with exact `HttpRequestDenied`).

Net-only; fs.read/fs.write deferred to 3.6-b (identical cfg-gate + generated-bindings pattern). Keeps the 3.4 gate-drop sound (no in-guest gate; InGuest caps still gated; `AttenuatedDispatcher`/`tau_native_tools` untouched). No external `wasi` crate (guest owns `cabi_realloc`).

Spec: docs/superpowers/specs/2026-08-10-epic-3-6-guest-effect-abi-design.md
Plan: docs/superpowers/plans/2026-08-10-epic-3-6-guest-effect-abi.md

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
gh pr merge <PR#> --auto   # bare; --squash/-d rejected while queue enabled
```

- [ ] **Step 4: Watch CI; update-branch if BEHIND**

If the PR goes `BEHIND` main (strict protection), `gh pr update-branch <PR#>`. Re-enroll `gh pr merge <PR#> --auto` (bare) if auto-merge drops after any non-required flake.

---

## Self-Review

**Spec coverage:**
- Binary-observable DoD (spec Goal 1) → Task 1 (build.rs cfg + Fetch arm + strengthened DoD assertion). ✓
- Live host enforcement (spec Goal 2) → Task 2 (round-trip, exact `HttpRequestDenied`). ✓
- Generated bindings not external `wasi` crate (D1) → Task 1 Step 4/5 (`wit_wasi` re-export; global constraint forbids the crate). ✓
- Effect arm in `GuestDispatcher` not `tau_native_tools` (D2) → Task 1 Step 5. ✓
- Key on declared native fn name (D3) → `native_fn_name` resolver, `module: Arc<Module>`. ✓
- build.rs cfg gate (D4) → Task 1 Step 3. ✓
- Spike gate → folded into Task 1 (Steps 6–7 are the PASS signal; explicit FAIL→Tier-2 stop). ✓
- Soundness invariants → Global Constraints + Task 1 arm comment (host-enforced, no in-guest gate). ✓
- Non-goals (fs, positive connect, multi-agent) → not implemented; body-read flagged offline-untested. ✓

**Placeholder scan:** No "TBD/TODO" left. The two spec "open items" are resolved: round-trip lives in `tau-cli`; `GuestDispatcher` holds `Arc<Module>`. The one deliberate deferral (exact `Capability` constructor + `CompletionResponse` field spelling) is called out as an explicit implementer note with the resolution method (grep real types), not a vague instruction — its uncertainty is real (types not in this plan's context) and the fixed contract (exact `HttpRequestDenied`) is stated.

**Type consistency:** `GuestDispatcher::new(.., module: Arc<Module>)` used consistently in `guest.rs` and `dispatcher.rs`. `native_fn_name(&ToolId) -> Option<&str>` matched against `Some("Fetch")`. `fetch_via_wasi(&Value) -> Result<Value, String>` returns wired into `ToolInvocationResult { body, error }`. Test helpers (`build_guest`, `cassette`) mirror the proven `build_wasm_world_dod.rs` recipe. `run_component_with_caps` signature copied verbatim from `crates/tau-wasm-host/src/lib.rs:278`.
