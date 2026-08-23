# EPIC 3.1 — Capability → WASI/WIT mapping table Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a pure, total `map_capability` function plus its data types to `tau-ports::target` that lowers each tau `Capability` to its WASI/WIT realization (imports + config + disposition) on the wasm target.

**Architecture:** A single new `no_std` module `crates/tau-ports/src/target/wasi_map.rs`. It reads `tau_domain` capability types read-only and returns a `WasiMapping { imports, config, disposition }` value object. No WIT-world generation, no `WasiCtx` build, no gate change — those are downstream stories 3.2/3.3/3.4. Re-exported from `target/mod.rs`.

**Tech Stack:** Rust `no_std` + `alloc`, `tau-domain` (capability types), inline `#[cfg(test)] mod tests`, `cargo nextest`.

## Global Constraints

- **Design doc (source of truth):** `docs/superpowers/specs/2026-07-23-epic-3-1-cap-wit-table-design.md`. The table in that spec is authoritative; this plan implements it verbatim.
- **Crate:** only `crates/tau-ports`. Do **not** touch `tau-pkg` or any other crate.
- **`no_std`:** the crate is `#![no_std]`; use `alloc::{vec::Vec, string::String, collections::BTreeSet}`. `tau-ports/src/lib.rs` already declares `extern crate alloc;`. `std` is only available under `#[cfg(test)]`.
- **Lints:** `#![forbid(unsafe_code)]` (workspace lint) and `#![deny(missing_docs)]` (crate) are in force — **every** `pub` item needs a `///` doc comment or the build fails.
- **`thiserror`:** NOT introduced. `map_capability` is total (returns for every input); there is no fallible boundary.
- **WASI version pin:** `WASI_VERSION = "0.2.3"` (wasip2, wasmtime-45).
- **Network mapping:** `net.http` → `wasi:http/{types,outgoing-handler}` only. Raw `wasi:sockets` is NOT a table entry (see spec "Network mapping decision").
- **Fail-closed:** `#[non_exhaustive]` catch-all arms map to `Disposition::HostMediated`, never `Wasi`.
- **CARGO RULES (CLAUDE.md) — every cargo command uses this exact shape:**
  - Tests: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e31 cargo nextest run -p tau-ports`
  - Filter one test: append the test-name substring, e.g. `... cargo nextest run -p tau-ports wasi_map::tests::package_id`
  - Doctests: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e31 cargo test -p tau-ports --doc`
  - Never run bare `cargo`, never omit `-p`, always wrap in `timeout`.

## Type / import reference (verified against the tree)

All re-exported at `tau_domain::` crate root:

```rust
use tau_domain::{
    Capability,        // enum: Filesystem(FsCapability) | Network(NetCapability)
                       //     | Process(ProcessCapability) | Agent(AgentCapability)
                       //     | Skill(SkillCapability) | TaskList { mode: String }
                       //     | Plan { mode: String } | Custom { name, params }
    FsCapability,      // Read { paths: Vec<String> }
                       //     | Write { paths: Vec<String>, max_bytes: Option<u64> }
                       //     | Exec { paths: Vec<String> }
    NetCapability,     // Http { hosts: HostSet, methods: Option<BTreeSet<HttpMethod>> }
    ProcessCapability, // Spawn { commands: Vec<String> }
    AgentCapability,   // Spawn { allowed_kinds: Vec<String> }
    SkillCapability,   // Spawn { allowed_skills: Vec<String> }
    HostSet,           // Any | Exact(BTreeSet<HostName>)
    HostName,          // HostName::parse(&str) -> Result<HostName, HostNameError>
    HttpMethod,        // Get | Head | Post | ...
};
```

All capability enums and their inner variants are `#[non_exhaustive]` → every `match` needs a catch-all arm.

## File structure

