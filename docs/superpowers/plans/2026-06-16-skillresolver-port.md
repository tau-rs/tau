# SkillResolver Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract skill resolution behind a `tau_ports::SkillResolver` port so `tau-runtime-core` drops its `tau-pkg` dependency and cross-compiles to `wasm32-wasip2`, completing the β.1 port pattern (Clock / RandomSource / CapabilityResolver already are ports).

**Architecture:** Mirror the existing `CapabilityResolver` port exactly. A new no_std `SkillResolver` trait in `tau-ports` returns a `ResolvedSkill` (install-path string + capabilities + SKILL.md body) or a `SkillResolveError`. `tau-runtime-core` calls only through the injected `Arc<dyn SkillResolver>` carried in `RunOptions`; all `tau_pkg::{find_installed_skill, Scope}` + `std::fs` reads move into a new `TauPkgSkillResolver` adapter in `tau-runtime-tokio`. The kernel's pure logic (`${SKILL_DIR}` substitution, scope narrowing, subset law) stays in core. A `NoSkillResolver` (always `NotFound`) ships in `tau-ports` for guest/wasm shells. A CI step builds core for `wasm32-wasip2 --no-default-features` so no_std cannot re-drift.

**Tech Stack:** Rust, hexagonal ports (tau-ports = no_std + alloc), tokio host shell, tau-pkg (std), globset, serde_json, wasm32-wasip2 target.

---

## Why this works (RED state, verified 2026-06-16)

`cargo build -p tau-runtime-core --target wasm32-wasip2 --no-default-features` fails today:

```
error[E0554]: `#![feature]` may not be used on the stable release channel
 --> rustix-0.38.44/src/lib.rs:9:5  |  feature(wasip2)
```

`rustix 0.38.44` is pulled by `tempfile`, which is pulled by **`tau-pkg`** (`crates/tau-pkg/Cargo.toml`: `tokio`, `tempfile`, `fs4`, `walkdir`). `tau-pkg` is a **non-optional** dependency of `tau-runtime-core` (`Cargo.toml:14`), so it compiles even under `--no-default-features` (where `host-fs` / `tool-validation` are off and the skill code itself isn't even compiled). Removing `tau-pkg` from core eliminates `tokio` + `rustix` + `fs4` + `walkdir` + `tempfile` from the graph. `wasm32-wasip2` is a full std target, so the remaining deps (`serde_json`, `globset`, `uuid v4`, `chrono`, …) compile there on stable.

## File Structure

- **Create** `crates/tau-ports/src/skill_resolver.rs` — the port: `SkillResolver` trait, `ResolvedSkill`, `SkillResolveError`, `NoSkillResolver`. no_std + alloc. Mirrors `capability_resolver.rs`.
- **Modify** `crates/tau-ports/src/lib.rs` — `pub mod skill_resolver;` + re-exports.
- **Modify** `crates/tau-runtime-core/src/options.rs` — add `skill_resolver` field to `RunOptions` (+ Debug + Default).
- **Modify** `crates/tau-runtime-core/src/orchestration/skill_resolve.rs` — `resolve_skill_for_spawn` takes `&dyn SkillResolver` instead of `&Scope`; drop `tau_pkg` + `std::fs` + `parse_skill_md`; `substitute_skill_dir` takes `&str`.
- **Modify** `crates/tau-runtime-core/src/orchestration/virtual_tools.rs` — `validate_skill_spawn` takes `&dyn SkillResolver`.
- **Modify** `crates/tau-runtime-core/src/stream.rs` — skill-spawn arm reads `options.skill_resolver` instead of resolving a `tau_pkg::Scope`; child opts propagate it.
- **Modify** `crates/tau-runtime-core/src/run.rs` — `spawn_root_agent_inner` gains a `skill_resolver` param, sets it in `opts`.
- **Modify** `crates/tau-runtime-core/Cargo.toml` — remove `tau-pkg` dep.
- **Create** `crates/tau-runtime-tokio/src/skill_resolver_impl.rs` — `TauPkgSkillResolver` adapter (mirrors `capability_resolver_impl.rs`).
- **Modify** `crates/tau-runtime-tokio/src/lib.rs` — `pub mod skill_resolver_impl;` + export.
- **Modify** `crates/tau-runtime-tokio/src/runtime_ext.rs` — build `TauPkgSkillResolver` from `scope_root`, pass to `spawn_root_agent_inner`.
- **Modify** `.github/workflows/ci.yml` — add a `wasm32-wasip2` build step to the `runtime-core-no-std` job.

## Cargo discipline (CLAUDE.md — every cargo call)

```
timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p <crate>
```

Timeouts: test 300, build/check 180, clippy 240, fmt 30. Run from the worktree
`/Users/titouanlebocq/code/tau-worktrees/beta-7-5-skillresolver`. The wasm cross-build
is a fresh target tree — allow up to 420s the first time.

---

### Task 1: Define the `SkillResolver` port in tau-ports

**Files:**
- Create: `crates/tau-ports/src/skill_resolver.rs`
- Modify: `crates/tau-ports/src/lib.rs:25` (add `pub mod`) and `:44` (add re-export)
- Test: inline `#[cfg(test)] mod tests` in `skill_resolver.rs`

- [ ] **Step 1: Write the port file with a failing test**

Create `crates/tau-ports/src/skill_resolver.rs`:

