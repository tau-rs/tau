# Bundle effective-capabilities — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `tau build` record real per-agent `effective_capabilities` for agents that declare `[[capabilities]]` overrides, by implementing the two stubbed functions in `crates/tau-pkg/src/bundle/build.rs` to mirror the runtime (compute from the agent's home-package manifest + overrides via `compute_effective`).

**Architecture:** `build()` step 5 already calls `collect_package_caps(...)` then `compute_effective(...)` then `effective_to_bundle(...)` for agents with overrides — but the first and last are stubs returning empty. Replace `collect_package_caps` with a home-package-manifest loader (mirrors `tau-runtime/src/builder.rs:568`, which feeds `package_manifest.capabilities()` to `compute_effective`), and implement `effective_to_bundle` to flatten `EffectiveCapability` entries into the bundle's 5 modeled allow/deny shapes. Fail loudly if an override-agent's home package can't be loaded.

**Tech Stack:** Rust, `tau_pkg::capability_override::{compute_effective, EffectiveCapability}`, `tau_pkg::read_manifest`, `tau_domain::Capability`. Workspace cargo rules in `CLAUDE.md` apply.

**Spec:** `docs/superpowers/specs/2026-05-29-bundle-effective-capabilities-design.md`

---

## Cargo command reference (per `CLAUDE.md`)

Every cargo command MUST be wrapped (substitute your role for `impl`):

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli
```

Never run bare `cargo`. Always `-p <crate>`. Commit identity:

```
git -c user.name="titouanlebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "<msg>"
```

---

## Verified facts (rely on these)

- `read_manifest` is re-exported at the tau-pkg crate root: `crate::read_manifest(path: &Path) -> Result<PackageManifest, ManifestReadError>` (the `path` is the `tau.toml` FILE).
- `ManifestReadError` is at `crate::error::ManifestReadError` (NOT `crate::manifest::`).
- `PackageManifest::capabilities(&self) -> &[Capability]`.
- `EffectiveCapability` (in `crate::capability_override`) has pub fields `source: Capability`, `allow_override: Option<Vec<String>>`, `deny: Vec<String>`, `max_bytes_override: Option<u64>`. It is `#[non_exhaustive]` but build.rs is in the same crate (tau-pkg), so struct-literal construction in tests is allowed.
- `tau_domain::Capability` variants used: `Filesystem(FsCapability::Read{paths}|Write{paths,max_bytes}|Exec{paths})`, `Network(NetCapability::Http{hosts,methods})`, `Process(ProcessCapability::Spawn{commands})`, `Agent(AgentCapability::Spawn{allowed_kinds})`, `Skill(SkillCapability::Spawn{allowed_skills})`. All re-exported under `tau_domain::`.
- Package-manifest capability TOML keys: `kind="fs.read" paths=[]`, `fs.write paths=[] max_bytes=N`, `fs.exec paths=[]`, `net.http hosts=[] methods=[]`, `process.spawn commands=[]`, `agent.spawn allowed_kinds=[]`, `skill.spawn allowed_skills=[]`.
- A minimal valid package `tau.toml` (per `read_manifest` doctest) needs: `name`, `version`, `description`, `authors`, `source`, `kind`, `dependencies`, plus `[[capabilities]]` blocks (which must follow the scalar keys).
- `build.rs`: `packages_root` is a local at line ~88 (`<project_root>/.tau/packages`), in scope through step 5. `packages` (Vec<BundlePackage>, line ~109) and `agents` (line ~157) are `let mut`. The capability block is at lines ~189-201. The two stubs are at `collect_package_caps` (~375) and `effective_to_bundle` (~390).
- Project capability-override TOML shape (for agent fixtures): `[[agents.<id>.capabilities]]` with `kind`, `allow_paths`/`deny_paths` (fs kinds), `allow_hosts`/`deny_hosts` (net), `allow_commands`/`deny_commands` (process), `max_bytes`. `compute_effective` enforces that the override is a NARROWING of the package grant (else `OverrideExpandError` -> already mapped to `BuildError::CapabilityOverrideFailed`).

