# E-4 — Local Ops Implementation Plan (Phase 4)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The machine is environment `local`: builds pin into a committed secret-free state file, `tau apply` applies atomically and emits systemd-user timer adapters from `[trigger]` declarations, `[[moved]]` records drive rename-not-replace in plan and checkpoint/journal remap on resume, `tau-lock.toml` v8 carries `[synth]` provenance, and the remaining design-§3.4 repairs land. **DoD:** north-star-v2 applied, scheduled by timer, and resumed after a rename via its moved record; wasm bundles are run-or-refuse per environment.

**Architecture:** The pin is `.tau/envs/local.state.toml` with the ADR-0075 field set (`ir_hash`, `bundle_path`, `applied_at`, `ir_format`, `lockfile_hash`, adapter unit names, applying `tau` version), parsed with the lockfile's additive-versioning discipline. `tau apply` = build (or take `--bundle`) → plan against the pin → write bundle + pin + adapters in one atomic sequence (temp + rename; partial failure rolls back the pin). Adapter emission follows ADR-0043: `[trigger]` cron declarations compile to systemd-user `.timer`/`.service` units invoking `tau run`; retry-policy encoding is deliberately absent in v1 (ADR-0075). `[[moved]]` lives in `tau.toml`, validated at parse, consumed by plan rendering and by resume-time id remap. Lockfile v8 adds the `[synth]` section (`crates/tau-pkg/src/lockfile.rs` version-bump precedent).

**Tech Stack:** Rust; systemd-user unit files (no daemon API — emit + `systemctl --user` invocations documented, not shelled by default); `cargo nextest`.

**Design:** [`../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md`](../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md) §5, §3.4, §12.
**ADRs:** [0075](../../decisions/0075-ops-lane-local-first.md) · [0074](../../decisions/0074-journal-record-substrate.md) (remap) · [0043](../../decisions/0043-trigger-ingress.md) (adapter doctrine) · [0076](../../decisions/0076-agentic-instruction-set.md) (repairs mandate).
**Tree:** [`../implementation-trees/ops-lane.md`](../implementation-trees/ops-lane.md)

## Global Constraints