```rust
//! Skill resolver port — looks up an installed skill by name and
//! returns its install path, declared capabilities, and SKILL.md body.
//!
//! The kernel doesn't know how skills are stored on disk — that's a
//! host-shell concern (tau-runtime-tokio ships a `tau_pkg`-backed impl
//! that reads the scope lockfile + the skill's `tau.toml` + SKILL.md).
//! Embassy/wasm guest shells with no on-disk package store can ship the
//! [`NoSkillResolver`] (always `NotFound`).
//!
//! Routing skill resolution through a port is what lets
//! `tau-runtime-core::Runtime` drive the `skill.<name>.spawn` virtual
//! tool without linking `tau-pkg` (which pulls tokio/rustix and does not
//! cross-compile to `wasm32-wasip2`).

use alloc::string::String;
use alloc::vec::Vec;

use tau_domain::Capability;

/// A resolved installed skill, ready for the kernel's
/// `skill.<name>.spawn` dispatch.
///
/// Produced by [`SkillResolver::resolve`]; the kernel applies
/// `${SKILL_DIR}` substitution, scope narrowing, and the capability
/// subset law to these fields before spawning the child agent.
///
/// `install_path` is a `String` (not `PathBuf`) so the type stays usable
/// in `no_std + alloc` guest shells; host adapters pass
/// `path.display().to_string()`.
#[derive(Debug, Clone)]
pub struct ResolvedSkill {
    /// Absolute path to the installed skill directory, as a display
    /// string. Used as the `${SKILL_DIR}` substitution value.
    pub install_path: String,
    /// Declared capabilities from the skill's manifest (pre-substitution).
    pub capabilities: Vec<Capability>,
    /// The skill's default system prompt — the SKILL.md body, already
    /// read and parsed by the adapter. The kernel uses this unless the
    /// caller supplies a `system_prompt` override.
    pub system_prompt: String,
}

/// Error returned by [`SkillResolver::resolve`].
///
/// Variants map onto the kernel's `OrchestrationError` skill variants so
/// the kernel can surface a typed error without depending on the host's
/// concrete error type (`tau_pkg::FindSkillError`).
#[derive(Debug, Clone)]
pub enum SkillResolveError {
    /// No installed skill matches the requested name.
    NotFound,
    /// A lockfile entry exists but the install path is missing on disk.
    InstallPathMissing {
        /// The expected install path, as a display string.
        expected_path: String,
    },
    /// The skill's manifest or SKILL.md could not be read/parsed, or the
    /// scope itself could not be resolved.
    Invalid {
        /// Human-readable reason.
        detail: String,
    },
}

/// Resolve an installed skill by name.
///
/// Host shells implement this against their on-disk package store
/// (tau-runtime-tokio ships `TauPkgSkillResolver`). Guest shells with no
/// store ship [`NoSkillResolver`].
pub trait SkillResolver: Send + Sync {
    /// Look up `name` and return the resolved skill, or a typed error.
    fn resolve(&self, name: &str) -> Result<ResolvedSkill, SkillResolveError>;
}

/// A [`SkillResolver`] that always reports [`SkillResolveError::NotFound`].
///
/// Ships for guest shells (wasm/embassy) that have no on-disk skill store
/// but still link the kernel. A `skill.<name>.spawn` call then fails
/// gracefully with a skill-not-installed error instead of panicking.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSkillResolver;

impl SkillResolver for NoSkillResolver {
    fn resolve(&self, _name: &str) -> Result<ResolvedSkill, SkillResolveError> {
        Err(SkillResolveError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;

    #[test]
    fn no_skill_resolver_always_not_found() {
        let r: Arc<dyn SkillResolver> = Arc::new(NoSkillResolver);
        let err = r.resolve("anything").expect_err("should be NotFound");
        assert!(matches!(err, SkillResolveError::NotFound));
    }
}
```

- [ ] **Step 2: Run the test to verify it fails (module not wired yet)**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ports --lib skill_resolver`
Expected: FAIL — `error[E0583]: file not found for module` / `unresolved module` because `lib.rs` does not yet declare the module.

- [ ] **Step 3: Wire the module + re-exports into `lib.rs`**

In `crates/tau-ports/src/lib.rs`, add the module declaration after the `pub mod capability_resolver;` line (currently `:25`):

```rust
pub mod skill_resolver;
```

And add the re-export after the `capability_resolver` re-export (currently `:44`):

```rust
pub use skill_resolver::{NoSkillResolver, ResolvedSkill, SkillResolveError, SkillResolver};
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ports --lib skill_resolver`
Expected: PASS (`no_skill_resolver_always_not_found ... ok`).

- [ ] **Step 5: Confirm tau-ports still builds no_std-clean and rustdoc passes**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-ports --no-default-features`
Expected: PASS (no_std build clean — `deny(missing_docs)` is satisfied by the doc comments above).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-ports/src/skill_resolver.rs crates/tau-ports/src/lib.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-ports): SkillResolver port + NoSkillResolver"
```

---

### Task 2: Add `skill_resolver` to `RunOptions`

**Files:**
- Modify: `crates/tau-runtime-core/src/options.rs` (field ~`:101`, Debug ~`:151`, Default ~`:184`)
- Test: inline `#[cfg(test)] mod tests` in `options.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/tau-runtime-core/src/options.rs` `mod tests`:

```rust
    #[test]
    fn run_options_skill_resolver_defaults_to_none() {
        let opts = RunOptions::default();
        assert!(opts.skill_resolver.is_none());
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-runtime-core --lib options::tests::run_options_skill_resolver_defaults_to_none`
Expected: FAIL — `no field 'skill_resolver' on type '&RunOptions'`.

- [ ] **Step 3: Add the field**

In `crates/tau-runtime-core/src/options.rs`, add the field immediately after `pub capability_resolver: ...` (currently `:101`):