- Create: `crates/tau-ports/src/target/wasi_map.rs` — the whole module (types + `map_capability` + inline tests).
- Modify: `crates/tau-ports/src/target/mod.rs` — add `pub mod wasi_map;` and a `pub use` re-export line.

---

### Task 1: `WitInterface` enum + `package_id()` + `WASI_VERSION`

The leaf data: the four WASI interfaces this table references, each with its fully-qualified WIT package id, and the version constant that guards drift.

**Files:**
- Create: `crates/tau-ports/src/target/wasi_map.rs`
- Test: same file, inline `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing (leaf).
- Produces:
  - `pub const WASI_VERSION: &str = "0.2.3";`
  - `pub enum WitInterface { WasiHttpTypes, WasiHttpOutgoingHandler, WasiFilesystemTypes, WasiFilesystemPreopens }` (`#[non_exhaustive]`, derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `impl WitInterface { pub fn package_id(&self) -> &'static str }`

- [ ] **Step 1: Write the failing test**

Create `crates/tau-ports/src/target/wasi_map.rs` with only the module doc, the test module, and nothing else yet:

```rust
//! Capability → WASI/WIT mapping table for the wasm target (EPIC 3.1).
//!
//! [`map_capability`] lowers one [`tau_domain::Capability`] to its WASI/WIT
//! realization: the WIT interface [`WitInterface`] imports the generated world
//! must declare (3.2), the [`WasiConfig`] fragment the host `WasiCtx` consumes
//! (3.3), and the [`Disposition`] that says how the capability is satisfied on
//! wasm (3.4). Pure, total, and read-only over `tau_domain`.
//!
//! See `docs/superpowers/specs/2026-07-23-epic-3-1-cap-wit-table-design.md`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_id_is_fully_qualified_and_version_pinned() {
        let all = [
            WitInterface::WasiHttpTypes,
            WitInterface::WasiHttpOutgoingHandler,
            WitInterface::WasiFilesystemTypes,
            WitInterface::WasiFilesystemPreopens,
        ];
        for iface in all {
            let id = iface.package_id();
            assert!(id.starts_with("wasi:"), "not fully qualified: {id}");
            assert!(
                id.ends_with(&alloc::format!("@{WASI_VERSION}")),
                "version drift: {id} != @{WASI_VERSION}"
            );
        }
        assert_eq!(
            WitInterface::WasiHttpOutgoingHandler.package_id(),
            "wasi:http/outgoing-handler@0.2.3"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e31 cargo nextest run -p tau-ports wasi_map`

But first the module must be declared or it won't compile. Add to `crates/tau-ports/src/target/mod.rs` after the other `pub mod` lines:

```rust
pub mod wasi_map;
```

Expected: compile error — `WitInterface` / `WASI_VERSION` not found.

- [ ] **Step 3: Write minimal implementation**

Insert above the `#[cfg(test)]` block in `wasi_map.rs`:

```rust
extern crate alloc;

/// WASI preview-2 version this table pins (wasip2, wasmtime-45, β.7.5).
pub const WASI_VERSION: &str = "0.2.3";

/// The WASI interfaces this table references. [`WitInterface::package_id`]
/// returns the fully-qualified WIT package id, e.g.
/// `"wasi:http/outgoing-handler@0.2.3"`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WitInterface {
    /// `wasi:http/types` — HTTP request/response value types.
    WasiHttpTypes,
    /// `wasi:http/outgoing-handler` — outbound HTTP; carries the host allow-list.
    WasiHttpOutgoingHandler,
    /// `wasi:filesystem/types` — filesystem descriptors and operations.
    WasiFilesystemTypes,
    /// `wasi:filesystem/preopens` — the set of preopened directories.
    WasiFilesystemPreopens,
}

impl WitInterface {
    /// Fully-qualified WIT package id (interface path + `@` + [`WASI_VERSION`]).
    pub fn package_id(&self) -> &'static str {
        match self {
            WitInterface::WasiHttpTypes => "wasi:http/types@0.2.3",
            WitInterface::WasiHttpOutgoingHandler => "wasi:http/outgoing-handler@0.2.3",
            WitInterface::WasiFilesystemTypes => "wasi:filesystem/types@0.2.3",
            WitInterface::WasiFilesystemPreopens => "wasi:filesystem/preopens@0.2.3",
        }
    }
}
```

