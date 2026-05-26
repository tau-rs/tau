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
| 22 | tau-pkg | install.rs:152 | TBD-by-Task-6 | ? | classify in Task 6 |
| 23 | tau-pkg | install.rs:769 | TBD-by-Task-6 | ? | classify in Task 6 |
| 24 | tau-pkg | lockfile.rs:135 | TBD-by-Task-6 | ? | classify in Task 6 |
| 25 | tau-pkg | lockfile.rs:192 | TBD-by-Task-6 | ? | classify in Task 6 |
| 26 | tau-pkg | lockfile.rs:317 | TBD-by-Task-6 | ? | classify in Task 6 |
| 27 | tau-pkg | lockfile.rs:538 | TBD-by-Task-6 | ? | classify in Task 6 |
| 28 | tau-pkg | lockfile.rs:587 | TBD-by-Task-6 | ? | classify in Task 6 |
| 29 | tau-pkg | lockfile.rs:608 | TBD-by-Task-6 | ? | classify in Task 6 |
| 30 | tau-pkg | lockfile.rs:632 | TBD-by-Task-6 | ? | classify in Task 6 |
| 31 | tau-pkg | manifest.rs:41 | TBD-by-Task-6 | ? | classify in Task 6 |
| 32 | tau-pkg | registry.rs:25 | TBD-by-Task-6 | ? | classify in Task 6 |
| 33 | tau-pkg | registry.rs:46 | TBD-by-Task-6 | ? | classify in Task 6 |
| 34 | tau-pkg | scope.rs:262 | TBD-by-Task-6 | ? | classify in Task 6 |
| 35 | tau-pkg | scope.rs:296 | TBD-by-Task-6 | ? | classify in Task 6 |
| 36 | tau-pkg | scope.rs:326 | TBD-by-Task-6 | ? | classify in Task 6 |
| 37 | tau-pkg | scope.rs:400 | TBD-by-Task-6 | ? | classify in Task 6 |
| 38 | tau-pkg | tree_hash.rs:86 | TBD-by-Task-6 | ? | classify in Task 6 |
| 39 | tau-pkg | update.rs:28 | TBD-by-Task-6 | ? | classify in Task 6 |
| 40 | tau-pkg | update.rs:94 | TBD-by-Task-6 | ? | classify in Task 6 |

## Status log

(Updated by Tasks 2–6 as each row is activated.)

- 2026-05-25 — rows 1, 2, 3 → activated (PR-A).
- 2026-05-25 — row 4 → activated; rows 5+6 → no_run with hidden fixture (PR-B).
- 2026-05-25 — rows 7, 8, 9 → activated (PR-C, established Runtime-flow fixture pattern via `tau_ports::fixtures::MockLlmBackend`).
- 2026-05-25 — rows 10-21 → activated/no_run per row classification (PR-D).