```rust
    /// Skill resolver used to look up installed skills for
    /// `skill.<name>.spawn` virtual-tool dispatch. Host shells supply
    /// their impl — tau-runtime-tokio ships `TauPkgSkillResolver` over
    /// `tau_pkg::find_installed_skill`; guest (wasm/embassy) shells ship
    /// `tau_ports::NoSkillResolver` or leave this `None`.
    ///
    /// When `None`, a `skill.<name>.spawn` call fails gracefully with a
    /// "no skill resolver available" tool error (the kernel never reads
    /// the filesystem itself). Single-agent runs leave this `None`.
    pub skill_resolver: Option<Arc<dyn tau_ports::SkillResolver>>,
```

In the `Debug` impl, add after the `capability_resolver` field (currently ~`:156`):

```rust
            .field(
                "skill_resolver",
                &self.skill_resolver.as_ref().map(|_| "<SkillResolver>"),
            )
```

In the `Default` impl, add after `capability_resolver: None,` (currently `:184`):

```rust
            skill_resolver: None,
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-runtime-core --lib options::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-runtime-core/src/options.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-runtime-core): add skill_resolver to RunOptions"
```

---

### Task 3: Rewrite `resolve_skill_for_spawn` to use the port

**Files:**
- Modify: `crates/tau-runtime-core/src/orchestration/skill_resolve.rs`
- Test: inline `#[cfg(test)] mod tests` in the same file

This replaces the `tau_pkg::{find_installed_skill, Scope}` + `std::fs::read_to_string` + `tau_domain::parse_skill_md` body with a single `resolver.resolve(name)` call, and changes `substitute_skill_dir` to take a `&str` install path (no `std::path`).

- [ ] **Step 1: Write the failing tests**

Add to `crates/tau-runtime-core/src/orchestration/skill_resolve.rs` `mod tests`. First add a mock resolver and helper at the top of `mod tests` (after the existing `fn net_http` helper):

```rust
    use tau_ports::{ResolvedSkill, SkillResolveError, SkillResolver};

    struct MockResolver {
        result: Result<ResolvedSkill, SkillResolveError>,
    }
    impl SkillResolver for MockResolver {
        fn resolve(&self, _name: &str) -> Result<ResolvedSkill, SkillResolveError> {
            self.result.clone()
        }
    }

    #[test]
    fn resolve_skill_for_spawn_builds_request_from_port() {
        let resolver = MockResolver {
            result: Ok(ResolvedSkill {
                install_path: "/scope/.tau/packages/critic/0.1.0".to_string(),
                capabilities: vec![fs_read(vec!["${SKILL_DIR}/refs/**"])],
                system_prompt: "You are a critic.".to_string(),
            }),
        };
        let args = SkillSpawnArgs {
            message: "review this".into(),
            system_prompt: None,
            scope_paths: None,
        };
        // parent grants fs.read over the substituted install path so the
        // subset law passes.
        let parent = vec![fs_read(vec!["/scope/.tau/packages/critic/0.1.0/refs/**"])];
        let req = resolve_skill_for_spawn("critic", &args, &parent, &resolver)
            .expect("resolve ok");
        assert_eq!(req.skill_name, "critic");
        assert_eq!(req.system_prompt, "You are a critic.");
        assert_eq!(
            req.install_path,
            std::path::PathBuf::from("/scope/.tau/packages/critic/0.1.0")
        );
        // ${SKILL_DIR} was substituted in the grant.
        match &req.grant[0] {
            Capability::Filesystem(FsCapability::Read { paths, .. }) => {
                assert_eq!(paths[0], "/scope/.tau/packages/critic/0.1.0/refs/**");
            }
            other => panic!("expected fs.read, got {other:?}"),
        }
    }

    #[test]
    fn resolve_skill_for_spawn_maps_not_found() {
        let resolver = MockResolver {
            result: Err(SkillResolveError::NotFound),
        };
        let args = SkillSpawnArgs::default();
        let err = resolve_skill_for_spawn("ghost", &args, &[], &resolver).unwrap_err();
        assert!(matches!(
            err,
            OrchestrationError::SkillNotInstalled { .. }
        ));
    }

    #[test]
    fn resolve_skill_for_spawn_caller_override_wins() {
        let resolver = MockResolver {
            result: Ok(ResolvedSkill {
                install_path: "/scope/skill".to_string(),
                capabilities: vec![],
                system_prompt: "default body".to_string(),
            }),
        };
        let args = SkillSpawnArgs {
            message: "go".into(),
            system_prompt: Some("override body".into()),
            scope_paths: None,
        };
        let req = resolve_skill_for_spawn("s", &args, &[], &resolver).expect("ok");
        assert_eq!(req.system_prompt, "override body");
    }
```

Also update the two existing `substitute_skill_dir` unit tests to pass a `&str` instead of `std::path::Path::new(...)`:

```rust
    #[test]
    fn substitute_skill_dir_replaces_in_fs_read() {
        let caps = vec![fs_read(vec!["${SKILL_DIR}/refs/**"])];
        let out = substitute_skill_dir(&caps, "/scope/.tau/packages/critic/0.1.0");
        match &out[0] {
            Capability::Filesystem(FsCapability::Read { paths, .. }) => {
                assert_eq!(paths[0], "/scope/.tau/packages/critic/0.1.0/refs/**");
            }
            other => panic!("expected fs.read, got {other:?}"),
        }
    }

    #[test]
    fn substitute_skill_dir_passes_through_non_fs() {
        let caps = vec![net_http(vec!["api.example.com"])];
        let out = substitute_skill_dir(&caps, "/scope");
        assert_eq!(out.len(), 1);
        match &out[0] {
            Capability::Network(NetCapability::Http {
                hosts, methods: _, ..
            }) => {
                assert_eq!(hosts[0], "api.example.com");
            }
            other => panic!("expected net.http, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-runtime-core --lib orchestration::skill_resolve`
Expected: FAIL to COMPILE — `resolve_skill_for_spawn` still expects `&Scope`; `substitute_skill_dir` still expects `&Path`; `tau_ports::SkillResolver` unused import error etc.