Note: `package_id` returns `&'static str` literals with the version baked in (not `format!`), so the test's `ends_with(@{WASI_VERSION})` is the guard that keeps the literals and the const in lockstep.

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e31 cargo nextest run -p tau-ports wasi_map`
Expected: PASS (`package_id_is_fully_qualified_and_version_pinned`).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ports/src/target/wasi_map.rs crates/tau-ports/src/target/mod.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-ports): WitInterface + package_id for cap→WASI table (3.1)"
```

---

### Task 2: Mapping value types + the three `Wasi` rows (network, fs.read, fs.write)

The `WasiMapping` value object, its `Disposition`/`WasiConfig`/`Preopen`/`PreopenAccess` companions, and `map_capability` covering the three capabilities that bind to a WASI import: `net.http`, `fs.read`, `fs.write`.

**Files:**
- Modify: `crates/tau-ports/src/target/wasi_map.rs`
- Test: same file

**Interfaces:**
- Consumes: `WitInterface` (Task 1); `tau_domain::{Capability, FsCapability, NetCapability, HostSet, HttpMethod}`.
- Produces:
  - `pub fn map_capability(cap: &Capability) -> WasiMapping`
  - `pub struct WasiMapping { pub imports: Vec<WitInterface>, pub config: WasiConfig, pub disposition: Disposition }`
  - `pub enum Disposition { Wasi, InGuest, HostMediated, Unsupported { reason: &'static str } }` (`#[non_exhaustive]`)
  - `pub enum WasiConfig { None, AllowedHosts { hosts: HostSet, methods: Option<BTreeSet<HttpMethod>> }, Preopens(Vec<Preopen>) }` (`#[non_exhaustive]`)
  - `pub struct Preopen { pub paths: Vec<String>, pub access: PreopenAccess }`
  - `pub enum PreopenAccess { ReadOnly, ReadWrite }`

- [ ] **Step 1: Write the failing test**

Add these tests inside the existing `mod tests`:

```rust
    use tau_domain::{
        Capability, FsCapability, HostName, HostSet, HttpMethod, NetCapability,
    };
    use alloc::collections::BTreeSet;
    use alloc::vec;

    fn exact(hosts: &[&str]) -> HostSet {
        HostSet::Exact(hosts.iter().map(|h| HostName::parse(h).unwrap()).collect())
    }

    #[test]
    fn net_http_maps_to_wasi_http_with_hosts_and_methods_verbatim() {
        let mut methods = BTreeSet::new();
        methods.insert(HttpMethod::Post);
        let cap = Capability::Network(NetCapability::Http {
            hosts: exact(&["api.anthropic.com"]),
            methods: Some(methods.clone()),
        });

        let m = map_capability(&cap);

        assert!(matches!(m.disposition, Disposition::Wasi));
        assert_eq!(
            m.imports,
            vec![WitInterface::WasiHttpTypes, WitInterface::WasiHttpOutgoingHandler]
        );
        match m.config {
            WasiConfig::AllowedHosts { hosts, methods: got } => {
                assert_eq!(hosts, exact(&["api.anthropic.com"]));
                assert_eq!(got, Some(methods));
            }
            other => panic!("expected AllowedHosts, got {other:?}"),
        }
    }

    #[test]
    fn net_http_any_and_all_methods_pass_through_unchanged() {
        let cap = Capability::Network(NetCapability::Http {
            hosts: HostSet::Any,
            methods: None,
        });
        match map_capability(&cap).config {
            WasiConfig::AllowedHosts { hosts, methods } => {
                assert!(hosts.is_any());
                assert_eq!(methods, None);
            }
            other => panic!("expected AllowedHosts, got {other:?}"),
        }
    }

    #[test]
    fn fs_read_maps_to_readonly_preopen_with_paths_verbatim() {
        let cap = Capability::Filesystem(FsCapability::Read {
            paths: vec!["/data/**".into()],
        });
        let m = map_capability(&cap);
        assert!(matches!(m.disposition, Disposition::Wasi));
        assert_eq!(
            m.imports,
            vec![WitInterface::WasiFilesystemTypes, WitInterface::WasiFilesystemPreopens]
        );
        match m.config {
            WasiConfig::Preopens(p) => {
                assert_eq!(p.len(), 1);
                assert_eq!(p[0].paths, vec!["/data/**".to_string()]);
                assert!(matches!(p[0].access, PreopenAccess::ReadOnly));
            }
            other => panic!("expected Preopens, got {other:?}"),
        }
    }

    #[test]
    fn fs_write_maps_to_readwrite_preopen() {
        let cap = Capability::Filesystem(FsCapability::Write {
            paths: vec!["/out".into()],
            max_bytes: None,
        });
        match map_capability(&cap).config {
            WasiConfig::Preopens(p) => {
                assert_eq!(p[0].paths, vec!["/out".to_string()]);
                assert!(matches!(p[0].access, PreopenAccess::ReadWrite));
            }
            other => panic!("expected Preopens, got {other:?}"),
        }
    }
```

