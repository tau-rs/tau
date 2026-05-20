# Tau roadmap

This document tracks current phase, near-term priorities, and
explicit out-of-scope items. Updated at phase transitions per PG1 and
PG4.

For per-issue tracking, see [GitHub
Issues](https://github.com/LEBOCQTitouan/tau/issues).

## Current phase: 2 — tau as a compiled language for agentic workflows

**Phase 1 complete** (2026-05-17). All Tier 1–4 priorities shipped,
including serve mode (§15, ADR-0033). See [Phase 2](#phase-2--tau-as-a-compiled-language-for-agentic-workflows) for active work.

**Status:** Phase 1 priority 3 (first real Tool plugins: fs-read +
shell) shipped 2026-04-30. Tier 1 fully complete: plugin loading
mechanism (priority 1), three real LLM-backend plugins (priority 2),
and two real Tool plugins (priority 3) with end-to-end capability
enforcement. **Tier 2 fully complete** as of 2026-05-01: priorities
4 (capability override), 5 (transitive dependency resolution), 6
(tool-args schema validation), 7 (tau update/verify/uninstall), and
8 (streaming LLM responses) all shipped, closing the ADR-0007 §4,
§5, §1, ADR-0006 §3, and ADR-0006 §5 reservations. Tier 3 priority
11 (REPL persistence) shipped 2026-05-02 — closing ADR-0006 §16 +
ADR-0007 §11. Tier 3 priority 12 (sandboxing) shipped 2026-05-03 —
closing ADR-0006 §13 + Constitution G12; ADR-0014 records the design.
Sub-project A from the priority-12 followups (sandbox activation by
default — declarative requirements + adapter registry + resolver)
shipped 2026-05-04 — ADR-0015 records the design; sandboxing is
now ON by default for every plugin spawn, with `--no-sandbox` /
`[sandbox] required_tier = "none"` as the explicit opt-out.
Sub-project B (plugin compatibility verification + Layer 2 install-time
cross-check) shipped 2026-05-04 — ADR-0016 records the design; the 5
real plugins now declare `[sandbox] required_tier = "strict"` in their
manifests, install carries Layer 2 cross-check at step 8.7, and a new
`tau-plugin-compat` crate hosts per-plugin Layer 3 verification (Layer 4
e2e gated `#[ignore]` pending sub-project D). Sub-project D (end-to-end
landlock CI integration + port-aware Layer 4 driver) shipped 2026-05-06
— ADR-0017 records the design; 9 real-kernel e2e tests + 2 runtime e2e
tests pass on Linux CI; two priority-12 native-adapter bugs caught and
fixed (Execute access flag + binary-parent auto-add); GitHub Actions
caching via `Swatinem/rust-cache` + composite action. The 10 Layer 4
plugin-compat `#[ignore]`'s from sub-project B persist (plugin
startup-IO cataloging deferred to a D-followup or sub-project F).
Phase 2 (tau as a
compiled language for agentic workflows) is now unblocked. Remaining
Tier 3: priorities 9 (multi-agent orchestration) and 10 (workflow
runner).

| # | Sub-project | Produces | Merged |
|---|---|---|---|
| 1 | Plugin loading mechanism ✅ | Out-of-process IPC over MessagePack-RPC; tau-plugin-protocol + tau-plugin-sdk crates; plugin_host module in tau-runtime; tau-pkg build-on-install; debug-tier subcommands; echo-llm + echo-tool toy plugins | 2026-04-28 |
| 2a | Anthropic LLM-backend plugin ✅ | First real LLM-backend plugin: Anthropic Claude Messages API client at `crates/tau-plugins/anthropic/`; day-1 streaming + tool-use; cassette-replay test harness + env-gated live smoke; in-plugin retry honoring Retry-After; ConfigError::InvalidEnvVar SDK amendment | 2026-04-29 |
| 2b | Ollama LLM-backend plugin ✅ | Second real LLM-backend plugin: Ollama (local LLM runner) at `crates/tau-plugins/ollama/`; native `/api/chat` over NDJSON streaming (~50 LOC hand-rolled, no eventsource-stream); optional bearer-token auth; cassette-replay test harness duplicated from Anthropic; in-plugin retry honoring 503-on-model-load case; 404 errors include `ollama pull` remediation hint | 2026-04-29 |
| 2c | OpenAI plugin + supporting infrastructure ✅ | Third real LLM-backend plugin: OpenAI Chat Completions client at `crates/tau-plugins/openai/`; SSE streaming, real `tool_call_id` round-trip, full `tool_choice` round-trip. Plus `crates/tau-plugin-test-support/` (rule-of-three refactor of cassette replayer) and `crates/tau-plugin-conformance/` (parameterized behavioral test suite, deferred from ADR-0008 §17). All 3 plugins migrated to typed `LlmError` variants. ADR-0009 Accepted. | 2026-04-29 |
| 3 | First real Tool plugins (fs-read + shell) ✅ | Two minimal Tool plugins demonstrating the kernel's capability check end-to-end. `fs-read` enforces `FsCapability::Read.paths` globs; `shell` enforces `ProcessCapability::Spawn.commands` allow-list (wall-clock timeout, 1 MiB output cap, kill+drain on timeout, no env inheritance, no stdin). Closed two infrastructure gaps in the same sub-project: `tool.describe_capabilities` wire method (Gap 1: plugin-declared capabilities now surface to the kernel for IPC tools); `SessionContext.granted_capabilities` (Gap 2: agent grants flow to plugin processes for finer-grained scope checks). Trust model: unsandboxed v0.1; sandboxing deferred to Tier 3 priority 12. | 2026-04-30 |
| 4 | Capability override implementation ✅ | Tier 2 priority 4 — realizes ADR-0007 §4 reservation. Project tau.toml `[[agents.<id>.capabilities]]` narrows but never expands package manifest grants. `tau-runtime::capability_override` module (semantic glob-subset analyzer + `compute_effective`); `RunOptions.project_override` flows from tau-cli through `Runtime::run`; `SessionContext.deny_entries` channel; `DenyEntry` type; fs-read + shell plugins honor deny-after-allow (deny wins per spec §9). Validation at parse time AND every runtime load (fail-closed both places). New `tau list agents --capabilities` audit surface. New typed errors `ProjectConfigError::CapabilityOverrideExpands` and `RuntimeError::CapabilityOverrideExpands`. Telemetry event `runtime.capability_override_rejected`. No new CI jobs (23 required checks unchanged). | 2026-04-30 |
| 5 | Transitive dependency resolution ✅ | Tier 2 priority 5 — realizes ADR-0007 §5 reservation. New `tau-pkg::source_list` (git ls-remote tag enumeration + rev-pinned shallow read) and `tau-pkg::resolve` (three-phase resolver: group / conflict / pick highest-compatible). New `tau resolve` subcommand. Schema upgrade: `[[agents.<id>.requires.tools]]` typed entries with `name + source + version`; bare strings rejected at parse. Lazy resolve at `tau run`/`tau chat` with `--no-install` opt-out emitting copy-pasteable install hints. npm-style progress output (one line per phase, JSON event stream). New typed `ResolveError`, `SourceListError`, `RequiresToolsBareStringRejected`. Tests use `file://` git fixtures — no real network in CI. No new CI jobs (23 required checks unchanged). | 2026-04-30 |
| 6 | Tool-args schema validation ✅ | Tier 2 priority 6 — realizes ADR-0006 §3 deferral closure. New `tau-runtime::tool_args` module with `ToolArgsValidator` (Draft 7 via `jsonschema` crate). Schemas pre-compile at `RuntimeBuilder::build()`; malformed → `BuildError::ToolSchemaInvalid` (terminates build before any LLM round-trip). Runtime arg-validation failures surface as `ToolError::BadArgs` with MANDATORY template (original args + full schema + specific issue) so the LLM self-corrects via the conversation. Loop survives validation errors; only real plugin invocation crashes still terminate. New ADR-0010. No new CI jobs (23 required checks unchanged). | 2026-04-30 |
| 7 | tau update / verify / uninstall ✅ | Tier 2 priority 7 — closes ADR-0007 §1 deferral. New tau_pkg::tree_hash module (walkdir + sha2; excludes .git/, target/, *.tau-tmp/; symlinks contribute target bytes). New tau_pkg::verify module returning structured VerifyReport (Ok / TreeDrift / BinaryDrift / Missing / Unverified). New tau_pkg::update_package library function composing existing source_list + resolver + install + uninstall. Three CLI subcommands: tau update (default latest tag, --version pin, --prune), tau verify (exit 0/2, --json), tau uninstall (permissive + remediation hint). Lockfile schema v2 → v3 additive (LockedPlugin.binary_sha256 field; v2-leftover entries flagged unverified, not drift). Existing tau_pkg::uninstall library function reused unchanged. New ADR-0012. No new CI jobs (23 required checks unchanged). | 2026-05-01 |
| 11 | REPL persistence ✅ | Tier 3 priority 11 — closes ADR-0006 §16 + ADR-0007 §11 deferrals. New tau-cli/src/session module: SessionId (UUID v7), SessionWriter / SessionReader (JSONL), list_sessions, render_session. Auto-save default with --ephemeral opt-out. tau chat --resume <id-or-prefix> with strict drift validation (agent_id + package.name + package.version + llm_backend match), --force overrides. New tau session subcommand group (list, show, delete, export with jsonl/md/json formats). /clear removed (incoherent with persistence); /info added. Schema v1 baseline. No tau-runtime changes. New ADR-0013. No new CI jobs (23 required checks unchanged). | 2026-05-02 |
| 8 | Streaming LLM responses ✅ | Tier 2 priority 8 — realizes ADR-0006 §5 deferral closure. New `tau-runtime::stream` module with `RunEvent` enum + `run_streaming_inner` async generator (via `async-stream` crate). Kernel pump translates `CompletionChunk` into higher-level `RunEvent`s (`TextDelta`, `ToolCallStarted`, `ToolCallCompleted`, `TurnCompleted`, `RunCompleted`, `FatalError`). `Runtime::run_streaming` + `run_streaming_with_history` public entry points return `Result<impl Stream + 'static, RuntimeError>`. `Runtime::run`/`run_with_history` REFACTOR as thin stream-drainers (zero behavior change; 100+ existing tests pass unchanged). New `RunEvent::FatalError` variant (with `tool_error_variant` tag) preserves byte-identical batch-API error reconstruction for typed `RuntimeError::*` variants (plan-erratum revision documented in ADR-0011 decision 2). `tau chat` streams by default (`--no-stream` opt-out, two-pass termimad render); `tau run --stream` opt-in flag (human + JSON modes; canonical 5-event JSON shape per spec §4.6). New ADR-0011. No new CI jobs (23 required checks unchanged). | 2026-04-30 |
| 12 | Sandboxing ✅ | Tier 3 priority 12 — closes ADR-0006 §13 + Constitution G12. New `tau_ports::Sandbox` port + typed `CapabilityShape` vocabulary in `tau-domain`. Two adapter crates: `tau-sandbox-native` (Linux landlock + seccomp + namespaces; Light + Strict tiers) and `tau-sandbox-container` (docker/podman shell-out, cross-platform). Adapter chain selection in `tau-runtime::sandbox` via probe-based first-Available-meeting-tier wins; `<scope>/config.toml [sandbox]` section configures the chain (schema v1 → v2, additive). Layer 3 pre-flight validation. Plugin host integration via `tokio::Command::as_std_mut()` bridge. Lockfile schema v3 → v4 (additive `required_shapes` field). New `tau resolve --check-sandbox` CLI advisory mode (human + JSON output). macOS / Windows / remote backends, per-command exec gating, per-host network filter, and default activation tracked as follow-ups in [the followups doc](docs/superpowers/specs/2026-05-03-sandboxing-followups.md). New ADR-0014. 25 required CI checks gating `main` (was 23). | 2026-05-03 |
| 12-A | Sandbox activation by default ✅ | Sub-project A from the priority-12 followups — see [spec](docs/superpowers/specs/2026-05-04-sandbox-activation-design.md) and [ADR-0015](docs/decisions/0015-sandbox-activation.md). Sandboxing is now ON by default for every plugin spawn. Scope config schema migrates v2 (chain + minimum_tier) → v3 (declarative `required_tier` + `required_shapes`); v2 lockfiles auto-load with a `tracing::warn!`. Architectural pivot: chain-based selection replaced with Bazel-style `AdapterRegistration` registry + 5-stage `resolve_adapter` filter pipeline (platform → probe → tier → shape → plugin tier). New `passthrough` adapter (~30 LOC) replaces the `Option<None>` "no isolation" branch as a registered first-class adapter. Plugin manifest `[sandbox]` block (`PluginSandboxRequirements`) added to `tau-domain`. `PluginHostOptions` gains `sandbox_adapter` / `force_passthrough` / `force_adapter_kind` fields; CLI integration via `plugin_loader::load_plugins`. Global `--no-sandbox` and `--sandbox <kind>` flags. New subcommands: `tau sandbox status` (diagnostic), `tau sandbox setup` (interactive + `--non-interactive` modes). `tau resolve --check-sandbox` extended to surface plugin-tier mismatches. Hard refuse on resolution failure (exit 2) with guided multi-option diagnostic via `crates/tau-cli/src/cmd/error_render.rs` + insta snapshots. `TAU_TESTING_ALLOW_MOCK_SANDBOX=1` env-var Mock injection preserved. No new CI jobs (25 required checks unchanged from priority 12). | 2026-05-04 |
| 12-B | Plugin compatibility verification ✅ | Sub-project B from the priority-12 followups — see [spec](docs/superpowers/specs/2026-05-04-plugin-compat-design.md) and [ADR-0016](docs/decisions/0016-plugin-compat-verification.md). All 5 real plugins (anthropic, ollama, openai, fs-read, shell) declare `[sandbox] required_tier = "strict"`. New `tau-pkg::sandbox_check` public module: `cross_check_plugin_capabilities` spawns the plugin binary, performs `meta.handshake`, enumerates `tool.describe_capabilities` per method (tool plugins), and bidirectionally diffs against the manifest. `InstallError::CrossCheck { message: String }` (String not `#[from]` because Capability isn't Eq) propagates failure; `InstallOptions::skip_cross_check: bool` escape hatch for stub-binary tests. Wired into `install_with_options` step 8.7 between source SHA-256 and lockfile write. Native adapter symlink-resolution fix in `tau-sandbox-native::light::resolve_symlinks_for_landlock` (absorbs sub-project D's foundation). New workspace crate `tau-plugin-compat` (publish=false): per-plugin tau.toml fixtures, controlled-env-binary (statically linked, isolated from workspace), Layer 3 `tau resolve --check-sandbox` tests (5/5 pass). Layer 4 container + Layer 4 native tests scaffolded but `#[ignore]`'d (10 total) pending sub-project D's port-aware driver + cassette-replay-through-sandboxed-process infrastructure. New `render_cross_check_error` in tau-cli + 3 insta snapshots. CLAUDE.md tracked in git documenting per-agent `CARGO_TARGET_DIR` convention. 27 required CI checks gating `main` (was 25). | 2026-05-04 |
| 12-D | End-to-end landlock CI integration + port-aware Layer 4 driver ✅ | Sub-project D from the priority-12 followups — see [spec](docs/superpowers/specs/2026-05-05-e2e-landlock-design.md) and [ADR-0017](docs/decisions/0017-e2e-landlock-and-driver.md). Re-introduces 5 e2e kernel-enforcement test files removed at priority-12 ship: `light_landlock.rs`, `strict_seccomp.rs`, `strict_net_filter.rs`, `strict_exec_gating.rs` (`#[ignore]`'d pending sub-project E), and `tau-runtime/tests/sandbox_native.rs` for runtime-level adapter integration. New `integration-tests` Cargo feature on `tau-sandbox-native` and `tau-runtime`. New port-aware test driver at `tau-plugin-compat/src/driver.rs` (~150 LOC, 5 unit tests) wrapping public `tau_runtime::plugin_host::load_{tool,llm_backend,storage}`. Two priority-12 native-adapter bugs caught and fixed: (1) `AccessFs::Execute` granted alongside `Read*` in landlock ruleset (without it, exec returns EACCES); (2) auto-add the spawned binary's parent dir to read_paths so plugins built outside system paths can load themselves. Controlled-env binary gains `TAU_FIXTURE_MODE` dispatch (read / open-socket / exec / default). New `docs/reference/sandbox-platform-support.md` documenting kernel features + tested distros. New CI infrastructure: `.github/actions/setup-rust` composite action wrapping `dtolnay/rust-toolchain` + `Swatinem/rust-cache@v2`; workflow-level `CARGO_INCREMENTAL=0` for sccache compatibility; `CLAUDE.md` Rule 4 added. 2 new Linux CI jobs (`test (tau-sandbox-native e2e / linux)` + `test (tau-runtime e2e / linux)`); 29 required CI checks gating `main` (was 27). The 10 Layer 4 plugin-compat `#[ignore]`'s from sub-project B persist — plugins exec under strict-tier landlock but EOF before handshake because their startup-IO surface (config dirs, /tmp, /proc) needs cataloging per plugin; deferred to a D-followup or sub-project F. |
| 12-E | CI optimization ✅ | Sub-project E from the priority-12 followups — see [spec](docs/superpowers/specs/2026-05-06-ci-optimization-design.md), [plan](docs/superpowers/plans/2026-05-06-ci-optimization.md), and [ADR-0018](docs/decisions/0018-ci-optimization.md). Five-phase migration that reshaped CI from 23 required checks / ~33-min PR critical path to **14 required checks / ≤25-min path** without sacrificing coverage. Phase A: tooling — `Swatinem/rust-cache` `shared-key` for cross-job cache sharing; `cargo nextest run` for non-doctest invocations (3× faster on workspace with many test binaries); `mozilla-actions/sccache-action@v0.0.10` on test/check jobs (skipping release builds per spec); `mold` linker on Linux jobs via `rui314/setup-mold@v1`. Phase B: matrix `test:` split into `test-stable / {linux, macos, windows}` (full nextest + doctest) and `msrv-check / {linux, macos, windows}` (cargo check only); `-- --ignored` integration test block removed (dedicated e2e jobs cover that scope); **Windows promoted from `continue-on-error: true` advisory to hard gate** (W2 strictness upgrade) after a 4-of-4 Windows audit. Phase C: new `build-fixtures-linux` job builds 9 binaries once (5 plugins + 2 toy plugins + tau-cli + controlled-env), uploads as `linux-fixture-binaries`; 4 e2e/conformance jobs refactored to download the artifact; eliminates ~9 min of redundant compilation per PR. Phase D: dropped 6 redundant plugin release-build jobs absorbed by `build-fixtures`. Phase E: consolidated 7 `--no-default-features` jobs (5 explicit + 2 misnamed) into one `feature-flag-matrix / linux` shell loop with `::group::<crate>` markers; renamed `test (tau-ports test-fixtures only)` → `test-fixtures-ports / linux`. Real fixes shipped along the way: sccache-action v0.0.6 → v0.0.10 (legacy GHA cache v1 API sunset Feb 2025); `cargo nextest --no-tests=pass` for empty `--run-ignored` filter sets; `.config/nextest.toml` `retries = 2` for parallelism-exposed flakes; `_artifacts/` staging in build-fixtures (upload-artifact preserves directory tree); `chmod +x` after artifact download (upload-artifact strips x bits); restore `cargo build -p tau-cli --bin tau` (debug) in test-tau-plugin-compat (Layer 3 tests hardcode target/debug/tau path). New ADR-0018; CLAUDE.md Rule 6 documents `cargo nextest` for local dev. 14 required CI checks gating `main` (was 23). | 2026-05-06 |
| 12-F | Per-host network filtering ✅ | Sub-project F + sub-project H. F (PR #35, commit d4438ae) shipped the initial veth+nft+CAP_NET_ADMIN design; F task 6.5 (PR #37, commit b14408c) wired apply_post_spawn integration. Sub-project H (PR <TBD>) replaced both with a userspace HTTP-CONNECT proxy + bridge per [ADR-0020](docs/decisions/0020-sandbox-proxy.md). Net result: zero kernel privileges, all 7 previously `#[ignore]`'d sandbox tests now runnable, CI no longer needs privileged Docker. 14 required CI checks (was 15). | 2026-05-06 | 2026-05-07 |
| 12-G | Per-plugin Docker images ✅ | Sub-project I + inline closure of sub-project J from the priority-12 followups — see [spec](docs/superpowers/specs/2026-05-08-per-plugin-images-design.md) and [ADR-0021](docs/decisions/0021-per-plugin-images.md). PR #41 (commit ae8c21c) replaced the Container adapter's bind-mount-the-plugin-binary approach with per-plugin Docker images built on a shared `tau-plugin-base`. New `xtask build-plugin-images` workspace member; CI builds with GHA buildx cache. PR #43 (commit 1d03075) closed the 3 HTTP cassette tests by adding plain-HTTP forwarding to `tau-sandbox-proxy` (alongside the existing CONNECT path) and switching the container to `--user 0` for HTTP plans (Docker Desktop forces root:root 660 perms on bind-mounted Unix sockets — `--cap-drop=ALL` + `--read-only` + seccomp-default keep the security envelope equivalent to nobody). All 5 originally-`#[ignore]`'d Container-adapter integration tests in `layer4_container.rs` close. ADR-0021 documents the four-phase roadmap (Phase 1 ships now; Phases 2–4 deferred). | 2026-05-08 |
| 12-H | Dev environment + pre-push test gate ✅ | Sub-project G (dev environment) + lefthook deep-gate. PR #42 (commit 9080bbb) shipped the lefthook pre-push gate running tests in a Linux Podman container with persistent named volumes (cargo-cache + target-cache). PR #44 (commit f341366) expanded the gate to cover all 10 Linux CI jobs sequentially in a single Podman container with `--security-opt label=disable` for SELinux compatibility, DooD via socket bind-mount, and BUILDX cache env vars. Cost: ~3–4 min warm, ~30–45 min cold. Eliminates ~5–10 min of CI feedback latency for cross-platform breakage that would otherwise only surface on `windows-latest` / `macos-latest` runners. No new CI jobs (gate runs locally before push). | 2026-05-08 |
| 12-I | macOS sandbox-exec adapter ✅ | Sub-project J from the priority-12 followups — see [spec](docs/superpowers/specs/2026-05-09-sandbox-darwin-design.md) and [ADR-0022](docs/decisions/0022-sandbox-darwin.md). PR #45 (commit 597db89). New `tau-sandbox-darwin` crate parallel to `tau-sandbox-native` + `tau-sandbox-container`. Strict tier only via Apple's `sandbox-exec` + SBPL profile (S-expression dialect). `wrap_spawn` builds an SBPL profile from the `SandboxPlan`, writes it to `/tmp/tau-darwin-<pid>-<n>.sb`, then replaces the original command with `sandbox-exec -f <profile> <orig-cmd>`. Network plans validated through `tau-sandbox-proxy::validate_hosts`; profile permits outbound only to `127.0.0.1:8443` (the proxy port). `cfg(target_os = "macos")`-gated runtime; pure-logic `profile.rs` + `baseline.rs` modules compile on any platform (7 unit tests run everywhere; 4 macOS-only integration tests). Runtime registry: `instantiate(RegistryKind::Native)` returns `DarwinSandbox` on macOS. Closes the macOS gap from ADR-0014 §"out of scope" — macOS plugin spawns now run under a real OS-level sandbox with the same defense-in-depth as Linux strict. | 2026-05-09 |
| 12-J | Windows AppContainer adapter — Phase 1 scaffold ✅ | Sub-project K from the priority-12 followups — see [spec](docs/superpowers/specs/2026-05-09-sandbox-windows-design.md) and [ADR-0023](docs/decisions/0023-sandbox-windows-scaffold.md). PR #46 (commit 6a58c53). New `tau-sandbox-windows` crate; **scaffold only** — pure-logic profile generation (`build_appcontainer_caps`) with 7 unit tests, Win32-shape API stubs (`acl.rs::{create_appcontainer_profile,delete_appcontainer_profile,grant_access,revoke_access}` returning `Ok(())` without calling Win32), and `WindowsSandbox` impl `Sandbox` whose probe returns `Unavailable` honestly. Runtime registry wired (`SandboxAdapter::Windows` cfg-gated to Windows). Phase 2 work deferred and tracked in ADR-0023: real Win32 calls via the `windows` crate, UDS→TCP variant of `tau-sandbox-proxy`, and `CreateProcessAsUserW` spawn integration (the three coupled changes that need a Windows dev environment to iterate on — UTM Windows 11 ARM VM is the future unlock). Resolver still falls back to `Passthrough` on Windows because the probe declines; behaviour is unchanged from today. | 2026-05-09 |

## Phase 0 (complete) — bootstrap + foundational sub-projects

**Goal:** empty repo with green CI, full governance files, and the
hexagonal workspace skeleton in place; then five foundational
sub-projects (tau-domain, tau-ports, tau-pkg, tau-runtime, tau-cli)
producing working, testable software on its own per the
brainstorm→spec→plan→implementation cycle.

**Outcome:** all sub-projects shipped on schedule (2026-04-24 →
2026-04-28). 6 ADRs Accepted. 464 workspace tests passing. 12 required
CI status checks gating `main`. Hexagonal architecture realized across
the 5-crate runtime surface (`tau-domain`, `tau-ports`, `tau-pkg`,
`tau-runtime`, `tau-cli`); 3 stub crates (`tau-app`, `tau-infra`,
`tau-observe`) reserved for Phase 1+ work.

**Material v0.1 limitation:** plugin loading is deferred to Phase 1+
per ADR-0007 §18. `tau install` records source trees; the loader lands
in Phase 1.

| # | Sub-project | Produces | Merged |
|---|---|---|---|
| 0 | Repo bootstrap | Empty workspace + governance + CI | 2026-04-24 |
| 1 | `tau-domain` Message + Agent + Package types ✅ | Pure-types crate with `thiserror` errors, doc tests, proptest for parsers | 2026-04-25 |
| 2 | `tau-ports` plugin traits ✅ | Trait definitions for LLM backend, tool, storage, sandbox | 2026-04-26 |
| 3 | `tau-pkg` package manager ✅ | `tau install` from git URLs, capability declarations parsed (G14), scope resolution (G8) | 2026-04-27 |
| 4 | `tau-runtime` agent lifecycle + message passing ✅ | Spawn an agent, deliver messages, observe via structured logs (solo path only) | 2026-04-28 |
| 5 | `tau-cli` real subcommands ✅ | `tau install`, `tau run`, `tau ls`, `tau init`, `tau chat` | 2026-04-28 |

Phase 0 retrospective: [`docs/retrospectives/phase-0.md`](docs/retrospectives/phase-0.md).

## Phase 1 priorities

Detailed motivation per priority is in
[`docs/retrospectives/phase-0.md` §7](docs/retrospectives/phase-0.md).
Tier ordering reflects criticality, not strict implementation order
(some Tier 2/3 items can run in parallel with later Tier 1 items).

### Tier 1 — unblocks Phase 1 itself

1. **Plugin loading mechanism.** ✅ Shipped 2026-04-28 — see
   [ADR-0008](docs/decisions/0008-plugin-loading.md). Out-of-process
   IPC over MessagePack-RPC + tau-pkg/tau-runtime/tau-domain
   amendments. 15 required CI checks gating `main` (was 12 in Phase
   0).
2. **First real LLM-backend plugin.** ✅ Tier 1 priority 2 fully
   complete: Anthropic shipped 2026-04-29 as priority 2a; Ollama
   shipped 2026-04-29 as priority 2b; OpenAI shipped 2026-04-29 as
   priority 2c — closing out Tier 1 priority 2 with the rule-of-three
   refactor (`tau-plugin-test-support`) and the deferred conformance
   suite (`tau-plugin-conformance`). All three plugins migrated to
   typed `LlmError` variants. ADR-0009 (typed-error migration policy
   + conformance suite charter) Accepted. 21 required CI checks
   gating `main` (was 17).
3. **First real Tool plugins.** ✅ `fs-read` + `shell` shipped
   2026-04-30 as priority 3 — exercises capability checks at runtime
   end-to-end. Closed two IPC infrastructure gaps in the same sub-
   project: kernel-side capability enforcement for IPC tools (Gap 1
   via new `tool.describe_capabilities` wire method) and agent-grant
   flow to plugin processes (Gap 2 via additive
   `SessionContext.granted_capabilities`). 23 required CI checks
   gating `main` (was 21).

### Tier 2 — completes Phase 0 deferrals

4. **Capability override implementation** ✅ Shipped 2026-04-30 — see
   [spec](docs/superpowers/specs/2026-04-30-capability-override-design.md).
   Realizes ADR-0007 §4 reservation. Project tau.toml
   `[[agents.<id>.capabilities]]` narrows package grants via
   semantic glob-subset on `allow_*` plus `deny_*` carve-outs (deny
   wins). Validation at parse time + every runtime load (fail-closed
   both places). Audit surface: `tau list agents --capabilities`.
5. **Transitive dependency resolution** ✅ Shipped 2026-04-30 — see
   [spec](docs/superpowers/specs/2026-04-30-transitive-deps-design.md).
   Realizes ADR-0007 §5 reservation. Project tau.toml
   `[[agents.<id>.requires.tools]]` declares typed dependencies
   (`name + source + optional version constraint`); `tau run`/`tau chat`
   auto-install missing entries via lazy resolve; new `tau resolve`
   subcommand serves project-wide install. Cargo-style semver
   intersection across declarations of the same tool. One level deep:
   recursive package-level `dependencies` (ADR-0004 §10) stays
   deferred. No new CI jobs (23 required checks unchanged).
6. **Schema validation for tool args** ✅ Shipped 2026-04-30 — see
   [spec](docs/superpowers/specs/2026-04-30-tool-args-schema-design.md)
   and [ADR-0010](docs/decisions/0010-tool-args-schema-validation.md).
   New `tau-runtime::tool_args` module validates every tool-call's
   args against the tool's declared `ToolSpec.input_schema` (Draft 7
   via `jsonschema` crate). Schemas pre-compile at
   `RuntimeBuilder::build()`; malformed → `BuildError::ToolSchemaInvalid`
   before any LLM round-trip. Runtime arg-validation failures surface
   as `ToolError::BadArgs` with MANDATORY template (original args +
   full schema + specific issue) so the LLM self-corrects via the
   conversation. `RuntimeError::PluginContractViolation` stays
   reserved for a future out-of-process plugin handshake-lying
   trigger path. No new CI jobs (23 required checks unchanged).
7. **`tau update` / `tau verify` / `tau uninstall` subcommands** ✅ Shipped 2026-05-01 — see
   [spec](docs/superpowers/specs/2026-05-01-tau-lifecycle-design.md)
   and [ADR-0012](docs/decisions/0012-tau-lifecycle-commands.md).
   New `tau_pkg::tree_hash`, `verify`, `update_package` modules.
   Whole-tree SHA-256 verify is source-agnostic (anticipates future
   `PackageSource` variants). `tau update <pkg>` defaults to latest
   tag; `--version` to pin; `--prune` opt-in. `tau uninstall` is
   permissive with a remediation hint pointing to project tau.toml's
   `[[requires.tools]]` entries. Lockfile schema v2 → v3 (additive:
   `LockedPlugin.binary_sha256`). Existing `tau_pkg::uninstall`
   library function reused unchanged. No new CI jobs (23 required
   checks unchanged).
8. **Streaming LLM responses** ✅ Shipped 2026-04-30 — see
   [spec](docs/superpowers/specs/2026-04-30-streaming-design.md)
   and [ADR-0011](docs/decisions/0011-streaming-llm-responses.md).
   New `Runtime::run_streaming` and `run_streaming_with_history`
   yield a `Stream<Item = RunEvent> + 'static` as the agent loop
   progresses. Existing `run`/`run_with_history` REFACTOR as thin
   stream-drainers (zero behavior change for batch callers; one
   source of truth for the agent loop). New `RunEvent::FatalError`
   variant preserves byte-identical batch-API error semantics
   (LLM, Tool::*, ToolNotRegistered errors round-trip via
   `tool_error_variant` tagging — see ADR-0011 decision 2). `tau
   chat` streams by default with two-pass termimad rendering
   (`--no-stream` opt-out); `tau run --stream` opt-in flag (human
   + JSON modes; canonical 5-event JSON shape per spec §4.6). No
   new CI jobs (23 required checks unchanged).

### Tier 3 — extends the runtime

9. **Multi-agent orchestration** (G10's deferred half). 🚧 In progress —
    primitive-set spec at
    [`2026-05-12-multi-agent-orchestration-design.md`](docs/superpowers/specs/2026-05-12-multi-agent-orchestration-design.md).
    Defines the 6 entities (Identity, Capability, Agent, Task, TraceEvent,
    Run), 3 verb classes (think/call/complete, virtual tools, host-emitted),
    and 3 channels (sync return, shared state, trace) that compose into
    linear / hierarchical / supervisor / worker-pool / plan-revise patterns.
    Coordination via shared TaskList with hierarchical task ids + locks
    (owner + lease + heartbeat). No bus, no inbox, no push-into-LLM. CLI
    output is npm/cargo-style line-feed.
10. **Workflow / pipeline runner** (deterministic step-by-step
    pipelines) ✅ Shipped 2026-05-12 — see
    [spec](docs/superpowers/specs/2026-05-12-tau-workflow-design.md) and
    [ADR-0022](docs/decisions/0022-tau-workflow.md). New `tau-workflow`
    crate; `tau workflow {list, run, log, resume}`; JSONL persistence;
    `--resume` with drift checking. v1 is linear pipelines; DAG and
    parallel branches earmarked as a "workflow-DAG" sub-project (see
    deferred follow-ups below).
11. **REPL persistence** (`tau chat --resume <id>`) ✅ Shipped 2026-05-02 — see
    [spec](docs/superpowers/specs/2026-05-01-repl-persistence-design.md)
    and [ADR-0013](docs/decisions/0013-repl-persistence.md).
    Sessions auto-save to JSONL files at `<scope>/.tau/sessions/<uuid>.jsonl`.
    `--ephemeral` opts out. `tau chat <agent> --resume <id-or-prefix>`
    with strict-mode drift validation (`--force` overrides). New
    `tau session` subcommand group (list, show, delete, export). New
    `/info` REPL slash command; `/clear` removed (replaced by `/exit`
    + re-run). No tau-runtime changes (NG6 preserved). No new CI jobs
    (23 required checks unchanged).
12. **Sandboxing implementation** ✅ Shipped 2026-05-03 — see
    [spec](docs/superpowers/specs/2026-05-02-sandboxing-design.md),
    [vision](docs/explanation/tau-as-language.md), and
    [ADR-0014](docs/decisions/0014-sandboxing.md). New
    `tau_ports::Sandbox` port (probe / supported_shapes /
    validate_plan / wrap_spawn) + typed `CapabilityShape` vocabulary
    in `tau-domain`. Two adapter crates: `tau-sandbox-native` (Linux
    landlock + seccomp + namespaces; Light + Strict tiers) and
    `tau-sandbox-container` (docker/podman shell-out, cross-platform).
    Adapter chain selection in `tau-runtime::sandbox` via probe-based
    first-Available-meeting-tier wins; `<scope>/config.toml [sandbox]`
    section configures the chain (schema v1 → v2, additive). Layer 3
    pre-flight validation (`build_plan` + `validate_plan_against_adapter`,
    returns ALL errors per pass). Plugin host integration via
    `tokio::Command::as_std_mut()` bridge. Lockfile schema v3 → v4
    (additive `required_shapes` field; v3 entries auto-upgrade with
    once-per-process migration warning). New `tau resolve --check-sandbox`
    CLI advisory mode (human + JSON output). macOS / Windows / remote
    backends, per-command exec gating (landlock V2), per-host network
    filter (nftables-in-netns), and default activation are tracked as
    follow-ups in [the followups doc](docs/superpowers/specs/2026-05-03-sandboxing-followups.md).
    25 required CI checks gating `main` (was 23).
    - **Sub-project A: Sandbox activation by default** ✅ Shipped 2026-05-04 — see
      [spec](docs/superpowers/specs/2026-05-04-sandbox-activation-design.md)
      and [ADR-0015](docs/decisions/0015-sandbox-activation.md).
      Sandboxing is now ON by default for every plugin spawn.
      Architectural pivot: chain-based selection replaced with
      Bazel-style declarative requirements + adapter registry +
      5-stage resolver filter pipeline. Schema v2 → v3 migration
      with auto-loading + `tracing::warn!`. New `passthrough`
      adapter, plugin manifest `[sandbox]` block, global
      `--no-sandbox` / `--sandbox <kind>` flags, `tau sandbox
      status` / `tau sandbox setup` subcommands, hard-refuse on
      resolution failure with guided multi-option diagnostic. The
      9 remaining priority-12 follow-ups (B–K minus A) stay
      tracked in the followups doc. No new CI jobs (25 required
      checks unchanged).
16. **Skills as first-class packages** (Constitution G10). Currently
    *partial*: `kinds::SKILL = "skill"` is a recognized `PackageKind`
    in tau-domain (per Constitution G10's commitment that "Skills and
    MCP are first-class concepts in core"), and the v1.2 multi-agent
    spawn arg `system_prompt: Option<String>` (PR #61, commit cb894cc's
    follow-up) provides the runtime foundation. What's missing: the
    end-to-end installation, discovery, and invocation pipeline. A
    Skill package contains a `(system_prompt, capability declaration,
    optional tools list)` triple — fundamentally what a spawn arg
    encodes, but shipped as an installable artifact. Concretely:
    - Manifest extension: `[skill]` table in tau.toml documenting the
      skill's purpose, capability requirements, and default system
      prompt (parallel to `[plugin]` and `[sandbox]` blocks).
    - `tau install <skill-pkg>` resolves + installs to scope (reuses
      tau-pkg).
    - `tau skill list` enumerates installed skills (parallel to
      `tau list agents`).
    - Agent-side invocation: a spawned `agent.<skill-name>.spawn`
      resolves to the installed skill's manifest, pulling system_prompt
      + grant defaults from the package rather than requiring the
      caller to supply them inline. Caller can override per spawn.
    - Agent Skills spec compliance — interop with the broader 2026
      ecosystem (the Anthropic Agent Skills spec, etc.).
    - Reference skill packages shipped as test fixtures + docs.

    This closes Constitution G10's commitment ("Skills and MCP are
    first-class concepts in core. Tau understands the Agent Skills
    spec and the Model Context Protocol natively"). The v1.2 spawn
    arg work is the necessary precondition; this is the layer that
    makes skills installable and discoverable. ~3-4 weeks of work.

### Tier 4 — operational quality

13. **Performance budgets enforced in CI** (Constitution QG14, G16).
14. **`cargo audit` + `cargo-deny` in CI** (Constitution QG16) ✅ Shipped
    2026-05-11 (PR #57, commit f8ad58f). `cargo-deny / linux` is the
    19th required CI check; gates RustSec advisories + license
    allow-list + non-crates.io sources.
15. **Serve mode** (JSON-RPC over stdio) ✅ Shipped 2026-05-17 — see
    [spec](docs/superpowers/specs/2026-05-17-tau-serve-mode-design.md)
    and [ADR-0033](docs/decisions/0033-tau-serve-mode.md).
    `tau serve` exposes runtime.run + runtime.run_streaming as JSON-RPC
    2.0 over NDJSON-framed stdio. 5 methods + 1 server-initiated
    notification in v1. One `Runtime` per process, parallel concurrent
    runs (cap 8 default). Graceful shutdown on SIGTERM/SIGINT/stdin-EOF/
    parent-death. `tau-app` crate exits stub status. Sister refactor in
    [ADR-0032](docs/decisions/0032-capability-override-relocation.md)
    moved `CapabilityOverride` to tau-pkg to break a dependency cycle.
    Phase 1 closes; Phase 2 (tau as a compiled language) is now the
    active phase.

### Deferred sub-projects (cross-tier)

Tracked here as future extensions of v1 primitives. Each is a clean
addition (not a re-architecture) when a concrete use case lands. See
the "Considered and rejected" table in
[`2026-05-12-multi-agent-orchestration-design.md`](docs/superpowers/specs/2026-05-12-multi-agent-orchestration-design.md)
for design context.

- **Background tools / monitors** — claude-code-style `Monitor` pattern:
  a tool that runs in the background, emits events over time, delivered
  to the agent's context at turn boundaries. Different primitive class
  than v1's synchronous tools: needs a fourth channel (push-at-turn-
  boundary) + `BackgroundTool` entity + `Tool::Background` capability.
  Useful for watching CI runs, log files, file changes, webhooks. Not
  in v1; tracked here for future implementation when a use case justifies
  the design surface (sandbox lifecycle of long-running tools is the
  load-bearing problem).
- **Inter-agent message bus / inbox stacks** — rejected for v1 in favor
  of shared TaskList. Would re-open if many-to-many coordination or
  unsolicited interrupts become necessary.
- **Pull-status tool** — `agent.<kind>_status()` virtual tool letting
  parent's LLM check on a still-running child. v1 uses host-side
  watchdog timeouts instead.
- **Output schemas** — typed tool returns (Pydantic / JSON schema
  constraints on results). Refinement of `Tool.result_schema`.
- **Plan DAG with task dependencies** — `Task.depends_on: [TaskId]` +
  topological scheduling. v1 uses linear hierarchy via
  `parent_task_id`.
- **Cross-run memory** — persistent state above `Run`. Currently each
  run is independent; session persistence (Tier 3 §11) is per-session
  not per-content.
- **Workflow-DAG** — extension of tau-workflow v1 from linear pipelines
  to a DAG with parallel branches + fan-out / fan-in. Tracked in
  [ADR-0022](docs/decisions/0022-tau-workflow.md).
- **Group chat / mediator agent** — many-to-many via a mediator. v1
  uses TaskList as the coordination primitive instead.

## Phase 2 — Tau as a compiled language for agentic workflows

The sandboxing sub-project (Tier 3 priority 12, [ADR-0014](docs/decisions/0014-sandboxing.md))
lays the foundation for tau as a compiled language. See
[`docs/explanation/tau-as-language.md`](docs/explanation/tau-as-language.md)
for the full vision: write a "tau program" (project tau.toml + plugin
manifests + lockfile) once, compile it for a specific target triple
(`linux-native-strict`, `container-podman`, `wasi-p2`, etc.), and the
toolchain guarantees the resulting bundle runs anywhere a matching
adapter exists. The same kind of "if it compiles, it runs" guarantee
Rust gives developers and Docker gives operators, applied to the
agent-workflow domain.

Phase 2 sub-projects build on the priority-12 foundation:

- **A. `tau check` standalone command** ✅ Shipped 2026-05-18 — see
  [spec](docs/superpowers/specs/2026-05-18-tau-check-design.md). Aggregator
  CLI verb wrapping every existing pre-flight validator (config, lockfile,
  packages, sandbox, plugins, skills) with human / `--json` / `--sarif`
  output and granular exit codes (0/2/3/64/70). Bare `tau check` runs all
  6 categories; `tau check <category>` runs one. Pure orchestration in
  tau-cli — no new tau-pkg/tau-runtime code. Suitable for CI gates,
  pre-commit hooks, and IDE Problems-panel integration via SARIF.
- **B. Tau target triple registry** ✅ Shipped 2026-05-19 — see
  [ADR-0034](docs/decisions/0034-target-triple-registry.md), the
  [reference page](docs/reference/target-triples.md), and the
  [design spec](docs/superpowers/specs/2026-05-19-target-triple-registry-design.md).
  Bazel-inspired 3-axis structural identifier (`Platform` × `AdapterFamily`
  × `SandboxTier`) in `tau-ports::target`. 5 Available triples + 1 Reserved
  (windows-native-strict); `remote-*` and `wasi-*` namespaces reserved.
  CLI surface: `tau target list`/`show` and `tau check --target <triple>`.
- **C. `tau build --target <triple>` + bundle format** (~6 weeks). The
  deployment artifact: a content-hashed bundle pinning resolved
  package versions + tree hashes (priority 7's `tree_hash`) + computed
  capability-effective set per agent (priority 4's `compute_effective`)
  + required capability shapes per plugin (priority 12) + target sandbox
  triple. `tau run --bundle <file>` executes a bundle.
- **D. Capability vocabulary forward-compatibility** (~2 weeks). A
  bundle compiled against tau v1.2 must continue to run on tau v1.3+
  with new shapes added. Stability discipline + ADR amendments.
- **E. Cross-machine reproducibility verification** (~3 weeks). Extends
  `tau verify` (priority 7) so a deployed bundle on a target machine
  can be verified to match the bundle the project author built.
- **F. Remote target backends** (~4-6 weeks per backend). Vercel
  Sandbox, Sandcastle, generic remote-execution providers. Each is an
  additional `Sandbox` impl. Major design concerns: authentication,
  IPC channel networking, cold-start latency budgets.
- **G. WASM target backend** (~12+ weeks). Most ambitious. Plugins
  compile to `wasm32-wasip2`. The `tau-plugin-sdk` migrates to
  support the new ABI. Plugin distribution becomes `.wasm` artifacts.
  Plausibly a Phase 2 effort in its own right.

These Phase 2 sub-projects are independent of the **immediate
follow-ups** that close gaps left by v0.1 of priority 12 (default
activation, plugin compatibility, e2e CI infrastructure, per-command
exec gating, per-host network filter, fork-server pattern, macOS /
Windows adapters). Those follow-ups are tracked in
[`docs/superpowers/specs/2026-05-03-sandboxing-followups.md`](docs/superpowers/specs/2026-05-03-sandboxing-followups.md).

## Out of scope (forever)

These are tau's explicit non-goals from
[`CONSTITUTION.md` §2](CONSTITUTION.md). They will not be added to
core regardless of demand:

- **NG1.** Tau is not an LLM or an agent.
- **NG2.** Tau is not a coding-specific tool.
- **NG3.** Tau is not a hosted service.
- **NG4.** Tau is not a package marketplace.
- **NG5.** Tau is not a general-purpose workflow engine.
- **NG6.** Tau does not provide persistent agent memory in core.
- **NG7.** Tau does not evaluate agent quality.
- **NG8.** Tau is not an AI safety harness.
- **NG9.** Tau does not manage identity, authentication, or
  credentials.
- **NG10.** Tau does not collect telemetry or training data.
- **NG11.** Tau is a developer tool, not an end-user tool.
- **NG12.** Tau is a runtime, not a framework.

Adjacent ideas may belong in plugins or downstream projects (such as
`stature`, the opinionated coding pipeline planned as a separate
project).