---

## File Structure

- `crates/tau-pkg/src/bundle/build_error.rs` — add 2 variants (Task 1).
- `crates/tau-cli/src/cmd/build.rs` — map the 2 variants to exit codes + unit-test assertion (Task 1).
- `crates/tau-pkg/src/bundle/build.rs` — implement `effective_to_bundle` (Task 2); replace `collect_package_caps` with `agent_home_package_caps` + rewire the step-5 call site (Task 3); all builder tests (Tasks 2, 3).
- `crates/tau-pkg/src/bundle/reproduce.rs` — reproduce regression test (Task 4).

---

## Task 1: `BuildError` variants + CLI exit codes

**Files:**
- Modify: `crates/tau-pkg/src/bundle/build_error.rs`
- Modify: `crates/tau-cli/src/cmd/build.rs` (`exit_code_for` + `exit_code_mapping_per_spec` test)

- [ ] **Step 1: Add the two variants**

In `crates/tau-pkg/src/bundle/build_error.rs`, add inside the `BuildError` enum (after `ManifestInvalid`, before `WriteFailed`):

```rust
    /// An agent declares capability overrides but its home package (the
    /// `[agents.<id>].package` ref) is absent from the resolved package
    /// set — the effective grant set cannot be computed.
    #[error("agent `{id}` has capability overrides but its home package `{package}` is not in the bundle's package set; run `tau install`")]
    AgentHomePackageMissing {
        /// Agent id with the dangling home-package reference.
        id: String,
        /// The unresolved home-package name.
        package: String,
    },

    /// An agent's home-package manifest could not be read or parsed while
    /// computing its effective capabilities.
    #[error("failed to read home-package manifest for agent `{id}` (package `{package}`): {source}")]
    AgentHomePackageManifest {
        /// Agent id whose home-package manifest failed to load.
        id: String,
        /// The home-package name.
        package: String,
        /// Underlying manifest read/parse error.
        #[source]
        source: crate::error::ManifestReadError,
    },
```

- [ ] **Step 2: Verify tau-pkg compiles**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-pkg`
Expected: clean.

- [ ] **Step 3: Add the failing CLI exit-code assertions**

In `crates/tau-cli/src/cmd/build.rs`, inside the `exit_code_mapping_per_spec` test, append (before the closing brace):

```rust
        // Override-agent home package missing -> install-state -> 3.
        assert_eq!(
            exit_code_for(&BuildError::AgentHomePackageMissing {
                id: "r".into(),
                package: "homepkg".into(),
            }),
            3,
        );
        // Home-package manifest unreadable -> config/parse -> 2.
        assert_eq!(
            exit_code_for(&BuildError::AgentHomePackageManifest {
                id: "r".into(),
                package: "homepkg".into(),
                source: tau_pkg::error::ManifestReadError::NotFound { path: "x".into() },
            }),
            2,
        );
```

(If `tau_pkg::error::ManifestReadError` isn't the public path, grep `crates/tau-pkg/src/lib.rs` for how `error`/`ManifestReadError` is exported and use that path. The variant `NotFound { path: String }` is confirmed in `crates/tau-pkg/src/error.rs`.)

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli --lib cmd::build::tests::exit_code_mapping_per_spec`
Expected: FAIL to compile — `exit_code_for`'s match is non-exhaustive (missing the 2 new variants).

- [ ] **Step 4: Map the variants in `exit_code_for`**

In `crates/tau-cli/src/cmd/build.rs`, in `exit_code_for`:
- Add `AgentHomePackageMissing { .. }` to the install-state arm (the one returning `3`, with `MissingLockfile | PackageNotInstalled`).
- Add `AgentHomePackageManifest { .. }` to the config/parse arm (the one returning `2`, with `ProjectConfig | LockfileLoad | ManifestInvalid | UnknownAgent`).

Resulting arms:

```rust
        BuildError::MissingLockfile
        | BuildError::PackageNotInstalled { .. }
        | BuildError::AgentHomePackageMissing { .. } => 3,
        BuildError::ProjectConfig(_)
        | BuildError::LockfileLoad(_)
        | BuildError::ManifestInvalid(_)
        | BuildError::UnknownAgent { .. }
        | BuildError::AgentHomePackageManifest { .. } => 2,
```

- [ ] **Step 5: Verify**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli --lib cmd::build::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-pkg/src/bundle/build_error.rs crates/tau-cli/src/cmd/build.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(tau-pkg,tau-cli): BuildError variants for agent home-package capability load"
```

---

## Task 2: Implement `effective_to_bundle`

**Files:**
- Modify: `crates/tau-pkg/src/bundle/build.rs` (`effective_to_bundle` body + unit tests in the `tests` module)

- [ ] **Step 1: Write failing unit tests**

Add to the `#[cfg(test)] mod tests` block in `crates/tau-pkg/src/bundle/build.rs`:

```rust
    // Helper: construct an EffectiveCapability (same crate => literal OK).
    fn eff(
        source: tau_domain::Capability,
        allow_override: Option<Vec<&str>>,
        deny: &[&str],
    ) -> crate::capability_override::EffectiveCapability {
        crate::capability_override::EffectiveCapability {
            source,
            allow_override: allow_override.map(|v| v.iter().map(|s| s.to_string()).collect()),
            deny: deny.iter().map(|s| s.to_string()).collect(),
            max_bytes_override: None,
        }
    }

    #[test]
    fn effective_to_bundle_uses_override_allow_and_deny() {
        use tau_domain::{Capability, FsCapability};
        let e = eff(
            Capability::Filesystem(FsCapability::Read { paths: vec!["/data/**".into(), "/tmp/**".into()] }),
            Some(vec!["/data/**"]),
            &["/data/secret/**"],
        );
        let b = effective_to_bundle(&[e]);
        assert_eq!(b.allow_fs_read, vec!["/data/**".to_string()]);
        assert_eq!(b.deny_fs_read, vec!["/data/secret/**".to_string()]);
    }

    #[test]
    fn effective_to_bundle_falls_back_to_source_when_no_override() {
        use tau_domain::{Capability, NetCapability};
        let e = eff(
            Capability::Network(NetCapability::Http { hosts: vec!["api.example.com".into()], methods: vec!["GET".into()] }),
            None,
            &[],
        );
        let b = effective_to_bundle(&[e]);
        assert_eq!(b.allow_net_http, vec!["api.example.com".to_string()]);
        assert!(b.deny_net_http.is_empty());
    }

    #[test]
    fn effective_to_bundle_unions_fs_exec_and_process_spawn_into_exec() {
        use tau_domain::{Capability, FsCapability, ProcessCapability};
        let a = eff(Capability::Filesystem(FsCapability::Exec { paths: vec!["/usr/bin/git".into()] }), None, &[]);
        let c = eff(Capability::Process(ProcessCapability::Spawn { commands: vec!["ls".into()] }), None, &[]);
        let b = effective_to_bundle(&[a, c]);
        assert_eq!(b.allow_exec, vec!["/usr/bin/git".to_string(), "ls".to_string()]);
    }

    #[test]
    fn effective_to_bundle_maps_fs_write_and_agent_spawn() {
        use tau_domain::{Capability, FsCapability, AgentCapability};
        let w = eff(Capability::Filesystem(FsCapability::Write { paths: vec!["/out/**".into()], max_bytes: Some(1024) }), None, &["/out/locked/**"]);
        let s = eff(Capability::Agent(AgentCapability::Spawn { allowed_kinds: vec!["critic".into()] }), None, &[]);
        let b = effective_to_bundle(&[w, s]);
        assert_eq!(b.allow_fs_write, vec!["/out/**".to_string()]);
        assert_eq!(b.deny_fs_write, vec!["/out/locked/**".to_string()]);
        assert_eq!(b.allow_agent_spawn, vec!["critic".to_string()]);
    }

    #[test]
    fn effective_to_bundle_drops_unrepresentable_shapes() {
        use tau_domain::{Capability, SkillCapability};
        let e = eff(Capability::Skill(SkillCapability::Spawn { allowed_skills: vec!["fact-checker".into()] }), None, &[]);
        let b = effective_to_bundle(&[e]);
        // No bundle field exists for skill.spawn => the result is fully empty.
        assert!(b.is_empty(), "skill.spawn must be dropped: {b:?}");
    }
```

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::build::tests::effective_to_bundle`
Expected: FAIL — the stub returns `Default` (all-empty), so the first four tests fail their non-empty assertions. (`drops_unrepresentable_shapes` passes against the stub too — that's fine, it stays green after implementation.)

- [ ] **Step 2: Implement `effective_to_bundle`**

Replace the stub body of `effective_to_bundle` in `crates/tau-pkg/src/bundle/build.rs` (keep the function name + signature) with the real flattening. For each entry the effective allow-list is `allow_override` when present, else the source variant's own field; deny is `e.deny`; shapes with no bundle field are skipped via the catch-all arm:

```rust
fn effective_to_bundle(
    eff: &[crate::capability_override::EffectiveCapability],
) -> BundleEffectiveCapabilities {
    use tau_domain::{AgentCapability, Capability, FsCapability, NetCapability, ProcessCapability};
    let mut out = BundleEffectiveCapabilities::default();
    for e in eff {
        // Effective allow-list = narrowed override if present, else the
        // source variant's own allow field. Deny = e.deny. Shapes with
        // no BundleEffectiveCapabilities representation (skill.spawn,
        // task_list, plan, custom) and sub-fields without representation
        // (fs.write max_bytes, net.http methods) are dropped — spec section 6.
        match &e.source {
            Capability::Filesystem(FsCapability::Read { paths }) => {
                out.allow_fs_read
                    .extend(e.allow_override.clone().unwrap_or_else(|| paths.clone()));
                out.deny_fs_read.extend(e.deny.clone());
            }
            Capability::Filesystem(FsCapability::Write { paths, .. }) => {
                out.allow_fs_write
                    .extend(e.allow_override.clone().unwrap_or_else(|| paths.clone()));
                out.deny_fs_write.extend(e.deny.clone());
            }
            Capability::Filesystem(FsCapability::Exec { paths }) => {
                out.allow_exec
                    .extend(e.allow_override.clone().unwrap_or_else(|| paths.clone()));
                out.deny_exec.extend(e.deny.clone());
            }
            Capability::Process(ProcessCapability::Spawn { commands }) => {
                out.allow_exec
                    .extend(e.allow_override.clone().unwrap_or_else(|| commands.clone()));
                out.deny_exec.extend(e.deny.clone());
            }
            Capability::Network(NetCapability::Http { hosts, .. }) => {
                out.allow_net_http
                    .extend(e.allow_override.clone().unwrap_or_else(|| hosts.clone()));
                out.deny_net_http.extend(e.deny.clone());
            }
            Capability::Agent(AgentCapability::Spawn { allowed_kinds }) => {
                out.allow_agent_spawn
                    .extend(e.allow_override.clone().unwrap_or_else(|| allowed_kinds.clone()));
                out.deny_agent_spawn.extend(e.deny.clone());
            }
            // skill.spawn / task_list / plan / custom: no bundle field (spec section 6).
            _ => {}
        }
    }
    out
}
```

Also replace the old stub doc-comment (the one mentioning "MVP stub: returns the default") with a one-line doc consistent with the spec.

- [ ] **Step 3: Run tests -> PASS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::build::tests::effective_to_bundle`
Expected: all 5 PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/tau-pkg/src/bundle/build.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(tau-pkg): flatten EffectiveCapability into BundleEffectiveCapabilities"
```

---

## Task 3: Replace `collect_package_caps` with home-package loader + rewire

**Files:**
- Modify: `crates/tau-pkg/src/bundle/build.rs` (`collect_package_caps` -> `agent_home_package_caps`; step-5 call site; build-level tests)

- [ ] **Step 1: Write failing build-level tests**

Add to the `#[cfg(test)] mod tests` block in `crates/tau-pkg/src/bundle/build.rs`. This fixture installs a home package whose manifest declares several capabilities, with an agent that overrides `fs.read` only:

```rust
    /// Project with an override-agent `r` whose home package `homepkg`
    /// is installed with a manifest granting fs.read + net.http +
    /// fs.exec + process.spawn + skill.spawn. The agent narrows fs.read.
    fn override_agent_project(tmp: &std::path::Path) {
        std::fs::write(
            tmp.join("tau.toml"),
            r#"
[project]
name = "capproj"
version = "0.1.0"

[agents.r]
display_name = "R"
package = "homepkg@^0.1"
llm_backend = "anthropic"

[agents.r.prompt]
system = "you are r"

[[agents.r.capabilities]]
kind = "fs.read"
allow_paths = ["/data/**"]
deny_paths = ["/data/secret/**"]
"#,
        )
        .unwrap();
        // Lockfile names homepkg.
        std::fs::write(
            tmp.join("tau.lock"),
            r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"

[[package]]
name = "homepkg"
active_version = "0.1.0"
source = "https://example.com/homepkg.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000001"
installed_at = "2024-01-01T00:00:00Z"
"#,
        )
        .unwrap();
        // Installed package manifest with the granted capabilities.
        let pkg_dir = tmp.join(".tau/packages/homepkg/0.1.0");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("tau.toml"),
            r#"name = "homepkg"
version = "0.1.0"
description = "home package"
authors = ["a <a@example.com>"]
source = "https://example.com/homepkg.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "fs.read"
paths = ["/data/**", "/tmp/**"]

[[capabilities]]
kind = "net.http"
hosts = ["api.example.com"]
methods = ["GET"]

[[capabilities]]
kind = "fs.exec"
paths = ["/usr/bin/git"]

[[capabilities]]
kind = "process.spawn"
commands = ["ls"]

[[capabilities]]
kind = "skill.spawn"
allowed_skills = ["critic"]
"#,
        )
        .unwrap();
    }

    fn read_agent_caps(tmp: &std::path::Path) -> crate::bundle::manifest::BundleEffectiveCapabilities {
        let artifact = build(opts(tmp)).expect("build succeeds");
        let s = std::fs::read_to_string(&artifact.path).unwrap();
        let m = crate::bundle::manifest::BundleManifest::parse_str(&s).unwrap();
        assert_eq!(m.agents.len(), 1);
        m.agents[0].effective_capabilities.clone()
    }

    #[test]
    fn build_records_narrowed_fs_read_for_override_agent() {
        let tmp = tempdir().unwrap();
        override_agent_project(tmp.path());
        let caps = read_agent_caps(tmp.path());
        assert_eq!(caps.allow_fs_read, vec!["/data/**".to_string()]);
        assert_eq!(caps.deny_fs_read, vec!["/data/secret/**".to_string()]);
    }

    #[test]
    fn build_records_unoverridden_source_caps() {
        let tmp = tempdir().unwrap();
        override_agent_project(tmp.path());
        let caps = read_agent_caps(tmp.path());
        // net.http had no override => source hosts; exec unions fs.exec + process.spawn.
        assert_eq!(caps.allow_net_http, vec!["api.example.com".to_string()]);
        assert!(caps.allow_exec.contains(&"/usr/bin/git".to_string()));
        assert!(caps.allow_exec.contains(&"ls".to_string()));
    }

    #[test]
    fn build_drops_skill_spawn_from_bundle() {
        let tmp = tempdir().unwrap();
        override_agent_project(tmp.path());
        let s = std::fs::read_to_string(&build(opts(tmp.path())).unwrap().path).unwrap();
        assert!(!s.contains("skill"), "skill.spawn must not appear in the bundle: {s}");
    }

    #[test]
    fn build_effective_caps_is_reproducible() {
        let tmp = tempdir().unwrap();
        override_agent_project(tmp.path());
        let a = build(opts(tmp.path())).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let b = build(opts(tmp.path())).unwrap();
        assert_eq!(a.sha256, b.sha256, "effective-caps build must be reproducible");
    }

    #[test]
    fn build_errors_when_override_agent_home_package_missing() {
        let tmp = tempdir().unwrap();
        override_agent_project(tmp.path());
        // Rewrite tau.toml so the agent references a package not in the lockfile.
        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "capproj"
version = "0.1.0"

[agents.r]
display_name = "R"
package = "ghost@^0.1"
llm_backend = "anthropic"

[agents.r.prompt]
system = "you are r"

[[agents.r.capabilities]]
kind = "fs.read"
allow_paths = ["/data/**"]
"#,
        )
        .unwrap();
        match build(opts(tmp.path())).unwrap_err() {
            BuildError::AgentHomePackageMissing { id, package } => {
                assert_eq!(id, "r");
                assert_eq!(package, "ghost");
            }
            other => panic!("expected AgentHomePackageMissing, got {other:?}"),
        }
    }

    #[test]
    fn build_errors_when_home_package_manifest_unreadable() {
        let tmp = tempdir().unwrap();
        override_agent_project(tmp.path());
        // Remove the installed package manifest but keep the dir (so the
        // step-3 install check still passes).
        std::fs::remove_file(tmp.path().join(".tau/packages/homepkg/0.1.0/tau.toml")).unwrap();
        match build(opts(tmp.path())).unwrap_err() {
            BuildError::AgentHomePackageManifest { id, package, .. } => {
                assert_eq!(id, "r");
                assert_eq!(package, "homepkg");
            }
            other => panic!("expected AgentHomePackageManifest, got {other:?}"),
        }
    }
```

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::build::tests::build_records`
Expected: FAIL — `collect_package_caps` returns `[]`, so `compute_effective` gets no source caps and produces nothing, leaving `effective_capabilities` empty; the `allow_fs_read` assertion fails. The two error tests also fail (no error path yet).

- [ ] **Step 2: Replace `collect_package_caps` with `agent_home_package_caps`**

In `crates/tau-pkg/src/bundle/build.rs`, replace the entire `collect_package_caps` function (signature + body + doc) with:

```rust
/// Load the agent's home-package manifest and return its declared
/// capability grants — the same source the runtime feeds to
/// `compute_effective` (see tau-runtime builder.rs). `home_package` is
/// the `<name>` half of `[agents.<id>].package`; its resolved version
/// comes from the gathered `packages` list. Fails loudly rather than
/// returning empty so the bundle never silently under-records grants.
fn agent_home_package_caps(
    home_package: &str,
    packages: &[BundlePackage],
    packages_root: &std::path::Path,
    agent_id: &str,
) -> Result<Vec<tau_domain::Capability>, BuildError> {
    let pkg = packages.iter().find(|p| p.name == home_package).ok_or_else(|| {
        BuildError::AgentHomePackageMissing {
            id: agent_id.to_owned(),
            package: home_package.to_owned(),
        }
    })?;
    let manifest_path = packages_root
        .join(&pkg.name)
        .join(pkg.version.to_string())
        .join("tau.toml");
    let manifest = crate::read_manifest(&manifest_path).map_err(|source| {
        BuildError::AgentHomePackageManifest {
            id: agent_id.to_owned(),
            package: home_package.to_owned(),
            source,
        }
    })?;
    Ok(manifest.capabilities().to_vec())
}
```

`BundlePackage` is already imported in build.rs (used throughout). If `BundlePackage` isn't in scope by that bare name in the fn, use the fully-qualified `crate::bundle::manifest::BundlePackage` as elsewhere in the file.

- [ ] **Step 3: Rewire the step-5 call site**

In `build()` step 5, replace the capability block (currently):

```rust
        let effective_capabilities = if entry.capability_overrides.is_empty() {
            BundleEffectiveCapabilities::default()
        } else {
            let package_caps = collect_package_caps(&packages, &required_tools)?;
            let eff = crate::capability_override::compute_effective(
                &package_caps,
                &entry.capability_overrides,
            )
            .map_err(|source| BuildError::CapabilityOverrideFailed { id: id.clone(), source })?;
            effective_to_bundle(&eff)
        };