Note: `Disposition` and `WasiConfig` need `Debug` derives so the `panic!("... {other:?}")` arms compile.

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e31 cargo nextest run -p tau-ports wasi_map`
Expected: compile error — `map_capability`, `WasiMapping`, `Disposition`, `WasiConfig`, `Preopen`, `PreopenAccess` not found.

- [ ] **Step 3: Write minimal implementation**

Add imports at the top of `wasi_map.rs` (below `extern crate alloc;`):

```rust
use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use tau_domain::{Capability, FsCapability, HostSet, HttpMethod, NetCapability};
```

Add the types and function (place above the `#[cfg(test)]` block):

```rust
/// How a capability is satisfied on the wasm target.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    /// Bounded by a WASI import + config: network, fs.read, fs.write.
    Wasi,
    /// Enforced in-guest by the tau runtime; no WASI surface
    /// (taskllist, plan, agent.spawn, skill.spawn).
    InGuest,
    /// Requires host mediation outside the WASI ABI; out of scope for wasm
    /// capability gating (hardware / generic `Custom`).
    HostMediated,
    /// Cannot be expressed on the wasm target (fs.exec, process.spawn).
    Unsupported {
        /// Human-readable reason, surfaced by 3.2/3.4 diagnostics.
        reason: &'static str,
    },
}

/// Preopen access mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreopenAccess {
    /// fs.read → read-only preopen.
    ReadOnly,
    /// fs.write → read-write preopen.
    ReadWrite,
}

/// A single preopen derived from an fs capability. Glob → directory
/// resolution is deferred to story 3.3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preopen {
    /// Glob patterns copied verbatim from the fs capability.
    pub paths: Vec<String>,
    /// Read-only (fs.read) or read-write (fs.write).
    pub access: PreopenAccess,
}

/// Runtime configuration a capability contributes to the host `WasiCtx` (3.3).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WasiConfig {
    /// No runtime config (all non-`Wasi` dispositions).
    None,
    /// Network egress filter. `hosts` reuses D4-B [`HostSet`] semantics
    /// (exact | typed `Any`); `methods == None` means all methods.
    AllowedHosts {
        /// Allowed hostnames, copied verbatim from the capability.
        hosts: HostSet,
        /// Allowed HTTP methods; `None` = all.
        methods: Option<BTreeSet<HttpMethod>>,
    },
    /// Filesystem preopens derived from the capability's glob paths.
    Preopens(Vec<Preopen>),
}

/// The WASI/WIT realization of a single capability on the wasm target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasiMapping {
    /// WIT interface imports the generated world must declare (3.2). Empty
    /// unless `disposition == Disposition::Wasi`.
    pub imports: Vec<WitInterface>,
    /// Runtime config fragment this capability contributes to `WasiCtx` (3.3).
    pub config: WasiConfig,
    /// How this capability is satisfied on the wasm target.
    pub disposition: Disposition,
}

/// Lower one tau [`Capability`] to its WASI/WIT realization on the wasm target.
///
/// Total and pure: every capability yields a [`WasiMapping`]. Capabilities
/// that bind to a WASI import return `Disposition::Wasi` with non-empty
/// `imports`; all others carry empty `imports` and `WasiConfig::None`.
///
/// # Example
///
/// ```
/// use tau_ports::target::wasi_map::{map_capability, Disposition};
/// use tau_domain::{Capability, FsCapability};
///
/// let cap = Capability::Filesystem(FsCapability::Read { paths: vec!["/d".into()] });
/// assert!(matches!(map_capability(&cap).disposition, Disposition::Wasi));
/// ```
pub fn map_capability(cap: &Capability) -> WasiMapping {
    match cap {
        Capability::Network(NetCapability::Http { hosts, methods }) => WasiMapping {
            imports: vec![
                WitInterface::WasiHttpTypes,
                WitInterface::WasiHttpOutgoingHandler,
            ],
            config: WasiConfig::AllowedHosts {
                hosts: hosts.clone(),
                methods: methods.clone(),
            },
            disposition: Disposition::Wasi,
        },
        Capability::Filesystem(FsCapability::Read { paths }) => {
            fs_preopen(paths.clone(), PreopenAccess::ReadOnly)
        }
        Capability::Filesystem(FsCapability::Write { paths, .. }) => {
            fs_preopen(paths.clone(), PreopenAccess::ReadWrite)
        }
        // Remaining arms added in Task 3.
        _ => WasiMapping {
            imports: Vec::new(),
            config: WasiConfig::None,
            disposition: Disposition::HostMediated,
        },
    }
}

