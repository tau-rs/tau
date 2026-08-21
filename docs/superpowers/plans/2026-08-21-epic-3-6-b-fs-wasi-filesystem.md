# EPIC 3.6-b — Guest fs effects → wasi:filesystem Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route the production wasm guest's `fs.read`/`fs.write` tool effects through `wasi:filesystem` (preopen → open-at → stream), so a granted fs capability is binary-observable in the compiled component and an ungranted path is denied by the host WasiCtx — mirroring EPIC 3.6 (net, #585).

**Architecture:** A `build.rs`-emitted `tau_cap_fs_read`/`tau_cap_fs_write` cfg pair (set iff the cap-derived world imports `wasi:filesystem`) gates two new `GuestDispatcher::invoke` arms keyed on the declared native fn name (`Read`/`Write`). The arms use the guest's OWN `generate_all` wit-bindgen bindings (no external `wasi` crate) to resolve the requested path against the host's `get-directories()` preopen set — pure descriptor plumbing, no in-guest cap gate — then stream. Enforcement stays 100% host-side (preopen set + `open-at` error-codes + `DirPerms`).

**Tech Stack:** Rust `no_std` wasm32-wasip2 guest, `wit-bindgen generate_all`, `wasmtime-wasi` host, `wit_component::decode` for import assertions, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-08-21-epic-3-6-b-fs-wasi-filesystem-design.md`

## Global Constraints

- **Cargo discipline (CLAUDE.md):** every cargo invocation is `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p <crate>`. Test timeout 300s, build/check 180s, clippy 240s. Use `cargo nextest run` for tests, `--run-ignored all` for the wasm-lane `#[ignore]` tests.
- **No external `wasi` crate** in `tau-wasm-guest` — the guest already exports `cabi_realloc`; a second one is a dup-symbol link error. Use `crate::wit_wasi::filesystem::*` (the `generate_all` bindings).
- **No in-guest capability gate** (3.4 invariant): the Read/Write arms perform zero cap checks. Reachability derives only from host `get-directories()`; access from host `open-at`/stream error-codes. Do NOT strip/normalize `..` in the guest — let the host `open-at` reject escapes.
- **Workspace lints = `-D warnings`**: every `cfg` must be `cargo:rustc-check-cfg`-declared unconditionally.
- **Exact denial marker:** the guest's no-preopen branch returns `Err("FsAccessDenied: …")`; tests assert `contains("FsAccessDenied")`, never a bare `"denied"` (#546 lesson).
- **Guest has no native compile surface:** all `tau-wasm-guest` deps are `[target.'cfg(target_arch="wasm32")'.dependencies]`; the guest is verified by wasm build + host/DoD tests, never native unit tests.

---

### Task 1: fs-read fixture + failing DoD import assertion

Writes the RED acceptance test first: an fs-granting fixture whose compiled component MUST import `wasi:filesystem`. Fails today because the guest never calls `wasi:filesystem`, so wasm-ld DCE-strips the import. Task 2 turns it green.

**Files:**
- Create: `crates/tau-cli/tests/fixtures/wasm-build/fs-read/tau.toml`
- Modify: `crates/tau-cli/tests/build_wasm_world_dod.rs` (add a third `build_and_decode("fs-read")` block + assertions)

**Interfaces:**
- Consumes: `tau_cli::cmd::build_wasm::{lower_to_wasm_ir, wasm_world_for_project}` and the existing `build_and_decode(fixture) -> (String world_text, Vec<String> imports)` helper (already in the test file).
- Produces: the `fs-read` fixture, reused by Task 3's round-trip.

- [ ] **Step 1: Create the fs-read fixture** (mirror `net-http/tau.toml` exactly, swap the tool)

`crates/tau-cli/tests/fixtures/wasm-build/fs-read/tau.toml`:
```toml
packages = ["anthropic"]

[project]
name = "fs-read-wasm"
version = "0.1.0"

[models.claude]
backend = "anthropic"
model = "claude-sonnet-4-6"

[agents.main]
display_name = "Main"
package = "fs-read-wasm@^0.1"
model = "claude"
tool_refs = ["read_file"]

[agents.main.prompt]
system = "Uses an fs.read tool."

[tools.read_file]
native = "Read"
description = "Read a file from a granted path."
capabilities = [{ kind = "fs.read", paths = ["/data/**"] }]
```

