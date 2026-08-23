# Test Ignore Inventory

**Refreshed:** 2026-05-17 (supersedes 2026-05-13 inventory from PR #71)
**Workspace state:** based on `origin/main` at `bcbac9a`
**Total `#[ignore = "..."]` annotations:** 22

22 `#[ignore]` annotations across the workspace, organised into four
triage buckets. This file is the canonical reference for what each ignored
test needs in order to run, and which CI job (existing or future) is
responsible for lighting it up.

**Rule:** every PR that adds, removes, or promotes a `#[ignore]` annotation
must update the corresponding row here in the same commit.

**Bucket legend:**

- **LIVE-DOCUMENTED** — needs a real upstream service / credential.
  Legitimately `#[ignore]`'d by default; opt-in via env var. Should never
  block CI.
- **DARK** — could run in a dedicated CI job that satisfies the prereqs,
  but no such job exists today. These are the real coverage gaps. See plan
  [`2026-05-17-test-suite-upgrades.md`](superpowers/plans/2026-05-17-test-suite-upgrades.md) Task 11.
- **ENVIRONMENT-SPECIFIC** — needs a host shape that no GH runner provides
  (e.g. "no Docker AND no Linux native sandbox available"). Will only ever
  run in a sub-project's bespoke e2e CI.
- **DEFERRED** — waiting on an explicit dependency (helper, fixture,
  sibling PR). Should reference what is being awaited.

---

## What changed since the 2026-05-13 inventory

Resolved and removed from the inventory:

- 5 × `cmd_orchestration.rs` patterns A–E — Skills-4 T9 (PR #83) shipped
  `MockLlmBackend`; all five `#[ignore]` annotations removed.
- 2 × `install_cross_check.rs` (`install_with_matching_manifest_succeeds_…`,
  `install_force_after_cross_check_fix_succeeds`) — PR #117 enabled both.
- 1 × `run_kernel_errors.rs:127` (`plugin_contract_violation`) — PR #118
  implemented the test body and removed the `#[ignore]`.

Added since 2026-05-13 (Layer4 work merged):

- 10 × `tau-plugin-compat/tests/layer4_{native,container}.rs` — were on a
  local branch when the previous inventory was generated; now on `main`.

Net change: −8 + 10 = +2 entries (20 → 22).

---

## Bucket 1 — LIVE-DOCUMENTED (6 tests)

Live API smoke tests. Opt-in via `TAU_<provider>_LIVE_TESTS=1` + API key.

| File:line | Test | Reason |
|-----------|------|--------|
| `crates/tau-plugins/anthropic/tests/live.rs:45` | `live_complete_smoke` | `TAU_ANTHROPIC_LIVE_TESTS=1` + `ANTHROPIC_API_KEY` |
| `crates/tau-plugins/anthropic/tests/live.rs:59` | `live_stream_smoke` | `TAU_ANTHROPIC_LIVE_TESTS=1` + `ANTHROPIC_API_KEY` |
| `crates/tau-plugins/ollama/tests/live.rs:49` | `live_complete_smoke` | `TAU_OLLAMA_LIVE_TESTS=1` + running Ollama instance |
| `crates/tau-plugins/ollama/tests/live.rs:63` | `live_stream_smoke` | `TAU_OLLAMA_LIVE_TESTS=1` + running Ollama instance |
| `crates/tau-plugins/openai/tests/live.rs:45` | `live_complete_smoke` | `TAU_OPENAI_LIVE_TESTS=1` + `OPENAI_API_KEY` |
| `crates/tau-plugins/openai/tests/live.rs:59` | `live_stream_smoke` | `TAU_OPENAI_LIVE_TESTS=1` + `OPENAI_API_KEY` |

**CI plan:** keep `#[ignore]`'d. Each `live.rs` already documents the
opt-in invocation in its module header. No regular CI job runs these by
design; surface as a nightly secret-bearing job if/when the project wants
live regression signal.

---

## Bucket 2 — DARK (12 tests)

Sandbox-related tests that need Linux + a daemon (Docker/Podman) and/or
prebuilt plugin binaries. **Largest hidden coverage gap in the workspace
today.** Plan Task 11 introduces a dedicated `layer4-ignored` CI matrix.

### 2a — `tau-plugin-compat` Layer4 native (Linux landlock/seccomp + prebuilt plugin)

| File:line | Test | Plugin binary required |
|-----------|------|------------------------|
| `crates/tau-plugin-compat/tests/layer4_native.rs:235` | `shell_layer4_native_runs_echo_hello` | `cargo build -p tau-plugins-shell --release` |
| `crates/tau-plugin-compat/tests/layer4_native.rs:326` | `fs_read_layer4_native_reads_data_file` | `cargo build -p tau-plugins-fs-read --release` |
| `crates/tau-plugin-compat/tests/layer4_native.rs:517` | `anthropic_layer4_native_completes_via_cassette` | anthropic-plugin + `tau-net-bridge` |
| `crates/tau-plugin-compat/tests/layer4_native.rs:613` | `ollama_layer4_native_completes_via_cassette` | ollama-plugin + `tau-net-bridge` |
| `crates/tau-plugin-compat/tests/layer4_native.rs:703` | `openai_layer4_native_completes_via_cassette` | openai-plugin + `tau-net-bridge` |

### 2b — `tau-plugin-compat` Layer4 container (Docker/Podman + plugin image)

| File:line | Test | Image / binary required |
|-----------|------|-------------------------|
| `crates/tau-plugin-compat/tests/layer4_container.rs:278` | `shell_layer4_container_runs_echo_hello` | `tau-plugin-shell-plugin:dev` |
| `crates/tau-plugin-compat/tests/layer4_container.rs:363` | `fs_read_layer4_container_reads_data_file` | `tau-plugin-fs-read-plugin:dev` |
| `crates/tau-plugin-compat/tests/layer4_container.rs:478` | `anthropic_layer4_container_completes_via_cassette` | `tau-plugin-anthropic-plugin:dev` |
| `crates/tau-plugin-compat/tests/layer4_container.rs:570` | `ollama_layer4_container_completes_via_cassette` | `tau-plugin-ollama-plugin:dev` |
| `crates/tau-plugin-compat/tests/layer4_container.rs:653` | `openai_layer4_container_completes_via_cassette` | `tau-plugin-openai-plugin:dev` |

### 2c — `tau-runtime-tokio` container smoke (Linux + Docker/Podman on PATH)

The file is `#![cfg(all(target_os = "linux", feature = "integration-tests"))]`,
so it already only compiles under the integration-tests feature.

| File:line | Test | Reason |
|-----------|------|--------|
| `crates/tau-runtime-tokio/tests/sandbox_container.rs:23` | `fs_read_works_inside_container` | requires Linux + docker or podman on PATH |
| `crates/tau-runtime-tokio/tests/sandbox_container.rs:51` | `shell_plugin_runs_under_container` | requires Linux + docker or podman on PATH |

**CI plan:** LIT since 2026-08-23. `test-tau-runtime-e2e` grew a second
step (`Run --ignored container-gated tests`) that opts in via
`cargo nextest run --run-ignored only -p tau-runtime-tokio
--features integration-tests`, mirroring the sibling step on
`test-tau-sandbox-native-e2e`. GHA `ubuntu-latest` ships both docker
and podman, so the in-test probe resolves `Available` and the tests do
real work rather than taking their skip branch.

Enabling the lane surfaced a stale assertion in
`shell_plugin_runs_under_container`: it asserted the wrapped argv still
contained the original program path (`/bin/sh`). That contract changed
when the adapter moved to per-plugin images — for a non-HTTP plan the
image's own `ENTRYPOINT` *is* the plugin binary, so the program survives
only as the image tag (`tau-plugin-sh:dev`, from its basename) and the
caller's args are appended after the image. The assertion now checks the
current contract.

### 2d — `tau-sandbox-native` landlock-gated tests

Two tests previously used silent `eprintln!("SKIP") + return` paths when
their runtime probes (landlock + seccomp + namespaces, or proxy spawn)
failed. T3 of the test-suite-upgrades plan converted them to
`#[ignore = "..."]` with explicit reasons so the silent skip stops
masking environments where the test never runs.

| File:line | Test | Reason |
|-----------|------|--------|
| `crates/tau-sandbox-native/tests/strict_bridge.rs:111` | `bridge_survives_strict_tier_filter` | requires Linux kernel with landlock + seccomp + user namespaces |
| `crates/tau-sandbox-native/src/strict.rs:700` | `wrap_spawn_with_http_cap_sets_both_proxy_env_vars` | requires environment where the strict-tier proxy can spawn |

**CI plan:** the existing `test-tau-sandbox-native-e2e` job grew a
second step (`Run --ignored landlock-gated tests`) that opts in via
`cargo nextest run --run-ignored only -p tau-sandbox-native
--features integration-tests`. GHA `ubuntu-latest` runners satisfy
the prereqs, so both tests are LIT in CI but stay opt-in for local
developer runs (where landlock support is unknown).

**Status (2026-05-18):** Bucket 2b fully LIT. Bucket 2a partially LIT —
the 2 tool-plugin tests (`shell_layer4_native_runs_echo_hello`,
`fs_read_layer4_native_reads_data_file`) run on the native leg of
`test-tau-plugin-compat-layer4-ignored`; the 3 HTTP cassette tests
(`anthropic|ollama|openai _layer4_native_completes_via_cassette`) stay
DARK on native because `tau-net-bridge`'s network-namespace setup
needs `CAP_SYS_ADMIN` + `CAP_NET_ADMIN`, which standard GHA
`ubuntu-latest` runners do not grant. They ARE covered via the
container leg (Bucket 2b counterparts), so the strict-tier behaviour
is exercised; only the native-adapter variant of that behaviour is
ungated by privileges. Promotable when a privileged runner is
available. The `NEXTEST_FILTER` filterset in `tier2.yml` that encodes
this exclusion now carries that reason inline, so it can no longer read
as an unexplained silent skip.

**Status (2026-08-23):** Bucket 2c
(`tau-runtime-tokio/tests/sandbox_container.rs`) is now LIT via the
`--run-ignored only` step on `test-tau-runtime-e2e`. All remaining DARK
entries are the 3 privileged-runner native HTTP tests.

---

## Bucket 3 — ENVIRONMENT-SPECIFIC (2 tests)

Tests requiring a host with **no** strict-capable sandbox available
(no Docker, no Linux native). GitHub runners can't reproduce this
shape; they need a bespoke sub-project D e2e CI.

| File:line | Test | Reason |
|-----------|------|--------|
| `crates/tau-cli/tests/cmd_resolve_check_sandbox.rs:373` | `no_adapter_emits_clear_error` | no Docker AND no Linux native; sub-project D e2e |
| `crates/tau-cli/tests/cmd_resolve_check_sandbox.rs:538` | `check_sandbox_errors_when_only_passthrough_available` | no non-passthrough adapter; sub-project D e2e |

**CI plan:** wait for sub-project D e2e CI to land. Until then, document
as expected-dark and verify manually before any sandbox-error-rendering
refactor.

---

## Bucket 4 — DEFERRED (2 tests)

Waiting on a specific helper / fixture / sibling work.

| File:line | Test | Waiting on |
|-----------|------|-----------|
| `crates/tau-pkg/tests/install_cross_check.rs:213` | `cross_check_fires_and_fails_for_non_protocol_binary` | Full release build + 10s handshake timeout makes this too slow for routine CI. Promote when (a) a slow-tier CI lane exists OR (b) the cross-check timeout becomes configurable. |
| `crates/tau-cli/tests/cmd_workflow.rs:42` | `workflow_run_writes_jsonl_and_succeeds` | Needs `echo-llm` plugin fixture + project scaffold helpers. Lift from `cmd_chat.rs` / `cmd_run.rs` once those helpers stabilise. |

**CI plan:** revisit each line when its blocker resolves. If a blocker is
gone but the test is still `#[ignore]`'d, promote in a dedicated PR.

---

## Summary

| Bucket | Count | CI plan |
|--------|------:|---------|
| LIVE-DOCUMENTED | 6 | Stay `#[ignore]`'d; document opt-in |
| LIT — bucket 2d (Task T3) | 2 | `test-tau-sandbox-native-e2e` `--run-ignored only` step |
| LIT — bucket 2c (2026-08-23) | 2 | `test-tau-runtime-e2e` `--run-ignored only` step |
| DARK | 3 | 3 × native HTTP (need privileged runner) |
| LIT (Task 11) | 7 | 5 × container + 2 × native tool plugins via `test-tau-plugin-compat-layer4-ignored / {native,container}` matrix |
| ENVIRONMENT-SPECIFIC | 2 | Sub-project D e2e (separate) |
| DEFERRED | 2 | Promote when blocker resolves |
| **Total** | **22** | |

Numbers updated on each PR that touches an `#[ignore]` annotation.

---

## Appendix — feature-gated dark lanes (not `#[ignore]`)

A test can be dark without an `#[ignore]` annotation: a whole file gated
behind `#![cfg(all(target_os = "...", feature = "integration-tests"))]`
never runs unless some CI job passes `--features integration-tests` on
that OS. `--workspace --all-targets` does **not** enable it. These lanes
are invisible to the buckets above, so they are tracked here.

| Crate | Test file | Enabled by |
|-------|-----------|-----------|
| `tau-sandbox-windows` | AppContainer launcher + strict enforcement | `nextest-windows` → "Test tau-sandbox-windows AppContainer (integration)" |
| `tau-sandbox-darwin` | `tests/strict_integration.rs` | `nextest-macos` → "Test tau-sandbox-darwin sandbox-exec (integration)" (added 2026-08-23) |
| `tau-sandbox-native` | landlock/seccomp e2e | `test-tau-sandbox-native-e2e` (both steps) |
| `tau-plugin-compat` | layer4 native/container | `test-tau-plugin-compat` + the layer4-ignored matrix |
| `tau-runtime-tokio` | `tests/sandbox_container.rs` | `test-tau-runtime-e2e` (both steps) |

Before 2026-08-23 `tau-sandbox-darwin` declared the
`integration-tests` feature but **no** CI job enabled it, so macOS
sandbox-exec enforcement was structurally untested. Adding a crate-scoped
`integration-tests` feature without a matching CI step reintroduces that
hole — add the step in the same PR.