```

with:

```rust
        let effective_capabilities = if entry.capability_overrides.is_empty() {
            BundleEffectiveCapabilities::default()
        } else {
            // Mirror the runtime (tau-runtime builder.rs): compute the
            // agent's effective grant from its HOME-package manifest +
            // the project overrides. The home package name is the `<name>`
            // half of `[agents.<id>].package`.
            let (home_pkg, _req) = crate::project::agent::parse_package_ref(&entry.package)
                .map_err(|_| BuildError::AgentHomePackageMissing {
                    id: id.clone(),
                    package: entry.package.clone(),
                })?;
            let package_caps =
                agent_home_package_caps(&home_pkg, &packages, &packages_root, id)?;
            let eff = crate::capability_override::compute_effective(
                &package_caps,
                &entry.capability_overrides,
            )
            .map_err(|source| BuildError::CapabilityOverrideFailed { id: id.clone(), source })?;
            effective_to_bundle(&eff)
        };
```

Notes:
- `parse_package_ref` is `pub(crate)` in `crate::project::agent` (used by the agent-slicing code).
- `packages_root` is the local from step 3; it's in scope here. `packages` is the gathered Vec (full set at step 5, before slicing).
- Leave `required_tools` as-is — it is still used later in step 5 to populate `BundleAgent.required_tools`.

- [ ] **Step 4: Run tests -> PASS**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::build::`
Expected: all new `build_records_*` / `build_drops_*` / `build_effective_caps_is_reproducible` / `build_errors_*` PASS, and all pre-existing build tests still pass.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/bundle/build.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "feat(tau-pkg): compute agent effective_capabilities from home-package manifest"
```

---

## Task 4: Reproduce regression + final verification + PR

**Files:**
- Modify: `crates/tau-pkg/src/bundle/reproduce.rs` (one regression test)

- [ ] **Step 1: Add a reproduce regression test**

In the `#[cfg(test)] mod tests` block of `crates/tau-pkg/src/bundle/reproduce.rs`, add a test that builds an override-agent project (installed home-package manifest granting fs.read; agent narrows it) and verifies it reproduces. Reuse the existing `ropts` helper:

```rust
    #[test]
    fn verify_reproducible_with_effective_caps() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("tau.toml"),
            r#"
[project]
name = "caprepro"
version = "0.1.0"

[agents.r]
display_name = "R"
package = "homepkg@^0.1"
llm_backend = "anthropic"

[agents.r.prompt]
system = "you are r"

[[agents.r.capabilities]]
kind = "fs.read"
allow_paths = ["/data/**"]
deny_paths = ["/data/secret/**"]
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("tau.lock"),
            "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n\n[[package]]\nname = \"homepkg\"\nactive_version = \"0.1.0\"\nsource = \"https://example.com/homepkg.git\"\n\n[[package.versions]]\nversion = \"0.1.0\"\nresolved_commit = \"0000000000000000000000000000000000000001\"\ninstalled_at = \"2024-01-01T00:00:00Z\"\n",
        )
        .unwrap();
        let pkg_dir = root.join(".tau/packages/homepkg/0.1.0");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("tau.toml"),
            "name = \"homepkg\"\nversion = \"0.1.0\"\ndescription = \"x\"\nauthors = [\"a <a@example.com>\"]\nsource = \"https://example.com/homepkg.git\"\nkind = \"tool\"\ndependencies = []\n\n[[capabilities]]\nkind = \"fs.read\"\npaths = [\"/data/**\", \"/tmp/**\"]\n",
        )
        .unwrap();

        let artifact = build(BuildOptions {
            project_root: root.to_path_buf(),
            target: TargetTriple::host(),
            output_path: None,
            agent_filter: None,
        })
        .unwrap();
        // Sanity: the bundle actually recorded narrowed caps.
        let m = BundleManifest::from_path(&artifact.path).unwrap();
        assert_eq!(m.agents[0].effective_capabilities.allow_fs_read, vec!["/data/**".to_string()]);

        let report = verify_reproducible(ropts(artifact.path, root)).expect("repro ran");
        assert!(report.reproducible, "effective-caps bundle must reproduce; diffs={:?}", report.diffs);
    }
```