- [ ] **Step 2: Add the failing DoD assertion**

In `crates/tau-cli/tests/build_wasm_world_dod.rs`, inside `dod_guest_compiles_against_cap_exact_world`, after the existing `trivial` block and before/after the net-http import assertions, add:
```rust
    // fs-read grants fs.read only → world text has wasi:filesystem, no wasi:http.
    let (fs_world, fs_imports) = build_and_decode("fs-read");
    assert!(
        fs_world.contains("import wasi:filesystem/types@0.2.3;"),
        "fs granted → wasi:filesystem in the compiled-against world:\n{fs_world}"
    );
    assert!(
        !fs_world.contains("wasi:http"),
        "net UNGRANTED → wasi:http absent from the fs world:\n{fs_world}"
    );

    // 3.6-b binary-observable DoD: fs.read granted AND routed through
    // wasi:filesystem (GuestDispatcher's cfg-gated Read arm) → the compiled
    // component's ACTUAL imports now include wasi:filesystem. Vacuous before
    // 3.6-b (DCE stripped every WASI import); now the live proof.
    assert!(
        fs_imports.iter().any(|i| i.starts_with("wasi:filesystem/")),
        "fs granted AND routed → wasi:filesystem MUST be present in the compiled \
         component's actual imports (3.6-b binary-observable DoD): {fs_imports:?}"
    );
```

- [ ] **Step 3: Run the DoD test, verify it FAILS on the import assertion**

Run:
```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-cli --test build_wasm_world_dod --run-ignored all
```
Expected: FAIL at `wasi:filesystem MUST be present in the compiled component's actual imports` (the world-text assertions pass — `wasm_world_for_project` already emits the import — but the compiled component DCE-strips it because no guest code calls it yet).

- [ ] **Step 4: Commit**

```bash
git add crates/tau-cli/tests/fixtures/wasm-build/fs-read/tau.toml \
        crates/tau-cli/tests/build_wasm_world_dod.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "test(epic-3-6-b): fs-read fixture + failing wasi:filesystem import DoD (#596)"
```

---

### Task 2: The mechanism — build.rs cfg gate + dispatcher Read/Write arms

Adds the cfg pair, widens the `wit_wasi` re-export, and implements both fs effect arms. Turns Task 1's DoD green. Done as one task because the re-export widening would produce an unused-import warning (`-D warnings`) if landed without an arm that uses `wasi::filesystem`.

**Files:**
- Modify: `crates/tau-wasm-guest/build.rs` (add the fs cfg block after the net one, ~line 89)
- Modify: `crates/tau-wasm-guest/src/guest.rs:37` (widen `wit_wasi` cfg)
- Modify: `crates/tau-wasm-guest/src/lib.rs:23` (widen the re-export cfg)
- Modify: `crates/tau-wasm-guest/src/dispatcher.rs` (add Read/Write arms + `fs_read_via_wasi`/`fs_write_via_wasi` helpers)

**Interfaces:**
- Consumes: `self.native_fn_name(tool_id) -> Option<&str>` (already present); `crate::wit_wasi::filesystem::{preopens, types}` and `crate::wit_wasi::io` (generated by `generate_all`).
- Produces: `native == "Read"` → `{content, bytes}`; `native == "Write"` → `{bytes}`; both `Err("FsAccessDenied: …")` on a no-preopen path, `Err("<ErrorCode>")` on a host `open-at`/stream failure.

- [ ] **Step 1: Add the fs cfg block to `build.rs`**

After the `tau_cap_net_http` block (currently ending at `crates/tau-wasm-guest/build.rs:92`), add:
```rust
    // 3.6-b: the guest's fs-effect arms (dispatcher.rs) are cfg-gated on whether
    // the capability-derived world grants wasi:filesystem. fs.read and fs.write
    // map to the SAME two interfaces (types + preopens), so the world text can't
    // distinguish them — both cfgs are set together whenever wasi:filesystem is
    // present. Read-vs-write is enforced at RUNTIME by the host preopen perms
    // (DirPerms READ vs all()), not the cfg. check-cfg is unconditional so the
    // guest compiles cleanly on every world (workspace lints are -D warnings).
    println!("cargo:rustc-check-cfg=cfg(tau_cap_fs_read)");
    println!("cargo:rustc-check-cfg=cfg(tau_cap_fs_write)");
    if String::from_utf8_lossy(&world).contains("wasi:filesystem") {
        println!("cargo:rustc-cfg=tau_cap_fs_read");
        println!("cargo:rustc-cfg=tau_cap_fs_write");
    }
```