- Every cargo command: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p <crate> <filter>` (repo CARGO RULES; never bare cargo; never workspace-wide).
- Commit with explicit identity: `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "..."`.
- The pin file is **committed and secret-free by construction**: no env values, no credentials, no absolute home paths (bundle path is repo-relative; a test greps the serialized pin for `/home/|/Users/` and fails on match).
- State/lockfile formats: additive versioning with once-per-process warn on older versions (the lockfile v3→v4 model); v8 is additive — v7 lockfiles load unchanged.
- Systemd tests must not require a running user session in CI: unit-file emission is golden-file-tested; `systemctl` interaction sits behind a port faked in tests (the ProcessGate/ADR-0062 pattern).
- Every repair in Task 7 cites its design-§3.4 line in the commit message; each repair is its own commit, independently green.
- ISSUE RULES sweep before each task.

---

### Task 1: Environment model + the pin file

**Files:**
- Create: `crates/tau-pkg/src/envs.rs` (`EnvState` per the ADR-0075 field set; `load/store` for `.tau/envs/local.state.toml`; env name `local` implicit — one env until v2)
- Test: inline round-trip + versioning tests

**Steps:**
- [ ] **Step 1 (red):** Round-trip; unknown-field tolerance per interchange versioning (state file is tool-written interchange — version-gated per ADR-0065, warn-not-reject on newer minor); the secret-free grep test; repo-relative bundle path enforced.
- [ ] **Step 2:** Implement; green (`-p tau-pkg envs`).
- [ ] **Step 3:** Commit `feat(pkg): env local pin state (ADR-0075 field set)`.

### Task 2: `tau plan` reads the pin

**Files:**
- Modify: `crates/tau-cli/src/cmd/plan.rs` (default target = the local pin when present; `--against` stays for pinless use), docs page from E-3 updated

**Steps:**
- [ ] **Step 1 (red):** Fixture: pinned bundle + changed source → plan exits 2/3 appropriately with the pin as baseline; no pin + no `--against` → named error suggesting `tau apply`.
- [ ] **Step 2:** Implement; green; commit `feat(cli): plan defaults to the local pin`.

### Task 3: `tau apply` — atomic per repo

**Files:**
- Create: `crates/tau-cli/src/cmd/apply.rs` (build→plan→confirm (or `--yes`/`--check` for exit-code-only)→write bundle+pin atomically; `--pipeline` slicing valve per ADR-0075 — slices the *adapter set*, never the bundle)
- Test: CLI integration — apply twice idempotent (second exits 0 "no change"); induced failure mid-write leaves the previous pin intact (temp+rename atomicity test)

**Steps:**
- [ ] **Step 1 (red):** The idempotence + atomicity fixtures; apply refuses when plan reports exit 3 unless `--allow-widening` (the CI-gate semantic carried to the local verb).
- [ ] **Step 2:** Implement; green; commit `feat(cli): tau apply — atomic pin + bundle (+ --pipeline valve)`.

### Task 4: Trigger adapters — systemd-user timers

**Files:**
- Create: `crates/tau-cli/src/cmd/apply/adapters.rs` (emit `.timer`/`.service` per `[trigger]` cron/manual decl; unit names recorded in the pin; ADR-0043 doctrine; NO retry-policy encoding in v1 — a comment in the emitted unit cites ADR-0075)
- Test: golden unit files for the north-star-v2 triggers; unit-name collision/rename handling via pin diff

**Steps:**
- [ ] **Step 1 (red):** Goldens; a removed trigger removes its unit from the emitted set (and the pin's list names the delta for cleanup output).
- [ ] **Step 2:** Implement; green; document the `systemctl --user enable --now` handoff in the how-to (apply prints it; doesn't execute by default).
- [ ] **Step 3:** Commit `feat(cli): apply emits systemd-user timer adapters from [trigger]`.

### Task 5: `[[moved]]` records

**Files:**
- Modify: `crates/tau-pkg/src/project/project.rs` (`[[moved]] from/to/at` parse + validation: known kinds, `to` exists, `from` absent, ADR-0070 grammar both sides), `crates/tau-cli/src/cmd/plan.rs` (rename-not-replace rendering), resume path (`crates/tau-runtime-core` checkpoint load + journal replay id remap keyed by moved records)
- Test: plan fixture (rename with record → one rename line, no delete+add; without → delete+add loud); resume fixture (checkpointed run resumes across a rename via the record)

**Steps:**
- [ ] **Step 1 (red):** The parse, plan-rendering, and resume fixtures.
- [ ] **Step 2:** Implement; green across `tau-pkg`, `tau-cli`, `tau-runtime-core`.
- [ ] **Step 3:** Commit `feat(pkg,cli,runtime-core): [[moved]] rename records — plan + resume remap`.

### Task 6: Lockfile v8 — `[synth]` provenance

**Files:**
- Modify: `crates/tau-pkg/src/lockfile.rs` (v8: `[synth] sdk_version, gen_hash, fragments = [{name, source, resolved_sha}]`; additive)
- Test: v7 loads unchanged (compat fixture); v8 round-trips; `tau build` on a synth project writes the section; a TOML-only project writes none

**Steps:**
- [ ] **Step 1 (red):** The compat + round-trip + presence/absence fixtures.
- [ ] **Step 2:** Implement; green; commit `feat(pkg): lockfile v8 — [synth] provenance (additive)`.

### Task 7: The remaining repairs lot (one commit each)

**Files/Interfaces (anchor each via `rg` before editing; cite design §3.4 per commit):**
- `AgentBudget.max_tokens` parsed-but-never-read → enforced at the LLM boundary (named budget-exceeded error/event).
- `judge_model` accepted-but-no-op → judge calls resolve through it (per-judge model resolution exists — ADR-0052 path).
- `output_schema` runtime-inert → wire to structured output/validation at the agent boundary (failure = named validation error, retryable per goals policy).
- Goals hardcoded `OnFail::Abort` → retry authorable (bounded, per the quality-plane semantics).
- Subflow args not forwarded → forwarded + conformance fixture.
- Capability-order-sensitive hashes → canonical ordering before hash (bundle-hash shift: `!` commit + CHANGELOG).
- Decorative `max_concurrency` → honored as `min(declared, host cap)` + reported in run events (ADR-0076 PARALLEL_CAP ruling).

**Steps:**
- [ ] **Step 1:** For each: failing test reproducing the silent promise → fix → green → commit (`fix(...): <repair> (design §3.4)`). Order free; keep commits independent.
- [ ] **Step 2:** Cross-check §3.4's full list against what E-2/E-3 already fixed (`any-wasi-strict`, RunEvents, cassette keying, scalar coercion, predicate/structured access, stale comments) — anything left lands here.

### Task 8: Run-or-refuse verification + epic close-out

**Steps:**
- [ ] **Step 1:** Conformance fixture: a wasm bundle applied into an env whose host tier can't satisfy a declared structural capability → **refused with the named error at apply/run, never silently narrowed** (ADR-0075 rule 4).
- [ ] **Step 2:** **Epic DoD end-to-end:** north-star-v2 `tau apply` → timer-scheduled → rename a pipeline with `[[moved]]` → resumed run picks up via remap. Scripted as a conformance/e2e test where CI permits; otherwise a documented manual runbook + the component fixtures above.
- [ ] **Step 3:** Update the [ops-lane tree](../implementation-trees/ops-lane.md) + `vision-roadmap.md` E-4 stories; mark v1 complete in the roadmap arc note (north-star-v2 on the new stack, original north-star green).