- [ ] **Step 3: Rewrite the imports and `substitute_skill_dir` signature**

In `crates/tau-runtime-core/src/orchestration/skill_resolve.rs`:

Replace the import block (currently lines 14–28):

```rust
use alloc::string::String;
#[cfg(feature = "host-fs")]
use alloc::string::ToString;
use alloc::vec::Vec;

use globset::GlobBuilder;
#[cfg(feature = "host-fs")]
use tau_domain::SKILL_DIR_VAR;
use tau_domain::{Capability, FsCapability};
#[cfg(feature = "host-fs")]
use tau_pkg::{find_installed_skill, Scope};

use crate::orchestration::error::OrchestrationError;
#[cfg(feature = "host-fs")]
use crate::orchestration::virtual_tools::check_capability_subset;
```

with (drop `tau_pkg`; `substitute_skill_dir` no longer needs `std::path`, so `SKILL_DIR_VAR` becomes unconditional and `ToString` stays host-fs for the `to_string()` calls in `resolve_skill_for_spawn`):

```rust
use alloc::string::String;
#[cfg(feature = "host-fs")]
use alloc::string::ToString;
use alloc::vec::Vec;

use globset::GlobBuilder;
use tau_domain::SKILL_DIR_VAR;
use tau_domain::{Capability, FsCapability};

use crate::orchestration::error::OrchestrationError;
#[cfg(feature = "host-fs")]
use crate::orchestration::virtual_tools::check_capability_subset;
#[cfg(feature = "host-fs")]
use tau_ports::SkillResolver;
```

Change `substitute_skill_dir` (currently `:124`) from `#[cfg(feature = "host-fs")]` + `&std::path::Path` to an ungated `&str` version. Replace its attribute/signature/first line:

```rust
#[cfg(feature = "host-fs")]
pub fn substitute_skill_dir(
    caps: &[Capability],
    install_path: &std::path::Path,
) -> Vec<Capability> {
    let install_str = install_path.display().to_string();
```

with:

```rust
pub fn substitute_skill_dir(caps: &[Capability], install_path: &str) -> Vec<Capability> {
```

and inside the closure that follows, change `p.replace(SKILL_DIR_VAR, &install_str)` to `p.replace(SKILL_DIR_VAR, install_path)`. Also update its doc-comment example: remove the `# #[cfg(feature = "host-fs")]` guard wrapper and the `use std::path::Path;` line, and change the call to `substitute_skill_dir(&[cap], "/skills/critic/1.0.0")`. The doc example becomes:

```rust
/// ```
/// use tau_domain::{Capability, FsCapability, SKILL_DIR_VAR};
/// use tau_runtime_core::orchestration::skill_resolve::substitute_skill_dir;
///
/// let cap: Capability = serde_json::from_value(serde_json::json!({
///     "kind": "fs.read",
///     "paths": [format!("{SKILL_DIR_VAR}/refs/**")]
/// })).expect("valid capability");
///
/// let out = substitute_skill_dir(&[cap], "/skills/critic/1.0.0");
/// if let Capability::Filesystem(FsCapability::Read { paths, .. }) = &out[0] {
///     assert_eq!(paths[0], "/skills/critic/1.0.0/refs/**");
/// }
/// ```
```

Also remove the line `/// Gated behind `host-fs` because it takes a `std::path::Path` argument.` from that doc-comment.

- [ ] **Step 4: Rewrite the `resolve_skill_for_spawn` body**

Replace the whole `resolve_skill_for_spawn` function (currently `:305`–`:374`, including its `#[cfg(feature = "host-fs")]` attribute and doc-comment) with:

```rust
/// End-to-end resolution. Looks up the skill via the injected
/// [`SkillResolver`] port, substitutes `${SKILL_DIR}`, narrows by
/// `scope_paths`, verifies the subset law, returns the request.
///
/// `parent_grant` is the parent agent's effective capability grant —
/// used for the v1.1 capability subset law.
///
/// **Requires the `host-fs` feature**: returns a [`SkillSpawnRequest`]
/// whose `install_path` is a `std::path::PathBuf`. Guest shells without
/// `host-fs` cannot construct the request and never call this; they ship
/// `tau_ports::NoSkillResolver` so a `skill.<name>.spawn` fails gracefully.
#[cfg(feature = "host-fs")]
pub fn resolve_skill_for_spawn(
    skill_name: &str,
    args: &SkillSpawnArgs,
    parent_grant: &[Capability],
    resolver: &dyn SkillResolver,
) -> Result<SkillSpawnRequest, OrchestrationError> {
    use tau_ports::SkillResolveError;

    let resolved = resolver.resolve(skill_name).map_err(|e| match e {
        SkillResolveError::NotFound => OrchestrationError::SkillNotInstalled {
            name: skill_name.to_string(),
        },
        SkillResolveError::InstallPathMissing { expected_path } => {
            OrchestrationError::SkillInstallPathMissing {
                name: skill_name.to_string(),
                expected_path: std::path::PathBuf::from(expected_path),
            }
        }
        SkillResolveError::Invalid { detail } => OrchestrationError::SkillContentInvalid {
            name: skill_name.to_string(),
            detail,
        },
    })?;

    // 1. system_prompt: caller override OR the resolved SKILL.md body.
    let system_prompt = args
        .system_prompt
        .clone()
        .unwrap_or(resolved.system_prompt);

    // 2. ${SKILL_DIR} substitution in capabilities.
    let install_path = resolved.install_path;
    let substituted = substitute_skill_dir(&resolved.capabilities, &install_path);

    // 3. Apply caller's scope_paths if provided.
    let scoped = if let Some(sp) = &args.scope_paths {
        apply_scope_paths(substituted, sp)?
    } else {
        substituted
    };

    // 4. Subset law: child grant ⊆ parent grant.
    check_capability_subset(parent_grant, &scoped)?;

    Ok(SkillSpawnRequest {
        skill_name: skill_name.to_string(),
        install_path: std::path::PathBuf::from(install_path),
        system_prompt,
        grant: scoped,
        message: args.message.clone(),
    })
}
```

