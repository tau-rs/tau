# Ignored doctests inventory — round 2

**Source:** `git grep '```ignore' -- crates/{tau-plugin-protocol,tau-plugin-sdk,tau-runtime,tau-domain,tau-pkg}/src/` on 2026-05-25.
**Spec:** `docs/superpowers/specs/2026-05-25-doctests-round-2-design.md`.
**Plan:** `docs/superpowers/plans/2026-05-25-doctests-round-2.md`.

## Categories

- **A — pure activation:** body is correct; flip `ignore` → ` ``` `.
- **B — needs hidden setup:** body is correct but references types/values that need `# ` hidden preamble.
- **C — placeholder:** body is `/* ... */` or stale-reference. Rewrite or delete.
- **D — `no_run`:** activation would require forbidden side-effects. Convert to `no_run`, add justification.

## Items

| # | Crate | File:line | Item | Category | Strategy |
|---|---|---|---|---|---|
| 1 | tau-plugin-protocol | error.rs:13 | `ProtocolError` | A | flip to executed fence |
| 2 | tau-plugin-protocol | error.rs:83 | `RpcErrorEnvelope` | A* | flip + replace struct literal with `RpcErrorEnvelope::new()` (`#[non_exhaustive]` prevents external struct construction) |
| 3 | tau-plugin-protocol | frame.rs:27 | `Frame` enum (Notification example) | A* | flip + replace `params: vec![]` with `params: vec![0x90]` (empty MessagePack array) |
| 4 | tau-plugin-sdk | configure.rs:76 | `Configure` trait | A | flip to executed fence |
| 5 | tau-plugin-sdk | runners/llm_backend.rs:122 | `run_llm_backend_with_config` | B | no_run + hidden `MyPlugin` impl |
| 6 | tau-plugin-sdk | runners/tool.rs:125 | `run_tool_with_config` | B | no_run + hidden `MyTool` impl |
| 7 | tau-runtime | builder.rs:405 | `Runtime::run_streaming` | B | hidden MockLlmBackend + Runtime fixture |
| 8 | tau-runtime | builder.rs:464 | `Runtime::run_streaming_with_history` | B | same fixture shape as #7 |
| 9 | tau-runtime | error.rs:58 | `BuildError` | C | replace placeholder with `Runtime::builder().build()` + assert NoLlmBackend |
| 10 | tau-domain | message.rs:74 | `Message` struct | B | replace `#[non_exhaustive]` struct literal with `Message::new()` constructor; flip to executed fence |
| 11 | tau-domain | package/capability.rs:20 | `Capability` enum | C | rewrite body: show `Capability::Custom { .. }` (constructable variant) + `.required_shape()` assert; `FsCapability::Read` variant is `#[non_exhaustive]` — not constructable externally |
| 12 | tau-domain | package/capability.rs:70 | `FsCapability` enum | C | rewrite body: show `CapabilityShape::FilesystemRead` (the shape this verb maps to); all variants are variant-level `#[non_exhaustive]` — no external construction path |
| 13 | tau-domain | package/capability.rs:104 | `NetCapability` enum | C | rewrite body: show `CapabilityShape::NetworkHttp`; same constraint as row 12 |
| 14 | tau-domain | package/capability.rs:129 | `ProcessCapability` enum | C | rewrite body: show `CapabilityShape::ProcessExec`; same constraint as row 12 |
| 15 | tau-domain | package/capability.rs:149 | `AgentCapability` enum | C | rewrite body: show `CapabilityShape::AgentSpawn`; same constraint as row 12 |
| 16 | tau-domain | package/capability.rs:175 | `SkillCapability` enum | C | rewrite body: show `CapabilityShape::SkillSpawn`; same constraint as row 12 |
| 17 | tau-domain | package/manifest.rs:17 | `PackageDep` struct | C | rewrite body: show `PackageName` + `VersionReq` construction (field types); `PackageDep` is `#[non_exhaustive]` with no public constructor |
| 18 | tau-domain | package/manifest.rs:45 | `PackageId` struct | B | replace `#[non_exhaustive]` struct literal with `PackageId::new()` constructor; flip to executed fence |
| 19 | tau-domain | package/manifest.rs:507 | `UncheckedManifest::validate` | D | no_run; `UncheckedManifest` is `#[non_exhaustive]` with no public constructor; shows call shape with `unimplemented!()` placeholder |
| 20 | tau-domain | package/plugin.rs:96 | `PluginKind` enum | A | flip to executed fence; `from_str` + `to_string` work from outside the crate |
| 21 | tau-domain | package/plugin.rs:153 | `PluginManifest` struct | B | replace `toml::from_str` (requires serde feature) with `PluginManifest::new()` constructor; flip to executed fence |
| 22 | tau-pkg | install.rs:152 | `install` fn | D | no_run — shells out to `git clone` (subprocess + network) |
| 23 | tau-pkg | install.rs:769 | `uninstall` fn | D | no_run — acquires file lock + modifies on-disk install state |
| 24 | tau-pkg | lockfile.rs:135 | `LockedPackage` struct | B | parse via `LockFile::from_toml_str`; assert `pkg.name.as_str()` |
| 25 | tau-pkg | lockfile.rs:192 | `LockedPlugin` struct | B | construct via `LockedPlugin::new(PluginManifest::new(…), …)`; assert `manifest.bin` |
| 26 | tau-pkg | lockfile.rs:317 | `LockedVersion` struct | B | parse via `LockFile::from_toml_str` with `[[package.versions]]`; assert `ver.version` |
| 27 | tau-pkg | lockfile.rs:538 | `LockFile::save` | B | hidden `tempfile::tempdir()` + assert `path.exists()` |
| 28 | tau-pkg | lockfile.rs:587 | `LockFile::find` | A | pure — `LockFile::default()` + `find` on empty; assert `None` |
| 29 | tau-pkg | lockfile.rs:608 | `LockFile::upsert` | B | parse `LockedPackage` via `from_toml_str`; call `upsert`; assert `len == 1` |
| 30 | tau-pkg | lockfile.rs:632 | `LockFile::remove` | A | pure — `LockFile::default()` + `remove` on empty; assert `None` |
| 31 | tau-pkg | manifest.rs:41 | `read_manifest` fn | B | write minimal `tau.toml` to `tempfile::tempdir()`; assert `name` |
| 32 | tau-pkg | registry.rs:25 | `list` fn | B | `Scope::new_project(tmp.path())`; assert empty list |
| 33 | tau-pkg | registry.rs:46 | `get` fn | B | `Scope::new_project(tmp.path())`; assert `None` |
| 34 | tau-pkg | scope.rs:262 | `Scope` struct | B | `Scope::new_project(tmp.path())`; assert `kind == Project` |
| 35 | tau-pkg | scope.rs:296 | `Scope::resolve` | B | create `.tau/` in tempdir; `Scope::resolve` detects it; assert `kind == Project` |
| 36 | tau-pkg | scope.rs:326 | `Scope::global` | B | set `TAU_HOME` to tempdir path via `std::env::set_var`; assert `kind == Global` |
| 37 | tau-pkg | scope.rs:400 | `Scope::new_project` | B | `Scope::new_project(tmp.path())`; assert `state_path` ends with `.tau` |
| 38 | tau-pkg | tree_hash.rs:86 | `tree_hash` fn | B | write one file to tempdir; assert hash len == 64 |
| 39 | tau-pkg | update.rs:28 | `UpdateError` enum | A | define `fn describe(e: &UpdateError)` with `_ =>` catch-all; compiles |
| 40 | tau-pkg | update.rs:94 | `UpdateResult` struct | A | define `fn log_update(r: &UpdateResult)` referencing `from_version`/`to_version`; compiles |

## Status log

(Updated by Tasks 2–6 as each row is activated.)

- 2026-05-25 — rows 1, 2, 3 → activated (PR-A).
- 2026-05-25 — row 4 → activated; rows 5+6 → no_run with hidden fixture (PR-B).
- 2026-05-25 — rows 7, 8, 9 → activated (PR-C, established Runtime-flow fixture pattern via `tau_ports::fixtures::MockLlmBackend`).
- 2026-05-25 — rows 10-21 → activated/no_run per row classification (PR-D).
- 2026-05-25 — rows 22-40 → activated/no_run per row classification (PR-E).
