# Test Ignore Inventory

**Refreshed:** 2026-08-27 (supersedes the 2026-05-17 refresh, itself
superseding the 2026-05-13 inventory from PR #71)
**Workspace state:** based on `origin/main` at `e38f2530`
**Total `#[ignore]` annotations:** 42

42 `#[ignore]` annotations across the workspace, organised into four
triage buckets. This file is the canonical reference for what each ignored
test needs in order to run, and which CI job (existing or future) is
responsible for lighting it up.

**Rule:** every PR that adds, removes, or promotes a `#[ignore]` annotation
must update the corresponding row here in the same commit.

**The rule is enforced.** `xtask/tests/ignore_inventory.rs` counts `#[ignore]`
attributes under `crates/` and fails when the count differs from the header
total above. It runs in Tier 0 via `just test` (the `test-stable / linux` job),
costs a file walk, and is deliberately a *count* check rather than a parse: it
cannot tell you which row is wrong, which is what forces a human to open this
page and place the new annotation in a bucket. It is **not** a lint against
`#[ignore]` — several below are legitimately permanent, and satisfying the gate
by deleting an annotation is the wrong fix. For fifteen months the rule was
unenforced and drifted exactly as you would expect: the header said 22 while
the workspace held 42, and six crates had no bucket at all.

It works. The gate's first encounter with real drift came hours after it was
written: #697 landed `decode_does_not_retain_across_many_calls` while the PR
adding the gate was still in review, and rebasing onto that `main` turned the
Tier 0 job red until the row below was filled in. Under the old regime the test
would simply have been the 43rd uninventoried annotation.

What the gate counts: a line whose trimmed form starts with `#[ignore]`,
`#[ignore =` or `#[ignore(`. Prose mentions in `//` and `//!` comments are
excluded by construction (they start with a slash). A `#[cfg_attr(…, ignore)]`
would be invisible to it — none exist today; if one lands, it under-counts
silently, so prefer the plain attribute.

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

**LIT** is not a bucket but a status: an entry is LIT when a named CI job runs
it with `--run-ignored`. LIT entries stay in their bucket and name the job.

---

## What changed since the 2026-05-17 inventory

The 2026-05-17 refresh covered 22 annotations. Three sweeps have landed since
and the header was never re-derived, so this is a full re-count from
`grep -rn '#\[ignore' crates/`, not a patch.

Resolved and removed from the inventory:

- 2 × `cmd_resolve_check_sandbox.rs` — **deleted** by #656 (2026-08-23), not
  promoted. See Bucket 3.
- 1 × `cmd_workflow.rs` `workflow_run_writes_jsonl_and_succeeds` — **promoted**
  by #656, renamed `workflow_run_emits_step_record_and_succeeds`. See Bucket 4.

Newly inventoried (present on `main` for months; never had a bucket):

- 10 × `tau-wasm-host` guest-build tests → new Bucket 2f.
- 8 × `tau-cli` guest-build tests → new Bucket 2e. Seven were wired into CI by
  #656; the eighth (`build_wasm_linear_pipeline_runs_in_guest_and_returns_last_leaf`)
  arrived with #652 the same day and is covered by the same lane.
- 1 × `tau-conformance`, 1 × `tau-ir-lower`, 1 × `tau-mcp-tokio` → Bucket 4.

Landed while this refresh was in review:

- 1 × `tau-plugin-protocol` `decode_does_not_retain_across_many_calls` (#697,
  2026-08-27) → Bucket 4. Caught by the new gate on rebase, which is the
  intended behaviour rather than an inconvenience.

Net: 22 → 43 (−3 resolved, +21 newly inventoried, +1 landed mid-review,
+2 already-counted rows re-derived).

**Since that refresh:** 44 → 42. #717 promoted the two Bucket 4 rows whose
stated blockers had gone stale (`multi_event_sse_response`,
`lowering_refuses_on_capability_fit_mismatch`) — both are now live tests, not
deleted annotations. See "Stale-reason follow-ups" under Bucket 4.

A note on counting, because it bit the sweep that produced this refresh: a bare
`grep -c '#\[ignore' crates/` **overcounts by roughly 40%** — at the 2026-08-27
sweep it said 71 against 42 real attributes. The excess is prose: module docs
and inline comments that mention the annotation while explaining it, including
this page's own subject matter. Only lines whose *trimmed* form opens the
attribute count; comments start with a slash, so trimming excludes them. That
rule, not any raw grep, is the definition of record — it is what the gate
implements. To reproduce the gate's number by hand:

```sh
grep -rn '#\[ignore' crates/ --include='*.rs' | grep -vE ':[0-9]+: *(//|\*)' | wc -l
```

Keep the `-n` and the filename (no `-h`): the filter keys on the `file:line:`
prefix to find the start of the matched text, so dropping either makes it match
nothing and silently report the raw overcount.

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

**CI plan:** keep `#[ignore]`'d — permanently. Each `live.rs` already documents
the opt-in invocation in its module header. No regular CI job runs these by
design; surface as a nightly secret-bearing job if/when the project wants
live regression signal.

---

## Bucket 2 — DARK / heavy-lane (32 tests)

Tests that need a prerequisite the Tier 0 gate cannot cheaply provide: Linux +
a daemon (Docker/Podman), prebuilt plugin binaries, or a nested
`wasm32-wasip2` guest build. 29 of the 32 are LIT in a named tier2 job; the
remaining 3 need a privileged runner.

### 2a — `tau-plugin-compat` Layer4 native (Linux landlock/seccomp + prebuilt plugin)

| File:line | Test | Plugin binary required | Status |
|-----------|------|------------------------|--------|
| `crates/tau-plugin-compat/tests/layer4_native.rs:236` | `shell_layer4_native_runs_echo_hello` | `cargo build -p tau-plugins-shell --release` | LIT |
| `crates/tau-plugin-compat/tests/layer4_native.rs:327` | `fs_read_layer4_native_reads_data_file` | `cargo build -p tau-plugins-fs-read --release` | LIT |
| `crates/tau-plugin-compat/tests/layer4_native.rs:518` | `anthropic_layer4_native_completes_via_cassette` | anthropic-plugin + `tau-net-bridge` | **DARK** |
| `crates/tau-plugin-compat/tests/layer4_native.rs:614` | `ollama_layer4_native_completes_via_cassette` | ollama-plugin + `tau-net-bridge` | **DARK** |
| `crates/tau-plugin-compat/tests/layer4_native.rs:704` | `openai_layer4_native_completes_via_cassette` | openai-plugin + `tau-net-bridge` | **DARK** |

**CI plan:** LIT rows run on the native leg of
`test-tau-plugin-compat-layer4-ignored`. The 3 HTTP cassette tests stay DARK on
native because `tau-net-bridge`'s network-namespace setup needs `CAP_SYS_ADMIN`
+ `CAP_NET_ADMIN`, which standard GHA `ubuntu-latest` runners do not grant.
They ARE covered via the container leg (2b counterparts), so the strict-tier
behaviour is exercised; only the native-adapter variant of that behaviour is
ungated by privileges. Promotable when a privileged runner is available. The
`NEXTEST_FILTER` filterset in `tier2.yml` that encodes this exclusion carries
that reason inline, so it cannot read as an unexplained silent skip.

### 2b — `tau-plugin-compat` Layer4 container (Docker/Podman + plugin image)

| File:line | Test | Image / binary required |
|-----------|------|-------------------------|
| `crates/tau-plugin-compat/tests/layer4_container.rs:279` | `shell_layer4_container_runs_echo_hello` | `tau-plugin-shell-plugin:dev` |
| `crates/tau-plugin-compat/tests/layer4_container.rs:364` | `fs_read_layer4_container_reads_data_file` | `tau-plugin-fs-read-plugin:dev` |
| `crates/tau-plugin-compat/tests/layer4_container.rs:479` | `anthropic_layer4_container_completes_via_cassette` | `tau-plugin-anthropic-plugin:dev` |
| `crates/tau-plugin-compat/tests/layer4_container.rs:571` | `ollama_layer4_container_completes_via_cassette` | `tau-plugin-ollama-plugin:dev` |
| `crates/tau-plugin-compat/tests/layer4_container.rs:654` | `openai_layer4_container_completes_via_cassette` | `tau-plugin-openai-plugin:dev` |

**CI plan:** LIT since 2026-05-18 — container leg of
`test-tau-plugin-compat-layer4-ignored` (plan Task 11).

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
current contract. This is the canonical example of why a long-dark test is
not neutral: it rots.

### 2d — `tau-sandbox-native` landlock-gated tests

Two tests previously used silent `eprintln!("SKIP") + return` paths when
their runtime probes (landlock + seccomp + namespaces, or proxy spawn)
failed. T3 of the test-suite-upgrades plan converted them to
`#[ignore = "..."]` with explicit reasons so the silent skip stops
masking environments where the test never runs.

| File:line | Test | Reason |
|-----------|------|--------|
| `crates/tau-sandbox-native/tests/strict_bridge.rs:114` | `bridge_survives_strict_tier_filter` | requires Linux kernel with landlock + seccomp + user namespaces |
| `crates/tau-sandbox-native/src/strict.rs:714` | `wrap_spawn_with_http_cap_sets_both_proxy_env_vars` | requires environment where the strict-tier proxy can spawn |

**CI plan:** LIT — `test-tau-sandbox-native-e2e` grew a second step
(`Run --ignored landlock-gated tests`) that opts in via
`cargo nextest run --run-ignored only -p tau-sandbox-native
--features integration-tests`. GHA `ubuntu-latest` runners satisfy
the prereqs, so both tests are LIT in CI but stay opt-in for local
developer runs (where landlock support is unknown).

### 2e — `tau-cli` wasm guest-build tests (8, LIT 2026-08-23)

Every `#[ignore]`d test in `tau-cli` shells
`cargo build -p tau-wasm-guest --target wasm32-wasip2` from the CLI's own
lowering (`lower_to_wasm_ir` / `wasm_world_for_project`) and asserts on the
produced component. They were written and maintained but run by **no CI job at
all** until the 2026-08-23 QA sweep.

| File:line | Test | Asserts |
|-----------|------|---------|
| `crates/tau-cli/tests/build_wasm_e2e.rs:24` | `build_wasm_then_run_returns_typed_stream` | β.7.5 PR-E2 DoD — typed `RunEvent` stream out of a built component |
| `crates/tau-cli/tests/build_wasm_e2e.rs:55` | `build_wasm_linear_pipeline_runs_in_guest_and_returns_last_leaf` | #621 PR-2 (#652) — guest executes a linear pipeline |
| `crates/tau-cli/tests/build_wasm_world_dod.rs:115` | `dod_guest_compiles_against_cap_exact_world` | EPIC 3.2 — capability-exact WIT world text |
| `crates/tau-cli/tests/wasi_http_roundtrip.rs:106` | `ungranted_host_is_denied_at_runtime_through_real_guest` | EPIC 3.6 — `wasi:http` egress denial at the `WasiCtx` |
| `crates/tau-cli/tests/wasi_fs_roundtrip.rs:134` | `ungranted_path_is_denied_at_runtime_through_real_guest` | EPIC 3.6-b — `FsAccessDenied` for an ungranted path |
| `crates/tau-cli/tests/wasi_fs_roundtrip.rs:171` | `nested_preopens_bind_longest_prefix_and_write_truncates` | #604 preopen hardening |
| `crates/tau-cli/tests/wasi_fs_roundtrip.rs:227` | `root_preopen_serves_subpaths` | #604 — `/`-root preopen |
| `crates/tau-cli/tests/embed_wasm_e2e.rs:77` | `example_product_loads_and_runs_component` | EPIC 7.2 (#414) — product runtime loads + runs |

**CI plan:** LIT since 2026-08-23. The `wasm-lane` job in `tier2.yml` grew
a second step,
`cargo nextest run -p tau-cli --run-ignored ignored-only --test-threads 1`,
alongside the existing `tau-wasm-host` step. It subsumed the narrower
`-E 'binary(embed_wasm_e2e)'` step added the same day by #648.

`--test-threads 1` is load-bearing: these tests derive their guest
`CARGO_TARGET_DIR` from the fixture name, so the two `fs-rw` tests share
one dir and `wasi_http_roundtrip` shares `target/tau-build-wasm` with
`build_wasm_world_dod`. In parallel, two cargo processes drive the same
dir with different `TAU_WORLD_WIT`/`TAU_IR_BYTES` and the guest fails to
compile. **If a new ignored tau-cli test is added, give it its own guest
target dir** — the serialization keeps the lane honest but costs
wall-clock. First green CI run: 7/7 in 430s, job total 15m22s. The step
selects by crate rather than by name, so #652's eighth test joined the lane
without a CI edit.

### 2f — `tau-wasm-host` guest-build tests (10, LIT)

The embedder half of the same lane: each shells `cargo build --target
wasm32-wasip2` for the guest or a probe fixture, then loads it in wasmtime and
asserts runtime behaviour. Never inventoried before this refresh, though the
lane that runs them has existed since #548/#595.

| File:line | Test | Asserts |
|-----------|------|---------|
| `crates/tau-wasm-host/tests/roundtrip.rs:136` | `guest_with_no_baked_ir_errors` | β.7.5 PR-D — missing baked IR is an error, not a hang |
| `crates/tau-wasm-host/tests/roundtrip.rs:148` | `guest_decodes_baked_ir_and_starts_run` | β.7.5 PR-D — guest decodes its baked IR |
| `crates/tau-wasm-host/tests/roundtrip.rs:170` | `guest_drives_ir_and_returns_typed_stream` | β.7.5 PR-E2 — guest drives `run_ir_streaming` |
| `crates/tau-wasm-host/tests/roundtrip.rs:201` | `host_guest_roundtrip_is_deterministic` | determinism across two host↔guest runs |
| `crates/tau-wasm-host/tests/emit_event_buffer.rs:142` | `events_stream_via_emit_event_not_the_run_payload` | EPIC 5.4 — events arrive via `emit-event`, not the return payload |
| `crates/tau-wasm-host/tests/embed_ports.rs:162` | `with_ports_streams_the_same_events_the_buffered_api_returns` | EPIC 7.2 — `EmbedPorts` streaming ≡ buffered API |
| `crates/tau-wasm-host/tests/embed_ports.rs:201` | `with_ports_surfaces_complete_error_as_a_fatal_error_event` | EPIC 7.2 — backend error becomes a fatal event |
| `crates/tau-wasm-host/tests/fan_monitor_simple.rs:118` | `simplified_fan_monitor_runs_in_guest` | β.7.5 PR-F — shared native tools in-guest |
| `crates/tau-wasm-host/tests/wasi_fs_enforcement.rs:67` | `granted_path_is_readable_ungranted_path_is_not` | EPIC 3.6-b — host-side `wasi:filesystem` preopen scope |
| `crates/tau-wasm-host/tests/wasi_http_enforcement.rs:93` | `egress_is_denied_for_unauthorized_host_and_method` | EPIC 3.3/3.6 — host-side `wasi:http` egress policy |

**CI plan:** LIT — `wasm-lane` step
`cargo nextest run -p tau-wasm-host --run-ignored all`. `all`, not
`ignored-only`: the crate's non-ignored tests (e.g. `roundtrip.rs`'s
lowering-`Err` assertion, deliberately un-`#[ignore]`d because it needs no
guest build) are cheap and belong in the same run. Selecting by crate means a
new ignored `tau-wasm-host` test joins the lane with no CI edit — but it still
needs a row here, and the Tier 0 gate will say so.

**Status (2026-08-27):** of the 32 entries in Bucket 2, 29 are LIT and 3 are
DARK — the privileged-runner native HTTP cassette tests in 2a. That is the only
remaining DARK set in the workspace.

---

## Bucket 3 — ENVIRONMENT-SPECIFIC (0 tests — both DELETED 2026-08-23)

Tests requiring a host with **no** strict-capable sandbox available
(no Docker, no Linux native). GitHub runners can't reproduce this
shape; they needed a bespoke sub-project D e2e CI.

| File:line | Test | Disposition |
|-----------|------|-------------|
| ~~`crates/tau-cli/tests/cmd_resolve_check_sandbox.rs:373`~~ | ~~`no_adapter_emits_clear_error`~~ | **deleted** 2026-08-23 (#656) |
| ~~`crates/tau-cli/tests/cmd_resolve_check_sandbox.rs:538`~~ | ~~`check_sandbox_errors_when_only_passthrough_available`~~ | **deleted** 2026-08-23 (#656) |

**Resolution (2026-08-23):** sub-project D e2e CI was never built, and no
GitHub-hosted runner has the required host shape — `ubuntu-latest` probes
the Linux native (Landlock) adapter `Available`, `macos-latest` the darwin
`sandbox-exec` adapter, `windows-latest` has Docker. Verified by running
both on macOS: each fails (`1 plugins checked: 1 ok, 0 errors`) and would
fail identically anywhere it were enabled. A test that cannot run anywhere
is not coverage, so both were deleted with the rationale left inline in
`cmd_resolve_check_sandbox.rs` (see "Test 4: REMOVED" and "Test 8: REMOVED").
`ResolutionError::NoAdapterMatches`'s Display stays unit-tested in
`tau-runtime-tokio::process_gate::resolution_error`; the CLI's
error-to-exit-2 rendering on that path is uncovered (as it was before —
these tests never having run).

**Interaction with the tau-cli ignored lane:** the `wasm-lane` step added by
#656 (`-p tau-cli --run-ignored ignored-only`, `tier2.yml:611`) selects the
whole crate, so had these two survived they *would* now be swept into it — and
they would fail there, on a runner that has a strict-capable adapter. Deleting
them was the right call for that lane too, not just on principle.

**Rule for this bucket:** it should stay empty. An `#[ignore]` whose
prerequisite is a host shape no runner provides is a deletion candidate,
not an inventory row.

---

## Bucket 4 — DEFERRED (4 tests)

Waiting on a specific helper / fixture / sibling work.

| File:line | Test | Waiting on |
|-----------|------|-----------|
| `crates/tau-pkg/tests/install_cross_check.rs:222` | `cross_check_fires_and_fails_for_non_protocol_binary` | Full release build + 10s handshake timeout makes this too slow for routine CI. Promote when (a) a slow-tier CI lane exists OR (b) the cross-check timeout becomes configurable. |
| `crates/tau-plugin-protocol/tests/decode_allocation_bound.rs:215` | `decode_does_not_retain_across_many_calls` | 2,000,000 `Frame::decode` calls — the #676 retention property. Too slow for routine CI; no job runs `-p tau-plugin-protocol --run-ignored`. Same blocker as the row above: promote when a slow-tier lane exists. Added by #697 (2026-08-27). |
| `crates/tau-conformance/tests/conformance.rs:67` | `fan_monitor_dev_matches_wasm` | `WasmProfile::run` is still `unimplemented!()` (`crates/tau-conformance/src/profile/wasm.rs`). The β.6 `conformance / linux` job runs `-p tau-conformance` without `--run-ignored`, so it is skipped there by design. **Stated reason is stale — see follow-ups.** |
| `crates/tau-sandbox-windows/tests/install_rust_cargo_acceptance.rs:255` | `rust_cargo_install_succeeds_sandboxed_without_unsandboxed_escape` | #726: `CreateProcess` on `rustc.exe` denies even though its DACL carries a correct inherited allow-ACE (`FILE_EXECUTE` included) for the AppContainer's package SID. Windows-only, `integration-tests`-gated; egress chain itself is proven green by `tests/egress_integration.rs`. Un-ignore when #726 lands a fix. |

**CI plan:** revisit each line when its blocker resolves. If a blocker is
gone but the test is still `#[ignore]`'d, promote in a dedicated PR.

**Promotion note (2026-08-23):** the `cmd_workflow.rs` row was a `todo!()`
stub, not a test — its blocker (`common::setup_echo_project`) had
stabilised long before. Implementing it immediately surfaced
[#650](https://github.com/tau-rs/tau/issues/650): `tau workflow run` never
persists its JSONL run log, because `WorkflowRunLogLayer::on_event` writes
from a detached `tokio::spawn` dropped at runtime shutdown. The promoted
test (`workflow_run_emits_step_record_and_succeeds`) asserts the emitted
`tau::workflow::step` event; its on-disk assertions sit commented out
pointing at #650. A `#[ignore]`d stub is the cheapest place for a bug this
size to hide.

### Stale-reason follow-ups — resolved by #717 (2026-08-29)

Three Bucket 4 reasons were flagged on 2026-08-27 as naming a prerequisite that
had since landed — the same failure class #648 found in 2c, a test whose stated
blocker no longer describes reality. Two of them were #717's scope; both are now
**promoted out of the bucket**, which is why the total dropped 44 → 42.

- **`multi_event_sse_response`** (`tau-mcp-tokio`) — the comment read "Fix in
  PR-5 when McpBridge gets a richer session-aware fixture". PR-5 is #287, merged
  2026-06-09, without the fixture, so the row pointed at closed work and read as
  owned when it was unowned. **#717 wrote the fixture**: a `wiremock::Respond`
  impl that dispatches on the POSTed request's JSON-RPC `id`, serving the
  two-event `initialize` body for `id=0` and a distinct `tools/list` body for
  `id=1`. The stale-`id=0` timeout is structurally impossible now, and the test
  is live — it is the only coverage of `recv_response_for`'s skip-loop over a
  leading notification.

- **`lowering_refuses_on_capability_fit_mismatch`** (`tau-ir-lower`) — recorded
  in 2026-08-27 as the weaker, third signal: the reason ("no `Available` entry
  lacks `NetworkHttp`") was *true*, but the body would have passed for the wrong
  reason, because `lookup_target_excluding_network()` returned a synthetic triple
  that missed the registry entirely and so took `capability_fit::check`'s
  unknown-target arm (`missing: []`), never the shape-miss arm it is named for.

  **#717 found the reason's conclusion wrong, not just its body.** The premise
  generalised "no entry lacks `NetworkHttp`" into "no entry lacks any shape a
  workflow can require" — but `any-wasi-strict` is an `Available` entry built
  from `fs_rw_net` (`{FilesystemRead, FilesystemWrite, NetworkHttp}`), so it
  lacks `ProcessExec` **and** `AgentSpawn`. The shape-miss path was drivable all
  along, just not with the shape the test happened to pick. The test now declares
  an `fs.exec` tool against `any-wasi-strict` and asserts
  `missing == [ProcessExec]` plus the blamed tool — a real miss against a real
  entry, no registry change needed. The unknown-triple case was split out as
  `lowering_refuses_on_unknown_target_triple`, which asserts `missing.is_empty()`
  explicitly; asserting the emptiness is what keeps the two arms distinguishable,
  where the old `matches!(err, CapabilityFitFailed { .. })` conflated them. No
  `no-network` target tier is needed, and that speculative registry work should
  not be scheduled on this test's behalf.

  Lesson for the next sweep: an `#[ignore]` reason can be *factually correct and
  still wrong*, when the fact is narrower than the conclusion drawn from it. Both
  the 2026-08-27 sweep and #717's own issue text repeated the generalisation
  without re-reading the shape constructors. Check the claim, then check that the
  claim implies the conclusion.

The third flagged reason — **`fan_monitor_dev_matches_wasm`** (`tau-conformance`)
— was deliberately **out of #717's scope** and is tracked in #691. Its reason
reads "TODO(β.7.5): WasmProfile needs `tau build wasm`"; β.7.5 shipped and the
`wasm-lane` job exercises `tau build wasm`, so that dependency is met. The
*actual* remaining blocker is that `WasmProfile::run` was never written — still a
stub that panics `unimplemented!()`. The row is correct to stay `#[ignore]`d; the
reason should name the stub, not the shipped dependency. It stays in Bucket 4.

---

## Summary

| Bucket | Count | CI plan |
|--------|------:|---------|
| 1 — LIVE-DOCUMENTED | 6 | Stay `#[ignore]`'d permanently; document opt-in |
| 2a — layer4 native (LIT) | 2 | `test-tau-plugin-compat-layer4-ignored` / native |
| 2a — layer4 native (**DARK**) | 3 | 3 × native HTTP cassette — need a privileged runner |
| 2b — layer4 container (LIT) | 5 | `test-tau-plugin-compat-layer4-ignored` / container |
| 2c — runtime-tokio container (LIT) | 2 | `test-tau-runtime-e2e` `--run-ignored only` step |
| 2d — sandbox-native landlock (LIT) | 2 | `test-tau-sandbox-native-e2e` `--run-ignored only` step |
| 2e — tau-cli guest builds (LIT) | 8 | `wasm-lane` `-p tau-cli --run-ignored ignored-only --test-threads 1` |
| 2f — tau-wasm-host guest builds (LIT) | 10 | `wasm-lane` `-p tau-wasm-host --run-ignored all` |
| 3 — ENVIRONMENT-SPECIFIC | 0 | Both deleted 2026-08-23 — no runner has the host shape |
| 4 — DEFERRED | 4 | Promote when blocker resolves (2 of them want the same slow-tier lane) |
| **Total** | **42** | |

Numbers updated on each PR that touches an `#[ignore]` annotation — enforced by
`xtask/tests/ignore_inventory.rs`.

---

## Appendix — feature-gated dark lanes (not `#[ignore]`)

A test can be dark without an `#[ignore]` annotation: a whole file gated
behind `#![cfg(all(target_os = "...", feature = "integration-tests"))]`
never runs unless some CI job passes `--features integration-tests` on
that OS. `--workspace --all-targets` does **not** enable it. These lanes
are invisible to the buckets above *and to the Tier 0 count gate* — there is
no annotation to count — so this table is the only control for the class.

| Crate | Test file(s) | Enabled by |
|-------|--------------|-----------|
| `tau-sandbox-windows` | `tests/launcher_integration.rs`, `tests/strict_integration.rs` | `nextest-windows` → "Test tau-sandbox-windows AppContainer (integration)" |
| `tau-sandbox-darwin` | `tests/strict_integration.rs` | `nextest-macos` → "Test tau-sandbox-darwin sandbox-exec (integration)" (added 2026-08-23) |
| `tau-sandbox-native` | `tests/{strict_bridge,strict_proxy,strict_seccomp,strict_exec_gating,light_landlock}.rs` | `test-tau-sandbox-native-e2e` (both steps) |
| `tau-plugin-compat` | `tests/{layer3_check_sandbox,layer4_native,layer4_container}.rs` | `test-tau-plugin-compat` + the layer4-ignored matrix |
| `tau-runtime-tokio` | `tests/{sandbox_container,sandbox_native}.rs` | `test-tau-runtime-e2e` (both steps) |

`tau-workflow` was on this table until 2026-08-28 and is now **un-gated**
(#716): `tests/integration.rs` lost its `#![cfg(feature =
"integration-tests")]` and the crate's `integration-tests = []` declaration
was deleted, so Tier 0's `--workspace --all-targets` runs it. Unlike the
sandbox lanes it needed no special host — it drives the real `Runner` against
`MockLlmBackend` from `tau_ports::fixtures`, with no subprocess and no
network, so the gate bought nothing.

Un-gating exposed exactly the rot the gate was hiding, the #648 class: the
test still asserted the pre-Sub-project-D contract in which `RunLog::append`
wrote the JSONL directly. It is now a `tracing::event!` emitter materialized
by `WorkflowRunLogLayer`, so with no subscriber installed the log file was
never created and `replay` failed with `NotFound`. #716 fixed the test — it
pins `run_id`, derives the path with `run_log_path`, and installs the layer
via `set_default` for the run — rather than weakening the persistence
assertions, which are the only end-to-end coverage of the property #650 broke.

`tau-sandbox-container`'s `integration-tests = []` declaration had **zero
consumers** — no `tests/` directory and no `cfg(feature =
"integration-tests")` anywhere in the crate. A dead feature flag reads as
coverage that exists, so #716 deleted the declaration. If that crate ever
grows integration tests, re-add the feature *and* its CI step together, per
the rule below.

Before 2026-08-23 `tau-sandbox-darwin` declared the
`integration-tests` feature but **no** CI job enabled it, so macOS
sandbox-exec enforcement was structurally untested. Adding a crate-scoped
`integration-tests` feature without a matching CI step reintroduces that
hole — add the step in the same PR, or do not add the feature.