Also update the module-level doc-comment at the top of the file (currently line 12): change `**Requires the `host-fs` feature** (reads SKILL.md via `std::fs`).` to `**Requires the `host-fs` feature** (returns a `PathBuf`). Skill lookup + SKILL.md reading are delegated to the injected `tau_ports::SkillResolver` port.`

- [ ] **Step 5: Run the tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-runtime-core --lib orchestration::skill_resolve`
Expected: PASS (this will still fail to *link* the crate if `virtual_tools` / `stream` callers aren't updated yet — that's Tasks 4 & 5. If the lib doesn't compile, proceed to Task 4 first, then re-run. To check just this file's logic in isolation is not possible; the lib compiles as a unit. **Do Tasks 4 and 5 before re-running Step 5.**)

- [ ] **Step 6: Commit (after Tasks 4 & 5 make the crate compile — see note)**

Defer the commit for this task until Task 5; commit Tasks 3–5 together since the crate only compiles once all three callers are consistent:

```bash
git add crates/tau-runtime-core/src/orchestration/skill_resolve.rs
# committed together in Task 5
```

---

### Task 4: Update `validate_skill_spawn` to take the port

**Files:**
- Modify: `crates/tau-runtime-core/src/orchestration/virtual_tools.rs:604`–`:653`

- [ ] **Step 1: Change the signature + call site**

In `crates/tau-runtime-core/src/orchestration/virtual_tools.rs`, change the `validate_skill_spawn` signature (currently `:604`):

```rust
#[cfg(feature = "host-fs")]
pub fn validate_skill_spawn(
    tool_name: &str,
    args: &Value,
    parent: &AgentId,
    parent_grant: &[Capability],
    scope: &tau_pkg::Scope,
) -> Result<crate::orchestration::SkillSpawnRequest, OrchestrationError> {
```

to:

```rust
#[cfg(feature = "host-fs")]
pub fn validate_skill_spawn(
    tool_name: &str,
    args: &Value,
    parent: &AgentId,
    parent_grant: &[Capability],
    resolver: &dyn tau_ports::SkillResolver,
) -> Result<crate::orchestration::SkillSpawnRequest, OrchestrationError> {
```

And change the final call (currently `:652`):

```rust
    crate::orchestration::resolve_skill_for_spawn(name, &spawn_args, parent_grant, scope)
```

to:

```rust
    crate::orchestration::resolve_skill_for_spawn(name, &spawn_args, parent_grant, resolver)
```

Also update the doc-comment at `:602` that says `Gated behind `host-fs` because it returns `SkillSpawnRequest` which contains a `std::path::PathBuf` install path.` — that remains accurate, leave it. (No other doc change needed.)

- [ ] **Step 2: No standalone test run** (crate links only after Task 5). Proceed to Task 5.

---

### Task 5: Update the stream.rs skill-spawn dispatch

**Files:**
- Modify: `crates/tau-runtime-core/src/stream.rs:641`–`:692` (scope resolution → resolver), `:786`–`:796` and `:1050` (child opts propagation), and the comment at `:616`–`:620`.

- [ ] **Step 1: Replace the Scope-resolution block with a resolver read**

In `crates/tau-runtime-core/src/stream.rs`, replace the block that resolves a `tau_pkg::Scope` (currently `:642`–`:681`, beginning `if is_skill_spawn {` and the `let scope_result = ...` / `let scope = match scope_result { ... }` portion) so that it reads `options.skill_resolver` instead. Replace from `if is_skill_spawn {` through the closing `};` of the `let scope = match scope_result` block with:

```rust
                        #[cfg(feature = "host-fs")]
                        if is_skill_spawn {
                            // Resolve the skill via the injected SkillResolver
                            // port (host shells supply TauPkgSkillResolver;
                            // guest shells supply NoSkillResolver or None).
                            // A `None` resolver fails gracefully here.
                            let resolver = match options.skill_resolver.as_ref() {
                                Some(r) => r.clone(),
                                None => {
                                    yield make_skill_spawn_error_tool_result(
                                        tool_use,
                                        "no skill resolver available for skill resolution",
                                    );
                                    // Append error tool-result message so LLM
                                    // history is coherent.
                                    let err_msg = Message::new(
                                        tool_addr.clone(),
                                        agent_addr.clone(),
                                        MessagePayload::ToolError {
                                            kind: "orchestration_virtual_tool_error"
                                                .into(),
                                            message: "skill spawn failed: no skill \
                                                      resolver available for skill \
                                                      resolution"
                                                .into(),
                                            details: None,
                                        },
                                    );
                                    debug!(
                                        parent: &turn_span,
                                        name = EV_MESSAGE_ADDED,
                                        role = ?err_msg.sender,
                                    );
                                    messages.push(err_msg);
                                    continue;
                                }
                            };
```

Then change the `validate_skill_spawn` call (currently `:686`–`:692`) from passing `&scope` to passing `resolver.as_ref()`:

```rust
                            let skill_req = match crate::orchestration::validate_skill_spawn(
                                &tool_use.name,
                                &args_json,
                                &agent_id_str,
                                &granted_capabilities,
                                resolver.as_ref(),
                            ) {
```

- [ ] **Step 2: Propagate `skill_resolver` into both child `RunOptions`**

In the child opts built at `:786` (the `let child_opts = crate::RunOptions { ... }` block), add a `skill_resolver` line next to `scope_root`:

```rust
                                        let child_opts = crate::RunOptions {
                                            orchestration_state: Some(state_arc.clone()),
                                            orchestration_runtime: Some(child_runtime.clone()),
                                            granted_capabilities_override: Some(
                                                skill_req.grant.clone(),
                                            ),
                                            clock: options.clock.clone(),
                                            random: options.random.clone(),
                                            scope_root: options.scope_root.clone(),
                                            skill_resolver: options.skill_resolver.clone(),
                                            ..Default::default()
                                        };
```

Find the second child-opts construction near `:1050` (the `agent.<kind>.spawn` arm, which also sets `scope_root: options.scope_root.clone(),`) and add the same `skill_resolver: options.skill_resolver.clone(),` line there. (Verify with: `grep -n "scope_root: options.scope_root.clone()" crates/tau-runtime-core/src/stream.rs` — add the line after each match.)

- [ ] **Step 3: Update the stale comment**

Update the comment at `:616`–`:620` that mentions `tau_pkg::Scope`:

```rust
                        // `is_skill_spawn` is host-fs-only: skill resolution
                        // goes through the injected SkillResolver port. Shells
                        // without host-fs (embassy/wasm v1) can't reach the
                        // skill dispatch arm; tool_use.name patterns are
                        // dispatched as plain plugin calls there.
```

- [ ] **Step 4: Build the crate (default features) to confirm Tasks 3–5 link**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-runtime-core`
Expected: PASS — note `tau-pkg` is still in `Cargo.toml`, so this proves the *new* code paths compile before we remove the dep. If `tau_pkg` is now genuinely unused you may see an `unused crate dependency` style warning only if the workspace enables that lint (it does not by default) — ignore; removal happens in Task 6.

- [ ] **Step 5: Run the full core lib test suite**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core`
Expected: PASS (including the three new `skill_resolve` tests). Doctests: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-runtime-core --doc` → PASS (the edited `substitute_skill_dir` doctest compiles ungated).

- [ ] **Step 6: Commit Tasks 3–5 together**

```bash
git add crates/tau-runtime-core/src/orchestration/skill_resolve.rs \
        crates/tau-runtime-core/src/orchestration/virtual_tools.rs \
        crates/tau-runtime-core/src/stream.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "refactor(tau-runtime-core): route skill resolution through SkillResolver port"
```

---

### Task 6: Drop `tau-pkg` from core + thread resolver through `spawn_root_agent_inner`

**Files:**
- Modify: `crates/tau-runtime-core/src/run.rs:481`–`:561` (add param + set in opts)
- Modify: `crates/tau-runtime-core/src/options.rs:94` (comment cleanup — drop stale `tau_pkg` mention is optional; the comment is on `capability_resolver`, leave unless inaccurate)
- Modify: `crates/tau-runtime-core/Cargo.toml:14` (remove dep)

- [ ] **Step 1: Add the `skill_resolver` parameter to `spawn_root_agent_inner`**

In `crates/tau-runtime-core/src/run.rs`, add a parameter after `scope_root` (currently `:498`) in the `spawn_root_agent_inner` signature:

```rust
        // Project-scope root as a String. ... (existing doc) ...
        scope_root: Option<alloc::string::String>,
        // Skill resolver for `skill.<name>.spawn` dispatch. Host shells
        // build a `TauPkgSkillResolver` from their scope; guest shells
        // pass `None` (or a `NoSkillResolver`). Carried into `RunOptions`.
        skill_resolver: Option<Arc<dyn tau_ports::SkillResolver>>,
```

And set it in the `opts` construction (currently `:554`):

```rust
        let opts = crate::options::RunOptions {
            orchestration_state: Some(state_arc.clone()),
            orchestration_runtime: Some(self.clone()),
            clock: Some(clock.clone()),
            random: Some(random.clone()),
            scope_root,
            skill_resolver,
            ..Default::default()
        };
```

- [ ] **Step 2: Remove `tau-pkg` from `Cargo.toml`**

In `crates/tau-runtime-core/Cargo.toml`, delete line 14:

```toml
tau-pkg      = { workspace = true }
```

- [ ] **Step 3: Build core default + no-default-features**

Run:
```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-runtime-core
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-runtime-core --no-default-features
```
Expected: BOTH PASS. (The `spawn_root_agent_inner` caller in tau-runtime-tokio is now broken — that's expected; Task 7 fixes it. This step only proves *core* no longer needs tau-pkg.)

- [ ] **Step 4: THE regression guard — build core for wasm32-wasip2**

Run:
```
rustup target add wasm32-wasip2
timeout 420 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo build -p tau-runtime-core --target wasm32-wasip2 --no-default-features
```
Expected: PASS — no `rustix`/`tokio`/`mio`/`tempfile` in the graph. If a different std-puller surfaces (e.g. `serde_json` requiring `default-features=false` + `alloc`, or a `getrandom` backend gap for `uuid`), fix it: prefer `default-features = false` + the `alloc` feature on the offending workspace dep entry for `tau-runtime-core`, and re-run. Document any such change in the commit message.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-runtime-core/src/run.rs crates/tau-runtime-core/Cargo.toml Cargo.lock
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(β.7.5): tau-runtime-core drops tau-pkg; builds for wasm32-wasip2"
```

---

### Task 7: `TauPkgSkillResolver` adapter in tau-runtime-tokio

**Files:**
- Create: `crates/tau-runtime-tokio/src/skill_resolver_impl.rs`
- Modify: `crates/tau-runtime-tokio/src/lib.rs` (add `pub mod` + export)
- Modify: `crates/tau-runtime-tokio/src/runtime_ext.rs:25`–`:63` (build resolver, pass to inner)
- Test: inline `#[cfg(test)] mod tests` in `skill_resolver_impl.rs`

- [ ] **Step 1: Write the adapter with a failing test**

Create `crates/tau-runtime-tokio/src/skill_resolver_impl.rs`:

```rust
//! `tau_pkg`-backed implementation of the [`SkillResolver`] port.
//!
//! Wraps `tau_pkg::find_installed_skill` + the SKILL.md read so the
//! kernel can resolve `skill.<name>.spawn` targets through a stable trait
//! without linking `tau-pkg` (which pulls tokio/rustix and does not
//! cross-compile to `wasm32-wasip2`).
//!
//! Construct with [`TauPkgSkillResolver::new`] from the project scope
//! root; the kernel calls `resolve()` each time a `skill.<name>.spawn`
//! virtual tool fires.

use std::path::PathBuf;

use tau_pkg::{find_installed_skill, FindSkillError, Scope};
use tau_ports::{ResolvedSkill, SkillResolveError, SkillResolver};

/// Production skill resolver: resolves a `tau_pkg::Scope` from the
/// project scope root, looks up the installed skill, and reads its
/// SKILL.md body.
///
/// Build once per orchestrated run from the scope root and stuff
/// `Arc<TauPkgSkillResolver>` into `RunOptions.skill_resolver`.
pub struct TauPkgSkillResolver {
    scope_root: PathBuf,
}

impl TauPkgSkillResolver {
    /// Construct from the project scope root (the directory containing
    /// `.tau/`). Scope resolution is deferred to `resolve()` so a bad
    /// root surfaces as a typed `SkillResolveError::Invalid` rather than
    /// a constructor panic.
    pub fn new(scope_root: PathBuf) -> Self {
        Self { scope_root }
    }
}

impl SkillResolver for TauPkgSkillResolver {
    fn resolve(&self, name: &str) -> Result<ResolvedSkill, SkillResolveError> {
        let scope = Scope::resolve(&self.scope_root).map_err(|e| SkillResolveError::Invalid {
            detail: format!("resolving scope at {:?}: {e}", self.scope_root),
        })?;

        let installed = match find_installed_skill(&scope, name) {
            Ok(Some(s)) => s,
            Ok(None) => return Err(SkillResolveError::NotFound),
            Err(FindSkillError::InstallPathMissing { path, .. }) => {
                return Err(SkillResolveError::InstallPathMissing {
                    expected_path: path.display().to_string(),
                });
            }
            Err(e) => {
                return Err(SkillResolveError::Invalid {
                    detail: e.to_string(),
                });
            }
        };

        // Read + parse SKILL.md for the default system prompt.
        let skill_md_path = installed.install_path.join(&installed.skill.content);
        let text = std::fs::read_to_string(&skill_md_path).map_err(|e| {
            SkillResolveError::Invalid {
                detail: format!("reading SKILL.md at {skill_md_path:?}: {e}"),
            }
        })?;
        let parsed = tau_domain::parse_skill_md(&text).map_err(|e| SkillResolveError::Invalid {
            detail: format!("parsing SKILL.md: {e}"),
        })?;

        Ok(ResolvedSkill {
            install_path: installed.install_path.display().to_string(),
            capabilities: installed.capabilities,
            system_prompt: parsed.body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_scope_returns_invalid() {
        // A path with no .tau/ anywhere up the tree → Scope::resolve errs.
        let resolver = TauPkgSkillResolver::new(PathBuf::from("/nonexistent/tau/root"));
        let err = resolver.resolve("critic").unwrap_err();
        assert!(matches!(err, SkillResolveError::Invalid { .. }));
    }
}
```

NOTE: confirm `installed.skill.content` is the SKILL.md filename field used by the existing reader (it is — see the pre-refactor `crates/tau-runtime-core/src/orchestration/skill_resolve.rs:337` `installed.install_path.join(&installed.skill.content)`). If `SkillManifest`'s field differs, match the original.

- [ ] **Step 2: Wire the module into `lib.rs`**

In `crates/tau-runtime-tokio/src/lib.rs`, add near the other `pub mod` declarations (alongside `capability_resolver_impl`):

```rust
pub mod skill_resolver_impl;
```

- [ ] **Step 3: Run the adapter test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio -E 'test(skill_resolver_impl)'`
Expected: FAIL FIRST (module/signature mismatch) → after Step 1+2 land, PASS (`missing_scope_returns_invalid`). If `spawn_root_agent_inner`'s new arg breaks compilation, do Step 4 first.

- [ ] **Step 4: Wire the resolver into `spawn_root_agent_with_scope`**

In `crates/tau-runtime-tokio/src/runtime_ext.rs`, build the resolver from `scope_root` and pass it as the new final arg to `spawn_root_agent_inner`. After the `let scope_root_str = ...` line (currently `:48`), add:

```rust
    let skill_resolver: std::sync::Arc<dyn tau_ports::SkillResolver> =
        std::sync::Arc::new(crate::skill_resolver_impl::TauPkgSkillResolver::new(
            scope_root.clone(),
        ));
```

and change the `spawn_root_agent_inner` call (currently `:50`–`:60`) to pass `Some(skill_resolver)` as the new trailing argument:

```rust
    runtime
        .spawn_root_agent_inner(
            root_agent_def,
            root_manifest,
            initial_message,
            budget,
            vec![subscriber],
            Some(clock),
            Some(random),
            Some(run_id),
            Some(scope_root_str),
            Some(skill_resolver),
        )
        .await
        .map_err(RuntimeError::Core)
```

(`scope_root` is consumed by `run_log_path(&scope_root, ...)` earlier; add `.clone()` there if the borrow checker complains, or build `skill_resolver` before the `run_log_path` call. Confirm by reading the current order: `run_log_path` is at `:43`, before `scope_root_str` at `:48`, so building the resolver at `:49` with `scope_root.clone()` is safe.)

- [ ] **Step 5: Build + test tau-runtime-tokio**

Run:
```
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-runtime-tokio
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio
```
Expected: BOTH PASS. The existing multi-agent / skill-spawn integration tests now run through the real `TauPkgSkillResolver`.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-runtime-tokio/src/skill_resolver_impl.rs \
        crates/tau-runtime-tokio/src/lib.rs \
        crates/tau-runtime-tokio/src/runtime_ext.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "feat(tau-runtime-tokio): TauPkgSkillResolver adapter + wiring"
```

---

### Task 8: CI regression guard — `core-builds-wasm`

**Files:**
- Modify: `.github/workflows/ci.yml` — `runtime-core-no-std` job (`:349`–`:380`)

- [ ] **Step 1: Add the wasm build step**

In `.github/workflows/ci.yml`, in the `runtime-core-no-std` job, add a step after the existing "no-std builds (default and no-default-features)" step (which ends at `:364`) and before the "no forbidden executor imports" step (`:365`):

```yaml
      - name: core builds for wasm32-wasip2 (no_std regression guard)
        # β.7.5: tau-runtime-core must cross-compile to wasm32-wasip2 with
        # no tokio/rustix in the graph. Skill resolution is a port
        # (tau_ports::SkillResolver); tau-pkg lives only in the tokio shell.
        run: |
          rustup target add wasm32-wasip2
          cargo build -p tau-runtime-core --target wasm32-wasip2 --no-default-features
```

- [ ] **Step 2: Validate the workflow YAML locally**

Run: `timeout 30 python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('ok')"`
Expected: `ok`.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit -m "ci: build tau-runtime-core for wasm32-wasip2 in runtime-core-no-std"
```

---

### Task 9: Full verification before PR

**Files:** none (verification only)

- [ ] **Step 1: Workspace-wide affected-crate tests**

Run:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-ports --features test-fixtures
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-core
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-runtime-tokio
```
Expected: ALL PASS.

- [ ] **Step 2: Doctests for the three crates**

Run:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-ports --doc
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-runtime-core --doc
```
Expected: PASS.

- [ ] **Step 3: clippy + fmt on the touched crates**

Run:
```
timeout 30  env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-ports -p tau-runtime-core -p tau-runtime-tokio -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-runtime-core --all-targets
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-runtime-tokio --all-targets
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-ports --all-targets
```
Expected: PASS (no warnings; CI runs `just lint` = `clippy -D warnings`).

- [ ] **Step 4: Re-run THE regression guard once more clean**

Run:
```
timeout 420 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl \
  cargo build -p tau-runtime-core --target wasm32-wasip2 --no-default-features
```
Expected: PASS.

- [ ] **Step 5: Confirm no `tau_pkg` references remain in core src**

Run: `grep -rn "tau_pkg\|tau-pkg" crates/tau-runtime-core/src crates/tau-runtime-core/Cargo.toml`
Expected: NO matches (comments included — the `options.rs:94` comment on `capability_resolver` mentions `tau_pkg::capability_override` which is accurate as a description of the tokio adapter, so leave it; if it reads as a *core* dependency, reword to "tau-runtime-tokio ships ..."). Decide per the actual text.

- [ ] **Step 6: Request code review** (see Handoff — uses superpowers:requesting-code-review before opening the PR).

---

## Self-Review (completed by plan author)

**Spec coverage:**
- New port crates/tau-ports/src/skill_resolver.rs ✓ (Task 1) — trait + ResolvedSkill + SkillResolveError + NoSkillResolver, exported from lib.rs.
- RunOptions.skill_resolver ✓ (Task 2).
- Replace tau_pkg usage in skill_resolve.rs ✓ (Task 3), virtual_tools.rs ✓ (Task 4), stream.rs ✓ (Task 5), options.rs comment ✓ (Task 6/9).
- Remove tau-pkg from core Cargo.toml ✓ (Task 6).
- tau-runtime-tokio TauPkgSkillResolver adapter + wiring ✓ (Task 7).
- NoSkillResolver for guest/tests ✓ (Task 1, in tau-ports, always-compiled).
- wasm32-wasip2 regression guard build ✓ (Task 6 Step 4, Task 9 Step 4) + CI step ✓ (Task 8).
- All-green verification ✓ (Task 9).

**Type consistency:** `SkillResolver::resolve(&self, name: &str) -> Result<ResolvedSkill, SkillResolveError>` is used identically in Tasks 1, 3, 4, 7. `ResolvedSkill { install_path: String, capabilities: Vec<Capability>, system_prompt: String }` consistent. `SkillSpawnRequest.install_path` stays `PathBuf` (built via `PathBuf::from(String)` in Task 3) — no downstream consumer change. `substitute_skill_dir(&[Capability], &str)` consistent in Task 3 (impl + tests + doctest).

**Behavior note (intentional):** the adapter reads SKILL.md eagerly inside `resolve()`, whereas the pre-refactor code skipped the read when the caller supplied a `system_prompt` override. Reference skill packages always ship a valid SKILL.md, so this is a non-issue in practice; the change keeps the port minimal (no override threaded into the port). The Task 9 integration tests (tau-runtime-tokio) are the safety net. If a test regresses on a fixture skill missing SKILL.md, give that fixture a stub SKILL.md rather than re-introducing laziness into the port.

**Parallel-PR note:** a sibling session edits `stream.rs` for the `RunEvent` enum (PR-B). Task 5 touches only the host-fs skill-spawn arm (~`:616`–`:692`, `:786`, `:1050`) — a disjoint region. If this PR is BEHIND at merge, `gh pr update-branch` and resolve the small hunk.
```