/// Build a filesystem `Wasi` mapping (shared by fs.read / fs.write).
fn fs_preopen(paths: Vec<String>, access: PreopenAccess) -> WasiMapping {
    WasiMapping {
        imports: vec![
            WitInterface::WasiFilesystemTypes,
            WitInterface::WasiFilesystemPreopens,
        ],
        config: WasiConfig::Preopens(vec![Preopen { paths, access }]),
        disposition: Disposition::Wasi,
    }
}
```

The `_ =>` arm is a temporary stand-in that Task 3 replaces with explicit arms + the fail-closed catch-all.

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e31 cargo nextest run -p tau-ports wasi_map`
Expected: PASS — all four new tests plus Task 1's test green.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ports/src/target/wasi_map.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-ports): map network + fs caps to WASI imports (3.1)"
```

---

### Task 3: Remaining arms — `Unsupported`, `InGuest`, `HostMediated` + fail-closed catch-all

Replace the temporary `_ =>` arm with explicit arms for fs.exec, process.spawn (Unsupported), agent/skill/taskllist/plan (InGuest), Custom (HostMediated), and a fail-closed catch-all for future `#[non_exhaustive]` variants.

**Files:**
- Modify: `crates/tau-ports/src/target/wasi_map.rs`
- Test: same file

**Interfaces:**
- Consumes: everything from Tasks 1–2; adds `tau_domain::{ProcessCapability, AgentCapability, SkillCapability}` to the imports.
- Produces: no new public items — completes `map_capability`'s coverage.

- [ ] **Step 1: Write the failing test**

Add inside `mod tests`:

```rust
    use tau_domain::{AgentCapability, ProcessCapability, SkillCapability};
    use alloc::collections::BTreeMap;

    #[test]
    fn fs_exec_is_unsupported() {
        let cap = Capability::Filesystem(FsCapability::Exec { paths: vec!["/bin/x".into()] });
        let m = map_capability(&cap);
        assert!(m.imports.is_empty());
        assert!(matches!(m.config, WasiConfig::None));
        match m.disposition {
            Disposition::Unsupported { reason } => assert!(!reason.is_empty()),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn process_spawn_is_unsupported() {
        let cap = Capability::Process(ProcessCapability::Spawn { commands: vec!["ls".into()] });
        assert!(matches!(
            map_capability(&cap).disposition,
            Disposition::Unsupported { .. }
        ));
    }

    #[test]
    fn agent_and_skill_spawn_are_in_guest() {
        let agent = Capability::Agent(AgentCapability::Spawn { allowed_kinds: vec!["worker".into()] });
        let skill = Capability::Skill(SkillCapability::Spawn { allowed_skills: vec!["fmt".into()] });
        for cap in [agent, skill] {
            let m = map_capability(&cap);
            assert!(m.imports.is_empty());
            assert!(matches!(m.config, WasiConfig::None));
            assert!(matches!(m.disposition, Disposition::InGuest));
        }
    }

    #[test]
    fn tasklist_and_plan_are_in_guest() {
        let tasks = Capability::TaskList { mode: "read".into() };
        let plan = Capability::Plan { mode: "write".into() };
        for cap in [tasks, plan] {
            assert!(matches!(map_capability(&cap).disposition, Disposition::InGuest));
        }
    }

    #[test]
    fn custom_is_host_mediated() {
        let cap = Capability::Custom { name: "hw.fan".into(), params: BTreeMap::new() };
        let m = map_capability(&cap);
        assert!(m.imports.is_empty());
        assert!(matches!(m.disposition, Disposition::HostMediated));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e31 cargo nextest run -p tau-ports wasi_map`
Expected: `agent_and_skill_spawn_are_in_guest`, `tasklist_and_plan_are_in_guest` FAIL (temporary arm returns `HostMediated`, not `InGuest`); `fs_exec_is_unsupported` / `process_spawn_is_unsupported` FAIL (returns `HostMediated`, not `Unsupported`). `custom_is_host_mediated` passes by accident against the stand-in — it is locked in by this task's explicit arm.

- [ ] **Step 3: Write minimal implementation**

Extend the `use tau_domain::{...}` line to add `AgentCapability, ProcessCapability, SkillCapability`:

```rust
use tau_domain::{
    AgentCapability, Capability, FsCapability, HostSet, HttpMethod, NetCapability,
    ProcessCapability, SkillCapability,
};
```

Replace the temporary `_ =>` arm in `map_capability` with these explicit arms followed by the fail-closed catch-all:

```rust
        Capability::Filesystem(FsCapability::Exec { .. }) => WasiMapping {
            imports: Vec::new(),
            config: WasiConfig::None,
            disposition: Disposition::Unsupported {
                reason: "wasm target has no exec surface",
            },
        },
        Capability::Process(ProcessCapability::Spawn { .. }) => WasiMapping {
            imports: Vec::new(),
            config: WasiConfig::None,
            disposition: Disposition::Unsupported {
                reason: "wasm target cannot spawn OS processes",
            },
        },
        Capability::Agent(AgentCapability::Spawn { .. })
        | Capability::Skill(SkillCapability::Spawn { .. })
        | Capability::TaskList { .. }
        | Capability::Plan { .. } => WasiMapping {
            imports: Vec::new(),
            config: WasiConfig::None,
            disposition: Disposition::InGuest,
        },
        Capability::Custom { .. } => WasiMapping {
            imports: Vec::new(),
            config: WasiConfig::None,
            disposition: Disposition::HostMediated,
        },
        // Fail-closed: an unknown future capability (or future FsCapability /
        // NetCapability / … variant, all `#[non_exhaustive]`) is NOT granted a
        // WASI import. It maps to HostMediated so it can never silently reach
        // the guest's WASI ABI.
        _ => WasiMapping {
            imports: Vec::new(),
            config: WasiConfig::None,
            disposition: Disposition::HostMediated,
        },
