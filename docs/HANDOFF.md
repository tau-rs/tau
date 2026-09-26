# Handoff — Agent Kernel (tau rebase)

Status: framing complete, ready for repo bootstrap.
Scope: Unix only (Linux + macOS). Rust. Single-user harness for the first releases.
Identity decision: **the project keeps the name tau.** The reboot keeps the mission and abandons the implementation — the name is the promise, not the code (Linux survived rewrites of its scheduler and whole subsystems for the same reason). The existing work is renamed and archived as **tau v1** (`tau-rs/tau-legacy`); a fresh `tau-rs/tau` is created from scratch, no history carried over. This document is the seed of the new repository's `docs/` tree; a copy goes into tau v1's sessions directory as its final entry.

---

## 1. Mission and philosophy

Build an **agent kernel**: the minimal, stable substrate on which agent harnesses and pipelines are composed — explicitly not another agent framework. Design method borrowed from Linux:

- A tiny set of primitives that never break ("we do not break userspace"), with internals free to churn.
- One uniform interface over heterogeneous things (everything is a message, as Linux's everything-is-a-file).
- Isolation and resource limits as kernel primitives (namespaces/cgroups → capability namespaces/budgets), so containers-of-agents *emerge* rather than being features.
- Observation as installed programs, not built-in features (eBPF → hooks).
- Ship a working core first; grow by accretion around frozen interfaces. tau's failure mode (constitution and eight crates before a stable core) is the anti-pattern this reboot corrects.

Three-tag summary for architects: **hexagonal architecture with a single message-shaped port, event-sourced core, capability security — enforced at runtime, not by convention.**

---

## 2. The seven syscalls (the frozen boundary)

Everything an agent can do. Nothing else is expressible.

| # | Syscall | Signature (abridged) | Notes |
|---|---------|----------------------|-------|
| 1 | spawn | `spawn(program, ns: Namespace, budget: Budget) -> AgentId` | ns must be ⊆ parent's, snapshotted at birth; budget carved atomically from parent's; unspent returns at exit |
| 2 | exit | `exit(result: Bytes) -> !` | Consumes the kernel handle (type-level guarantee of "last act"); result stored until claimed |
| 3 | wait | `wait(Child(id) \| Any) -> ExitResult` | Level-triggered; results persist until claimed; Any returns in completion order |
| 4 | cancel | `cancel(id, CancelMode{grace, reason})` | Two-phase: atomic subtree freeze → CancelNotice + driver abandon → kernel-enforced grace deadline → hard abort |
| 5 | send | `send(cap: Capability, payload) -> Corr` | Capability = address AND permission (capability security); never blocks (WouldBlock error); append-first |
| 6 | recv | `recv(filter: Match) -> Msg` | Closed filter language: corr / sender / kind / any / or. Resolution (which msg matched) is itself a log entry |
| 7 | attach | `attach(HookPoint, HookProgram, FailureMode) -> HookId` | Harness-privileged only. Verdicts: Allow / Deny(reason) / Emit(Notice). Every verdict logged |

Grouping: 1–4 manage the **tree**, 5–6 the **flow**, 7 the **meta**. Irreducibility test (regression rule for scope creep): any proposed eighth syscall must be shown inexpressible as a program over these seven; if expressible, it is libc.

### 2.1 Shared types — the ABI surface

```rust
pub struct Capability(opaque);      // unforgeable; only from namespace or message transfer
pub struct Namespace { caps: Set<Capability> }   // snapshot at spawn
pub struct Budget { dims: Map<DimKey, u64> }     // see §4: map, not fixed fields
pub struct Corr(u64);               // kernel-allocated at send; only request↔reply link
pub struct AgentId(u64);            // never reused within a run

pub struct Msg {                    // THE frozen envelope
    abi:      u16,                  // version field — day zero (audit finding)
    seq:      u64,                  // log position
    from:     Endpoint,
    corr:     Option<Corr>,
    kind:     MsgKind,              // Request | Reply | Partial | Notice
    consumed: Option<Consumption>,  // driver-reported, on replies
    payload:  BlobRef,              // hash into blob store — NOT inline (§4)
}
```

The envelope + grant types are the "do not break userspace" surface. Everything else may churn.

---

## 3. Architecture layers

```
Agents (async fns; sealed world; only the 7 syscalls)
  └ libc (userspace convenience: infer(), tool_loop(), retries, fan-out — no special powers)
──── syscall boundary (frozen) ────
Kernel = Router/Reducer + Log + State(tree, budgets) + Hooks
──── endpoint queues ────
Drivers (border guards: envelope ↔ world protocol; report consumption)
  model | classifier/ML | tool | sandbox | CI | store | clock | human
World (APIs, processes, people, time)
```

Key invariants (each is load-bearing):
1. **No arrow skips the kernel.** Agents never touch drivers; parents never touch children directly. Every effect is a log entry.
2. **The kernel never parses payloads.** It routes by capability and accounts by driver-reported consumption metadata. This is what makes model-agnosticism structural.
3. **The log is the kernel; everything else is cache.** Any behavior that works without a log entry is a bug (replay divergence).
4. **Anything invisible to the log is a replay bug** (no bypass channels, no userspace JoinHandles as hidden state, recv resolutions logged).

### 3.1 Substrate decision

A + B, with C as planned upgrade and D quarantined:
- **A (in-process, tokio)** is the body: agents are tasks, kernel is a library.
- **B (event-sourced) is the constitution**: append-before-apply; the reducer is a pure, deterministic fold (`fn apply(state, entry) -> state`; no clock, no RNG, no iteration-order leaks). This is enforced by CI (see §8, determinism jobs).
- **C (durable execution)** becomes cheap later because the journal exists from day one: crash → refold log → completed effects replay from journal, no re-billing.
- **D (OS isolation)** lives only inside the sandbox driver (subprocess/cgroups/namespaces; Firecracker-class later). Agents are trusted loops; the code models ask to run is the untrusted thing.

### 3.2 Model bridge (how LLMs act)

Models are devices; they have no hands. The bridge is a libc tool loop:
namespace → projected tool schema (per-driver `describe()`; single source of truth via `#[derive(Deserialize, JsonSchema)]`) → provider-side constrained decoding → serde validation → name→capability resolution → `send`. All failures (bad args, unknown tool, policy veto) are fed back as tool results; the model self-corrects. Exposure levels are harness choices: slot-filling → tool loop → syscalls-as-tools (spawn/wait/cancel in the schema → sub-agent orchestration) → code-as-action (via sandbox driver only).

Security consequence: prompt injection degrades to contained failure — a hijacked model in a `ns![SEARCH]` child can, at worst, search.

### 3.3 Hooks

Registry filled at boot (before root spawn), synchronous metered loop at pinned points (PreSend, PreDeliver, OnSpawn, OnExit, OnBudget(threshold)), three-word effect vocabulary (Allow/Deny/Emit), every verdict logged, first Deny wins, FailureMode::Closed mandatory for veto-capable hooks. HookProgram: native closures (trusted, v1) + Rule DSL (fast declarative); WASM+fuel slot reserved for the day policies cross a trust boundary. Hooks never do I/O or inference; ML-based screening lives in a screening driver or agent, not a hook. Sorting rule for every future feature: per-agent → program; per-endpoint → driver; cross-cutting → hook.

### 3.4 ML models as peers

Cheap models (classifiers, embedders, rerankers) are drivers like any other, running in-process (ONNX/candle), reporting `compute_ms` instead of tokens. Cascade routing (cheap-first, escalate on low confidence) is plain agent code. Namespaces make cheap subtrees structurally unable to spend LLM money. The LLM is not privileged anywhere in the design — it is the most articulate device, not the center.

---

## 4. Resolved design decisions (from the audit)

Decide-before-code (all resolved as follows unless re-litigated by ADR):

1. **Envelope versioning**: `abi: u16` on Msg and log headers from day zero. Additive evolution only.
2. **Authority model**: agent authority = birth namespace ∪ received capabilities; capability transfer is a logged, hookable act; sender must hold what it transfers. "Immutable" means "no change without a log entry."
3. **Harness is not an agent**: it is the pre-agent code holding the kernel handle. attach/detach privilege belongs to it. Written down to resist future elegance.
4. **Time enters as log entries**: clock driver appends ticks and armed deadlines; wall-budget enforcement happens in the reducer on tick application. The reducer never reads a clock.
5. **Payloads out of the log**: envelope stores a content hash; payloads live in a content-addressed blob store (dedupe for free; crypto-shredding for erasure: per-subtree encryption, delete key = payload gone, structural log survives).
6. **Budget = map of dimensions** with reserved keys (`tokens`, `cost_microusd`, `wall_ms`, `calls`, `depth`, `compute_ms`). New device kinds report native units without ABI breaks.
7. **Snapshots**: reducer state fully serializable; replay = snapshot + tail. Designed in, implemented at M3.
8. **Driver supervision**: driver health is kernel state; supervisor restarts drivers; in-flight corrs across driver restart fail explicitly (error envelope, never a hang).
9. **Mailbox hygiene**: corrs are owned; unclaimed corrs dead-letter at owner exit; coarse TTL backstop.
10. **Branching honesty**: replay of a prefix is truthful; branching re-executes the world (new costs, model nondeterminism). Controlled branches require seeded/pinned sampling in the model driver or accepted divergence.
11. **Cancel-safety rule**: agent futures hold only plain memory + the kernel handle (no guards across await). Enforced by review + lint.

---

## 5. Identity, naming, and the rename choreography

**Epochs.** The archived work is referred to as *tau v1* (the prototype) everywhere — ADRs, READMEs, conversation. The new repo's versioning starts at `0.x`; **tau 1.0 means the ABI freeze**, nothing else. One sentence carried in both READMEs: "v1 explored the territory; this tau is the kernel."

**GitHub choreography (one sitting, order matters).**
1. Commit the tombstone into the old repo: this handoff as the final sessions entry (`sessions/<date>-kernel-reboot-handoff.md`), a final ADR ("tau, rebooted" — drift diagnosis, link forward), and a 3-line README banner pointing to the successor.
2. Rename `tau-rs/tau` → `tau-rs/tau-legacy`; flip the archive (read-only) flag.
3. Immediately create the fresh `tau-rs/tau` and push the initial commit (this file as `docs/HANDOFF.md` + ADR-0001).
Gotcha to know: GitHub's rename redirect dies the moment the new repo claims the old name — old deep links (issue permalinks, pinned file URLs) will land on the *new* repo or 404. That is the desired behavior (traffic finds the successor), but it is why step 1 precedes step 2: the archive must carry its own explanation before the links move.

**crates.io.** Bare `tau` is squatted by an abandoned 2015 math-constant crate; crates.io has no reclamation policy. Plan: publish the kernel as **`tau-kernel`** (verified free, along with `tau-core`, `libtau`, `taud`) and ship the binary as `tau` from the `cli/` crate (`tau-cli`) — Cargo decouples the two (`[[bin]] name = "tau"`), so users run `cargo install tau-cli` once and type `tau replay <log>` forever (the `ripgrep`→`rg` pattern). The binary is its own crate because it links the kernel *and* the store (#199), and the store depends on the kernel. Reserve `tau-kernel` + `libtau` with `0.0.0` placeholders on day one. Send one polite email to the dormant crate's owner asking for a transfer; if it ever lands, `tau` becomes a facade crate re-exporting `tau-kernel` — additive, nothing breaks.

## 6. Repository bootstrap

- Org/repo: `tau-rs/tau`, fresh (see §5 choreography). tau v1 archived read-only as `tau-rs/tau-legacy`; `docs/adr/0001-tau-rebooted.md` in the new repo records the drift diagnosis and links the archive.
- Crate layout (deliberately small; split only when compile times demand):
  - `kernel` — types (ABI module isolated: `kernel/src/abi/`), log, reducer, hooks, router.
  - `libc` — tool loop, infer, patterns. Depends on kernel's public API only.
  - `drivers` — model (Anthropic + OpenAI-compatible/vLLM), clock, store, sandbox, ci. One module each; feature-gated.
  - `harness` — boot, config, supervision, CLI entry.
  - `sim` — deterministic simulation harness (test-only; see §8 Tier 2).
- `docs/`: ADRs (MADR format), the layer diagrams from framing, this handoff.
- `CODEOWNERS`: `kernel/src/abi/` requires explicit owner review (the ABI is the constitution now — one file, not 59 guidelines).
- Conventional commits + merge queue + trunk-based development (short-lived branches, no long-running develop branch).

Milestones:
- **M0 (walking skeleton)**: log + reducer + spawn/exit/send/recv + echo driver + one end-to-end test. No hooks, no cancel, no budgets beyond tokens. Runs in a week of evenings, proves the loop.
- **M1**: wait/cancel, budgets full, clock driver, model driver (real), tool loop in libc.
- **M2**: hooks (native + Rule), sandbox driver v0 (subprocess + rlimits), replay CLI (`kernel replay <log>`).
- **M3**: snapshots, blob store + crypto-shredding, driver supervision, CI driver.
- **M4**: C-mode resume (crash → continue), branching with pinned sampling.

---

## 7. DevOps philosophy

Principles (state of the art, applied):
1. **Tiered feedback**: the pipeline is a funnel — seconds locally, minutes on PR, deep verification on demand or by path, exhaustive verification on a schedule. Nobody waits for fuzzing to merge a typo fix.
2. **Trunk-based + merge queue**: main is always releasable; the merge queue re-runs Tier 1 on the merged state, killing "green on branch, red on main."
3. **Determinism is a tested property, not a hope**: this project's core promise (replay) gets its own CI jobs that would fail loudly on drift.
4. **Path- and label-aware depth**: touching `kernel/src/abi/` or the reducer triggers deep jobs automatically; a docs change runs almost nothing.
5. **Drift is caught by schedule, not by surprise**: dependencies, toolchains, benchmarks, and flakiness are monitored by cron jobs that open issues, so drift arrives as a ticket instead of a broken Friday.
6. **Reproducibility**: pinned toolchain (`rust-toolchain.toml`), `Cargo.lock` committed, `--locked` everywhere in CI, caching keyed on lockfile.

Toolbelt: `cargo nextest` (runner), `clippy` (pedantic on kernel), `rustfmt`, `cargo-deny` (advisories/licenses/bans), `cargo-semver-checks` (API breaks), `proptest` (properties), `cargo-fuzz` (fuzzing), `criterion` + `iai-callgrind` (benches: wall + instruction counts), `cargo-llvm-cov` (coverage), `release-plz` or `cargo-dist` (releases), `insta` (snapshot tests for serialized envelopes).

---

## 8. The pipeline, tier by tier

### Tier 0 — local, sub-minute (developer loop)
Pre-commit (via `just` + optional git hook, never mandatory-slow):
`cargo fmt --check` · `cargo clippy --workspace -- -D warnings` · `cargo nextest run --workspace --profile quick` (unit tests only, `< 30s` budget enforced by nextest per-test timeout). A `just watch` task runs the same on save. Rule: any test slower than 5s is not a unit test and moves to Tier 1/2 suites.

### Tier 1 — every PR + merge queue, target < 8 min (Linux; macOS smoke)
Runs always, blocking:
1. fmt + clippy (workspace, `-D warnings`; kernel crate additionally `clippy::pedantic` allowlist).
2. `cargo nextest run --workspace` — unit + integration, both crates' doc tests.
3. **Fast determinism check**: run the sim harness with 3 fixed seeds, ~10k events each; fold the log twice in-process; assert identical state hashes. This is the canary for reducer nondeterminism and costs ~seconds.
4. **ABI guard** (the project-specific job): `insta` snapshot tests of serialized `Msg`/grant types + `cargo-semver-checks` on the kernel crate + a diff gate: changes under `kernel/src/abi/` fail unless the PR also bumps the `abi` constant or carries an `abi-change` label with a linked ADR. CODEOWNERS forces review on top.
5. `cargo-deny check` (advisories, licenses, duplicate/banned deps).
6. Coverage via `cargo-llvm-cov` with a *ratchet* (delta may not drop >0.5%), not an absolute bar — absolute bars rot into gaming.
7. macOS job: build + `nextest run` unit-only (smoke). Full macOS parity lives in Tier 2; day-to-day macOS breakage is almost always linkage/path issues that smoke catches.

Merge queue re-runs 1–4 on the merge result. Branch protection: no direct pushes to main, queue only.

### Tier 2 — deep verification (label `deep-ci`, or auto on paths: `kernel/**`, `drivers/sandbox/**`)
Runs on demand and always before a release tag:
1. **Property tests, cranked**: proptest suites with 10^4–10^5 cases. Core properties: budgets across any tree always sum to the root grant; ns-subset invariant holds under any spawn/transfer sequence; cancel leaves no live descendants and no leaked reservations; every recv resolution references an existing entry.
2. **Deterministic simulation soak** (FoundationDB-style, the crown jewel): the `sim` crate drives the kernel with a seeded random workload (spawns, sends, cancels racing exits, driver failures, injected WouldBlock) against fake drivers and a virtual clock, 10^6+ events, then (a) refolds and compares state hash, (b) replays on a *second platform build* (macOS runner refolds the Linux-produced log) and compares — catching platform-dependent nondeterminism (HashMap ordering, float formatting).
3. **Fuzzing, short**: `cargo-fuzz` 15-min jobs on envelope deserialization, log-file parsing, Rule-DSL parsing, tool-call JSON handling (model output is attacker-controlled input — fuzz the exact path it enters).
4. **Full macOS matrix**: complete test suite + sim smoke on macOS; both stable and beta toolchains on Linux.
5. **Sanitizers**: ASan/LSan test pass (nightly toolchain job) on the kernel; TSan on the queue/scheduler layer via every crate that boots a `Kernel`; ASan/LSan again over the sandbox driver and its shim, the one place the system runs code it did not write (#61).
6. `cargo nextest run` with `--test-threads 1` once — surfaces order-dependent tests early.
7. Benchmarks compiled (not judged) to catch bench rot.

### Tier 3 — scheduled (cron), drift and endurance
Nightly:
1. **Determinism drift sentinel**: refold a *stored corpus* of historical logs (grown from releases + interesting sim runs, committed as fixtures) with today's main; any state-hash change = the reducer's behavior drifted for old inputs = red alert issue. This is the project's single most important scheduled job: it is "we do not break userspace," executable.
2. **Long fuzz**: 4–8h continuous on the Tier 2 targets, corpus persisted between runs (or OSS-Fuzz if/when open-sourced).
3. **Dependency drift**: `cargo update` on a throwaway branch → full Tier 1; failures open an issue with the offending crate. Plus `cargo-deny` advisories (new CVEs arrive independent of code changes). Renovate/Dependabot for lockfile PRs, batched weekly.
4. **Toolchain canary**: build + test on beta and nightly Rust; failures are early warning, non-blocking, auto-issue.
5. **Benchmark regression**: criterion + iai-callgrind against stored baselines; wall-clock thresholds are noisy on shared runners, so instruction-count (iai) is the gating metric, wall-clock is informational. Reducer throughput (entries/sec) and send→deliver latency are the tracked KPIs; >5% instruction regression opens an issue with the flamegraph artifact attached.
6. **Flake hunter**: rerun the full suite 5× with random `--test-threads`; any test with <100% pass rate gets auto-quarantined (nextest flaky-test detection) and an issue.
Weekly: sim soak at 10^7 events; `cargo udeps` (unused deps); doc build + link check; license/SBOM refresh (`cargo-cyclonedx`); MSRV verification.

### Release lane (tag-triggered)
Tier 2 full → `cargo-dist` builds signed artifacts for `x86_64/aarch64-{linux-gnu,apple-darwin}` → `release-plz` changelog from conventional commits → the release's sim corpus snapshot is added to the Tier 3 determinism fixtures (each release permanently extends the backward-compat oracle).

### Why this shape (the reasoning, for your future recalibration)
- Speed budget math: Tier 1 must stay under ~8 min or people batch commits and feedback quality collapses; everything expensive is therefore pulled *out* of the PR path and pushed *down* the funnel — but never deleted, only rescheduled.
- Path-triggering encodes the architecture: the frozen ABI and the reducer get automatic deep scrutiny because that is where a quiet mistake costs the most; drivers and libc are cheap to fix later, so they ride the fast lane.
- The determinism jobs (T1.3 fast, T2.2 deep, T3.1 sentinel) are the same test at three intensities — one property, three budgets. That pattern (one invariant, tiered enforcement) is the template for adding future invariants.
- Scheduled drift jobs open issues instead of failing builds because drift is not the committer's fault; blaming the wrong human trains people to ignore red.

---

## 9. Day-one checklist

1. Run the §5 choreography in one sitting: tombstone commit into v1 → rename to `tau-legacy` + archive → create fresh `tau-rs/tau` → initial commit with this file as `docs/HANDOFF.md`. ADRs: 0001 "tau, rebooted" (drift diagnosis, link to tau-legacy), ADR-0002 (seven syscalls + irreducibility rule), ADR-0003 (substrate A+B, C path, D quarantine), ADR-0004 (ABI freeze + versioning policy).
1b. Reserve `tau-kernel` and `libtau` on crates.io (`0.0.0` placeholders); email the dormant `tau` crate owner about a transfer.
2. `rust-toolchain.toml` (pinned stable), `Cargo.lock` committed, `just` + `nextest` config, pre-commit hook (Tier 0).
3. CI skeleton: Tier 1 workflow + merge queue + branch protection + CODEOWNERS on `kernel/src/abi/` — before any real code, so the first real PR already flows through it.
4. `kernel/src/abi/` with `Msg` + grant types + `abi: u16 = 0` + insta snapshots (the ABI guard is live from commit ~3).
5. M0 walking skeleton behind the pipeline.
6. Tier 2 workflow lands with the sim crate (M0+); Tier 3 crons land once main has a week of history to drift from.

## 10. Deferred (tracked, not forgotten)
Multi-tenant authn/z at the harness boundary · WASM hook programs · distro/packaging story (the "Alpine/Ubuntu harnesses") · C-mode durable execution (M4) · controller research thread (PID-on-test-results from tau — now expressible as an OnBudget/PreDeliver hook pair + Emit; revisit after M2 when hooks exist to host it).