- [ ] **Step 2: Widen the `wit_wasi` re-export cfg in `guest.rs`**

At `crates/tau-wasm-guest/src/guest.rs:37`, change:
```rust
#[cfg(tau_cap_net_http)]
pub(crate) mod wit_wasi {
```
to:
```rust
#[cfg(any(tau_cap_net_http, tau_cap_fs_read, tau_cap_fs_write))]
pub(crate) mod wit_wasi {
```
(Update the surrounding doc comment to say the re-export is present whenever the world granted `wasi:http` OR `wasi:filesystem`.)

- [ ] **Step 3: Widen the re-export cfg in `lib.rs`**

At `crates/tau-wasm-guest/src/lib.rs:23`, change:
```rust
#[cfg(all(target_arch = "wasm32", tau_cap_net_http))]
pub(crate) use guest::wit_wasi;
```
to:
```rust
#[cfg(all(
    target_arch = "wasm32",
    any(tau_cap_net_http, tau_cap_fs_read, tau_cap_fs_write)
))]
pub(crate) use guest::wit_wasi;
```

- [ ] **Step 4: Add the Read/Write arms to `GuestDispatcher::invoke`**

In `crates/tau-wasm-guest/src/dispatcher.rs`, inside the `Box::pin(async move { … })` block, immediately after the existing `#[cfg(tau_cap_net_http)]` Fetch arm and before `let _ = &native;`, add:
```rust
            // 3.6-b fs effects: a tool declared `native = "Read"`/`"Write"`
            // routes through wasi:filesystem when fs.* was granted (the cfg
            // gate). Enforcement is the HOST preopen set + open-at error-codes
            // (3.3/3.4) — NOT an in-guest gate.
            #[cfg(tau_cap_fs_read)]
            if native.as_deref() == Some("Read") {
                return match fs_read_via_wasi(&args_owned) {
                    Ok(body) => Ok(ToolInvocationResult { body: Some(body), error: None }),
                    Err(msg) => Ok(ToolInvocationResult { body: None, error: Some(msg) }),
                };
            }
            #[cfg(tau_cap_fs_write)]
            if native.as_deref() == Some("Write") {
                return match fs_write_via_wasi(&args_owned) {
                    Ok(body) => Ok(ToolInvocationResult { body: Some(body), error: None }),
                    Err(msg) => Ok(ToolInvocationResult { body: None, error: Some(msg) }),
                };
            }
```

- [ ] **Step 5: Add the fs helpers at the bottom of `dispatcher.rs`**