```

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e31 cargo nextest run -p tau-ports wasi_map`
Expected: PASS — all Task 1/2/3 tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ports/src/target/wasi_map.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-ports): complete cap→WASI table (unsupported/in-guest/custom) (3.1)"
```

---

### Task 4: Re-export from `target/mod.rs` + full-crate verification

Expose the public surface at `tau_ports::target::{...}` for downstream 3.2/3.3, and run the whole crate (tests + doctests + clippy) to confirm nothing regressed and the `missing_docs`/`forbid(unsafe_code)` lints pass.

**Files:**
- Modify: `crates/tau-ports/src/target/mod.rs`

**Interfaces:**
- Consumes: the whole `wasi_map` module.
- Produces: re-exports `pub use wasi_map::{Disposition, Preopen, PreopenAccess, WasiConfig, WasiMapping, WitInterface, map_capability, WASI_VERSION};`

- [ ] **Step 1: Add the re-export**

In `crates/tau-ports/src/target/mod.rs`, the `pub mod wasi_map;` line was added in Task 1. Add this re-export alongside the existing `pub use` lines:

```rust
pub use wasi_map::{
    map_capability, Disposition, Preopen, PreopenAccess, WasiConfig, WasiMapping, WitInterface,
    WASI_VERSION,
};
```

- [ ] **Step 2: Verify the re-export compiles + full crate tests pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e31 cargo nextest run -p tau-ports`
Expected: PASS — the whole tau-ports suite, including the new `wasi_map::tests::*`.

- [ ] **Step 3: Run doctests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e31 cargo test -p tau-ports --doc`
Expected: PASS — the `map_capability` doctest compiles and runs. (If the doctest's `use tau_ports::target::wasi_map::...` path is wrong, fix it to match the re-export and re-run.)

- [ ] **Step 4: Run clippy (guards `missing_docs`, `forbid(unsafe_code)`, workspace lints)**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-e31 cargo clippy -p tau-ports --all-targets -- -D warnings`
Expected: no warnings, no errors.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-ports/src/target/mod.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-ports): re-export cap→WASI mapping table from target (3.1)"
```

---

## Self-review (done during planning)

**Spec coverage:**
- Network → `wasi:http` + allowed-hosts (HostSet) → Task 2. ✓
- fs → preopens (read-only / read-write) → Task 2. ✓
- hardware → host-mediated/out-of-scope (`Custom` → HostMediated) → Task 3. ✓
- Unsupported (fs.exec, process.spawn) → Task 3. ✓
- InGuest (agent/skill/taskllist/plan) → Task 3. ✓
- Fail-closed `#[non_exhaustive]` catch-all → Task 3. ✓
- `WitInterface::package_id` + `WASI_VERSION` pin → Task 1. ✓
- Total function, no `thiserror` → Tasks 2–3 (no `Result`). ✓
- Table exists + unit tests (acceptance) → Tasks 1–3 tests. ✓
- Re-export / public surface for downstream → Task 4. ✓

**Placeholder scan:** none — every step carries complete code or an exact command.

**Type consistency:** `map_capability`, `WasiMapping { imports, config, disposition }`, `Disposition`, `WasiConfig`, `Preopen { paths, access }`, `PreopenAccess`, `WitInterface`, `WASI_VERSION` are used identically across Tasks 1–4 and match the design doc's API section.

## Downstream (NOT in this plan)

3.2 (WIT world gen), 3.3 (WasiCtx config), 3.4 (drop guest gate), 3.5 (reproducibility) each consume this table and are separate stories. Do not implement them here.