(Confirm the test module already imports `build`, `BuildOptions`, `TargetTriple`, `BundleManifest`, `tempdir`, `verify_reproducible`, `ropts` — it does, per the agent-slicing work. If `BundleManifest` isn't imported, add `use crate::bundle::manifest::BundleManifest;`.)

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --lib bundle::reproduce::tests::verify_reproducible_with_effective_caps`
Expected: PASS.

- [ ] **Step 2: Commit**

```bash
git add crates/tau-pkg/src/bundle/reproduce.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "test(tau-pkg): effective-caps bundle round-trips through verify_reproducible"
```

- [ ] **Step 3: Full suites + fmt + clippy + doctests**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli
timeout 30  env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-pkg -p tau-cli -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-pkg -p tau-cli --all-targets
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-pkg
```
Expected: all green / clean. If fmt fails, run without `--check` and commit the formatting as a `style(...)` commit.

- [ ] **Step 4: Push + PR**

Per `CLAUDE.md`: do NOT plain `git push` from the agent runtime. If the local deep gate (Podman) is available, use `scripts/agent-push.sh`. Otherwise push `--no-verify` and document it (CI's required checks are the Linux gate):

```bash
git push --no-verify -u origin feat/bundle-caps
gh pr create --title "feat(tau-pkg): record real per-agent effective_capabilities (Phase 2 C.2.3)" --body "$(cat <<'EOF'
## Summary
- Closes the two build.rs capability stubs so tau build records real effective_capabilities for agents with [[capabilities]] overrides.
- Mirrors the runtime: computes from the agent's home-package manifest + overrides via compute_effective, then flattens into the bundle's 5 modeled allow/deny shapes.
- Fails loudly (AgentHomePackageMissing -> exit 3, AgentHomePackageManifest -> exit 2) instead of silently recording empty caps.

## Known limitations (spec section 6)
- Unrepresentable shapes (skill.spawn, task_list, plan, custom) and sub-fields (fs.write max_bytes, net.http methods) are dropped from the bundle record; the runtime still enforces them. Schema extension deferred to sub-project D.
- No-override agents still record no effective_capabilities (unchanged trigger).

## Test plan
- [ ] cargo test -p tau-pkg (effective_to_bundle unit + build-level + reproduce)
- [ ] cargo test -p tau-cli (exit-code mapping)
- [ ] cargo test --doc -p tau-pkg, fmt, clippy
- [ ] CI required checks

Generated with Claude Code
EOF
)"
```

---

## Self-Review notes

- **Spec coverage:** section 2 mirror-runtime -> Task 3 call site; section 2 overrides-only trigger -> unchanged `if entry.capability_overrides.is_empty()`; section 2 drop-unrepresentable -> `effective_to_bundle` catch-all arm + Task 2/3 drop tests; section 2 fail-loudly -> Task 1 variants + Task 3 error tests; section 4.1 loader -> Task 3; section 4.2 flatten -> Task 2; section 4.3 determinism -> `build_effective_caps_is_reproducible` + Task 4 reproduce test; section 4.4 variants+exit codes -> Task 1; section 4.5 call site -> Task 3; section 5 tests -> Tasks 1-4. Covered.
- **Type consistency:** `agent_home_package_caps(home_package, packages, packages_root, agent_id)` defined + called in Task 3. `effective_to_bundle(&[EffectiveCapability]) -> BundleEffectiveCapabilities` Task 2, called Task 3. `BuildError::AgentHomePackageMissing { id, package }` + `AgentHomePackageManifest { id, package, source: crate::error::ManifestReadError }` consistent across Tasks 1, 3. `EffectiveCapability { source, allow_override, deny, max_bytes_override }` consistent.
- **Verify-at-impl-time (flagged inline):** public path of `ManifestReadError` for the tau-cli test; `BundleManifest` import in reproduce tests. Each has a grep/fallback.