Append (after `fetch_via_wasi`). The shared preopen-resolution helper is gated on `any(read, write)` so it compiles whenever either arm does:
```rust
/// Resolve `path` against the host's preopen set (`get-directories`) and return
/// the `(preopen-descriptor, relative-path)` to `open-at` from. Pure descriptor
/// plumbing over HOST-provided state — NOT a capability check. `None` means the
/// host granted no preopen containing `path` (absence of capability); the caller
/// surfaces `FsAccessDenied`. Does not touch `..` — the host `open-at` rejects
/// escapes.
#[cfg(any(tau_cap_fs_read, tau_cap_fs_write))]
fn resolve_preopen(
    path: &str,
) -> Option<(crate::wit_wasi::filesystem::types::Descriptor, alloc::string::String)> {
    use crate::wit_wasi::filesystem::preopens::get_directories;
    for (desc, guest_path) in get_directories() {
        // Segment-aware prefix match: `/data` matches `/data` and `/data/x`,
        // but NOT `/dataX`.
        let rel = if path == guest_path {
            "" // the preopen dir itself
        } else if let Some(stripped) = path.strip_prefix(&guest_path) {
            if stripped.starts_with('/') {
                stripped.trim_start_matches('/')
            } else {
                continue;
            }
        } else {
            continue;
        };
        return Some((desc, rel.to_string()));
    }
    None
}

/// `Read`: `{path} → {content, bytes}`. A no-preopen path → `FsAccessDenied`
/// (host granted no descriptor). A host `open-at`/stream failure → `Err(code)`.
/// Never panics.
#[cfg(tau_cap_fs_read)]
fn fs_read_via_wasi(args: &Value) -> Result<Value, alloc::string::String> {
    use crate::wit_wasi::filesystem::types::{DescriptorFlags, OpenFlags, PathFlags};
    use alloc::string::String;
    use alloc::vec::Vec;

    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "Read: missing string arg `path`".to_string())?;

    let (dir, rel) = resolve_preopen(path)
        .ok_or_else(|| format!("FsAccessDenied: no preopen grants {path}"))?;

    let file = dir
        .open_at(PathFlags::SYMLINK_FOLLOW, &rel, OpenFlags::empty(), DescriptorFlags::READ)
        .map_err(|code| format!("{code:?}"))?;
    let stream = file
        .read_via_stream(0)
        .map_err(|code| format!("{code:?}"))?;
    let mut buf: Vec<u8> = Vec::new();
    // Closed / stream error → end of file.
    while let Ok(chunk) = stream.blocking_read(4096) {
        if chunk.is_empty() {
            break;
        }
        buf.extend_from_slice(&chunk);
    }
    let content = String::from_utf8_lossy(&buf).into_owned();
    Ok(serde_json::json!({ "bytes": buf.len(), "content": content }))
}

/// `Write`: `{path, content} → {bytes}`. Requires an fs.write-granted (RW)
/// preopen; a write to a read-only preopen fails at the host `open-at`. Never
/// panics.
#[cfg(tau_cap_fs_write)]
fn fs_write_via_wasi(args: &Value) -> Result<Value, alloc::string::String> {
    use crate::wit_wasi::filesystem::types::{DescriptorFlags, OpenFlags, PathFlags};

    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "Write: missing string arg `path`".to_string())?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| "Write: missing string arg `content`".to_string())?;

    let (dir, rel) = resolve_preopen(path)
        .ok_or_else(|| format!("FsAccessDenied: no preopen grants {path}"))?;

    let file = dir
        .open_at(PathFlags::SYMLINK_FOLLOW, &rel, OpenFlags::CREATE, DescriptorFlags::WRITE)
        .map_err(|code| format!("{code:?}"))?;
    let stream = file
        .write_via_stream(0)
        .map_err(|code| format!("{code:?}"))?;
    // blocking-write-and-flush permits ≤4096 bytes per call; chunk defensively.
    let bytes = content.as_bytes();
    for chunk in bytes.chunks(4096) {
        stream
            .blocking_write_and_flush(chunk)
            .map_err(|e| format!("Write: {e:?}"))?;
    }
    Ok(serde_json::json!({ "bytes": bytes.len() }))
}
```

- [ ] **Step 5b: Baseline compile — cfgs OFF (guest still builds clean against the empty world)**

Run (native check is enough to catch cfg-off syntax/warning issues; the arms compile out):
```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo build -p tau-wasm-guest --target wasm32-wasip2 --release
```
Expected: SUCCESS. (No `TAU_WORLD_WIT` → baseline world → all `tau_cap_*` cfgs off → arms + helpers + `wit_wasi` compiled out, no unused warnings.)

- [ ] **Step 6: Run the Task 1 DoD test, verify it now PASSES**

Run:
```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-cli --test build_wasm_world_dod --run-ignored all
```
Expected: PASS — the fs-read component now imports `wasi:filesystem/*`, and the net-http/trivial assertions still hold.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-wasm-guest/build.rs crates/tau-wasm-guest/src/guest.rs \
        crates/tau-wasm-guest/src/lib.rs crates/tau-wasm-guest/src/dispatcher.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(epic-3-6-b): route guest fs effects through wasi:filesystem (#596)"
```

---

### Task 3: Live denial round-trip through the real guest

Proves an ungranted path is denied at the host WasiCtx through the real production guest. Mirrors `wasi_http_roundtrip.rs` exactly (build guest → grant a DIFFERENT scope → cassette hits the ungranted target → assert the exact denial marker in emitted events).

**Files:**
- Create: `crates/tau-cli/tests/wasi_fs_roundtrip.rs`

**Interfaces:**
- Consumes: `tau_cli::cmd::build_wasm::{lower_to_wasm_ir, wasm_world_for_project}`; `tau_wasm_host::run_component_with_caps(&wasm, prompt, llm_responses, &caps, sandbox_root) -> Result<(payload, Vec<String> emitted), _>`; the `fs-read` fixture from Task 1; the `Read` arm's `FsAccessDenied` marker from Task 2.

- [ ] **Step 1: Write the round-trip test**

`crates/tau-cli/tests/wasi_fs_roundtrip.rs` (the `build_guest` helper is copied verbatim from `wasi_http_roundtrip.rs`; only the fixture name, cassette, caps, and assertion differ):
```rust
//! EPIC 3.6-b live host-enforcement round-trip: the REAL production guest,
//! built by `tau build wasm`, driven by real IR, issues a `Read` at an
//! UNGRANTED path — the host granted no preopen for it, so the guest holds no
//! descriptor and surfaces `FsAccessDenied`. Denial-only (offline; a granted
//! read needs seeded files, covered by the host-side `wasi_fs_enforcement.rs`
//! fs-probe test). Builds the wasm32-wasip2 guest, so it is #[ignore]d.

use std::path::PathBuf;
use std::process::Command;

use tau_cli::cmd::build_wasm::{lower_to_wasm_ir, wasm_world_for_project};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

/// Build the guest for a fixture and return its component bytes. Copied from
/// `wasi_http_roundtrip.rs::build_guest`.
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

/// A cassette turn that calls the `read_file` tool at an ungranted path, then a
/// turn that ends. Field spellings mirror `tau_ports::llm` exactly (verified in
/// `wasi_http_roundtrip.rs`).
fn cassette() -> Vec<String> {
    let tool_use = serde_json::json!({
        "text": "",
        "tool_uses": [{
            "id": "call_1",
            "name": "read_file",
            "input": { "path": "/etc/secret" }
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
fn ungranted_path_is_denied_at_runtime_through_real_guest() {
    let wasm = build_guest("fs-read");

    // Grant fs.read on /data/** → the host preopens <sandbox>/data as guest
    // path "/data". The cassette reads "/etc/secret", for which the host
    // granted NO preopen, so the guest holds no descriptor. Constructed via
    // Capability's Deserialize impl (FsCapability::Read is #[non_exhaustive],
    // same manifest-authoring path used by wasi_http_roundtrip.rs / wasi_map).
    let caps = vec![serde_json::from_str::<tau_domain::Capability>(
        r#"{"kind":"fs.read","paths":["/data/**"]}"#,
    )
    .unwrap()];

    let sandbox = tempfile::tempdir().unwrap();
    let (_payload, emitted) =
        tau_wasm_host::run_component_with_caps(&wasm, "go", cassette(), &caps, sandbox.path())
            .expect("run completes: the denial is a tool-result error, not a host trap");

    // Ungranted-path denial is guest-observed ABSENCE (no host error-code exists
    // by construction — the guest never calls the host for a path it holds no
    // descriptor for), so the marker is the guest's exact `FsAccessDenied`, not
    // net's host `HttpRequestDenied`. See ADR-0066.
    assert!(
        emitted.iter().any(|e| e.contains("FsAccessDenied")),
        "ungranted path must be denied with FsAccessDenied; emitted events:\n{emitted:#?}"
    );
}
```

- [ ] **Step 2: Run the round-trip, verify it PASSES**

Run:
```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-cli --test wasi_fs_roundtrip --run-ignored all
```
Expected: PASS — `emitted` contains `FsAccessDenied`. If it fails with a build error about the cassette field spellings, cross-check against `wasi_http_roundtrip.rs::cassette` (the canned-response schema is verified there).

- [ ] **Step 3: Commit**

```bash
git add crates/tau-cli/tests/wasi_fs_roundtrip.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "test(epic-3-6-b): live fs denial round-trip through real guest (#596)"
```

---

### Task 4: ADR-0066, docs, #597 clippy pre-flight

Pins the three soundness decisions, wires the ADR into the book, ticks the roadmap, and runs the manual cfg-ON clippy that #597's standing job doesn't yet cover.

**Files:**
- Create: `docs/decisions/0066-guest-fs-effect-descriptor-resolution.md`
- Modify: `docs/SUMMARY.md` (add the ADR-0066 line after ADR-0065)
- Modify: the EPIC-3 roadmap file (the line reading `net shipped; fs → 3.6-b`)

**Interfaces:** none (docs).

- [ ] **Step 1: Locate the roadmap line**

Run:
```bash
grep -rn "fs → 3.6-b\|net shipped; fs" docs/
```
Note the file:line for Step 4.

- [ ] **Step 2: Write ADR-0066** (`docs/decisions/0066-guest-fs-effect-descriptor-resolution.md`)

```markdown
# ADR-0066: Guest fs-effect descriptor resolution — preopen plumbing, dual cfg, absence-denial

**Status:** Accepted
**Date:** 2026-08-21
**Deciders:** tau core

## Context

EPIC 3.6-b routes the wasm guest's `fs.read`/`fs.write` tool effects through
`wasi:filesystem`, mirroring 3.6's net mechanism. Unlike net (a single host
call), fs requires a descriptor ceremony — the guest holds no ambient root on
wasip2 and must resolve a requested path against a host-preopened directory
before opening anything. Three sub-decisions fall out of that ceremony.

## Decision

**D2 — Both fs cfgs gate on the single `wasi:filesystem` world import.**
`fs.read` and `fs.write` map to the same two interfaces (`wasi:filesystem/types`
+ `/preopens`); the world text cannot distinguish them. `build.rs` therefore
sets both `tau_cap_fs_read` and `tau_cap_fs_write` whenever `wasi:filesystem` is
present. Read-vs-write is enforced at runtime by the host preopen perms
(`DirPerms::READ` vs `DirPerms::all()` from `PreopenAccess`), not the cfg — a
write to a read-only preopen fails at the host `open-at`. The two-cfg naming is
kept for per-effect symmetry and future interface divergence.

**D3 — Preopen-relative resolution is descriptor plumbing, not a cap gate.**
The guest computes reachability solely from the host's `get-directories()` list.
A matching preopen → strip the guest-path prefix, pass the remainder straight to
the host `open-at` (the guest does NOT reject/normalize `..`; the host rejects
escapes at the descriptor boundary). No matching preopen → the guest holds no
descriptor and cannot fabricate one. This is WASI's capability-security model:
absence of a preopen is absence of capability. The enforcement point is the
host's preopen set (its `WasiCtx`), which the host populates only from granted
caps — preserving 3.4's "no in-guest cap gate" invariant.

**D4 — Ungranted-path denial is guest-observed absence, not a host error-code.**
Net's denial is a host hook returning a `wasi:http` error-code
(`HttpRequestDenied`), asserted exactly. Fs's no-preopen denial produces no host
error-code by construction — the guest never calls the host for a path it holds
no descriptor for. So the round-trip asserts an exact, stable guest-authored
marker (`FsAccessDenied`). The enforcement stays 100% host (what the host placed
in `get-directories`); only the marker string is guest-emitted.

## Consequences

- A future divergence of the read/write WIT surface would let D2 split the cfgs;
  until then they move in lockstep.
- Tests asserting fs denial key on `FsAccessDenied` (guest constant), whereas
  net keys on the host `HttpRequestDenied` — an intentional, documented
  asymmetry, not an inconsistency.
- The positive/connected fs path is offline-untested by design (as net); the
  host-side `wasi_fs_enforcement.rs` fs-probe test covers the granted read.
```

- [ ] **Step 3: Add the ADR to `docs/SUMMARY.md`**

After the `ADR-0065` line (`docs/SUMMARY.md:147`), add:
```markdown
- [ADR-0066 — Guest fs-effect descriptor resolution: preopen plumbing, dual cfg, absence-denial](decisions/0066-guest-fs-effect-descriptor-resolution.md)
```

- [ ] **Step 4: Tick the roadmap line**

At the file:line found in Step 1, update the `net shipped; fs → 3.6-b` marker to reflect fs shipped (e.g. `✅ (net + fs shipped)`). Match the surrounding formatting exactly.

- [ ] **Step 5: Build the book locally (docs gate — DOCS RULES)**

```bash
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd ..
rm -rf docs/book
```
Expected: only `[INFO]` lines, no linkcheck errors.

- [ ] **Step 6: Manual cfg-ON clippy of the fs arm (#597 blind spot)**

The standing clippy jobs build `tau-wasm-guest` against the baseline world (fs cfgs OFF), so the fs arms are never linted. Generate an fs-granting world and clippy against it under `-D warnings`:
```bash
# Emit the fs-read fixture's cap-derived world to a temp file via a tiny helper
# run, OR reuse the DoD path: write wasm_world_for_project("fs-read") output to
# /tmp/fs-world.wit, then:
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  TAU_WORLD_WIT=/tmp/fs-world.wit \
  cargo clippy -p tau-wasm-guest --target wasm32-wasip2 --release -- -D warnings
```
To produce `/tmp/fs-world.wit`, add a throwaway `#[test]` that writes
`wasm_world_for_project(&fixture("fs-read")).unwrap()` to that path, run it, then
delete the test — or copy the world text the DoD test already computes. Expected:
clean (no warnings). Fix any lint inline in `dispatcher.rs` and re-run before
committing. Note the run in the PR body so #597 can fold the fs world into its
standing job.

- [ ] **Step 7: Commit**

```bash
git add docs/decisions/0066-guest-fs-effect-descriptor-resolution.md docs/SUMMARY.md docs/<roadmap-file>
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "docs(epic-3-6-b): ADR-0066 + roadmap tick for fs → wasi:filesystem (#596)"
```

---

### Task 5: Full verification + PR

**Files:** none (verification + PR).

- [ ] **Step 1: fmt + clippy (native) + the full wasm lane**

```bash
timeout 30  env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-wasm-guest -p tau-cli --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli --all-targets -- -D warnings
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-cli --test build_wasm_world_dod --test wasi_fs_roundtrip --run-ignored all
```
Expected: all green. (fmt covers both crates; the native clippy covers the cfg-OFF guest via tau-cli's dep; the wasm-lane run covers both DoD + round-trip.)

- [ ] **Step 2: Re-run the net round-trip as a no-regression check**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo nextest run -p tau-cli --test wasi_http_roundtrip --run-ignored all
```
Expected: PASS (the `wit_wasi` cfg widening must not have disturbed net).

- [ ] **Step 3: Push + open PR**

```bash
git push -u origin epic-3-6-b-fs-wasi-filesystem
gh pr create --base main --title "feat(epic-3-6-b): route guest fs effects through wasi:filesystem (#596)" \
  --body "Closes #596. Mirrors 3.6 (net, #585) for fs.read/fs.write: cfg-gated Read/Write arms route through wasi:filesystem via the guest's own generate_all bindings; preopen→open-at→stream ceremony; host-enforced (no in-guest gate). DoD: positive wasi:filesystem import assertion + live ungranted-path denial round-trip. ADR-0066 pins the descriptor-resolution decisions. Ran the #597 manual cfg-ON clippy against the fs world (clean) — #597 should fold the fs world into its standing job."
```

- [ ] **Step 4: Enroll auto-merge**

```bash
gh pr merge <PR#> --squash --delete-branch --auto
```

## Self-Review

- **Spec coverage:** D1 → Task 2 (build.rs + arms); D2 → Task 2 Step 1 + ADR; D3 → Task 2 Step 5 (`resolve_preopen`) + ADR; D4 → Task 3 + ADR; DoD.1 → Task 1; DoD.2 → Task 3; DoD.3 → Task 4 Step 6; DoD.4 roadmap → Task 4 Step 4; ADR (D5) → Task 4. All covered.
- **Placeholder scan:** the only "fill-in" is Task 4 Step 4's roadmap line + Step 6's `/tmp/fs-world.wit` generation, both with explicit discovery commands and concrete alternatives. No bare TODOs.
- **Type consistency:** `resolve_preopen(&str) -> Option<(Descriptor, String)>` used identically in both helpers; `FsAccessDenied` marker matches between Task 2 (emit) and Task 3 (assert); `run_component_with_caps` signature matches `wasi_http_roundtrip.rs`; tool-ref key `read_file` matches between fixture (Task 1) and cassette (Task 3); native fn name `Read`/`Write` matches between fixture and dispatcher arms.
