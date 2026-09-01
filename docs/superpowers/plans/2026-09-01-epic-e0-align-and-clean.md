# E-0 — Align & Clean Implementation Plan (Phase 0)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The repo stops contradicting the 2026-09-01 redesign, with **zero behavior change**: the ADR wave + doc amendments are merged (landed with the backlog PR train — verify, don't re-do), `tau-workflow` and the dead weight (`tau-plugin-base`, `landlock-exec-repro`, `embed_c` stubs, stale examples/refs) are deleted, and `ARCHITECTURE.md` + its freshness gate stay honest.

**Architecture:** Pure subtraction + documentation. Each deletion is one task: remove the crate/dir, remove every workspace/CLI/docs reference, update `ARCHITECTURE.md`, keep CI green. The ADR wave (ADR-0071..0077), backlog edits, ROADMAP retirements, CONSTITUTION G6/QG12 + cheatsheet, and `tau-philosophy.md` amendments are delivered by the backlog session's PR (`docs(plans)`/`docs(decisions)` commits); Task 1 verifies they are merged before any deletion starts, so every removal has its argued paper trail in place.

**Tech Stack:** Rust workspace surgery (`Cargo.toml` members, feature refs), `cargo nextest`, mdBook (`docs/` per DOCS RULES), `xtask/tests/architecture_md.rs`.

**Design:** [`../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md`](../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md) §11 Phase 0, §8, §10.8.
**ADRs:** [0071](../../decisions/0071-three-surface-split.md) · [0072](../../decisions/0072-synth-contract.md) · [0073](../../decisions/0073-ir-v3-multi-pipeline.md) · [0074](../../decisions/0074-journal-record-substrate.md) · [0075](../../decisions/0075-ops-lane-local-first.md) · [0076](../../decisions/0076-agentic-instruction-set.md) · [0077](../../decisions/0077-agent-exposure-surfaces.md)
**Tree:** [`../implementation-trees/authoring-surfaces.md`](../implementation-trees/authoring-surfaces.md)

## Global Constraints

- Every cargo command: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p <crate> <filter>` (repo CARGO RULES; `timeout 180` + `cargo check` for checks; `timeout 240` for clippy; never bare cargo; never workspace-wide `--workspace`, but after member removals a scoped `cargo check -p tau-cli` + `-p xtask` is the verification unit).
- Commit with explicit identity (lefthook tests can corrupt worktree git identity): `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "..."`.
- **Zero behavior change** is the epic's contract: no runtime code path may change meaning. Deletions only, plus reference cleanup. If a deletion forces a behavior decision, STOP and file it against E-1/E-2 instead.
- ISSUE RULES: before each task, `gh pr list --search "<crate name> in:title" --state all` — parallel sessions may already be deleting the same thing.
- Docs: every removed page must also leave `docs/SUMMARY.md` (mdbook silently skips non-SUMMARY pages, but linkcheck fails on links *to* deleted pages — grep inbound links before deleting). DOCS RULES: build the book before any docs-touching PR.
- Historical process artifacts (`docs/superpowers/plans/2026-05-12-tau-workflow.md`, old specs, ADR-0022 text, ROADMAP phase records) are **never deleted** — they are the record. Only *live* docs (explanation pages, SUMMARY entries, README/ARCHITECTURE references) are updated.

---

### Task 1: Verify the paper trail is merged (gate task)

**Files:**
- Read-only check: `docs/decisions/0071..0077-*.md`, `docs/decisions/README.md` (collision note), `docs/superpowers/plans/vision-roadmap.md` (E-0..E-4 section), `ROADMAP.md` (killed-item narrowing), `CONSTITUTION.md` (G6/QG12 amendment stamps), `docs/explanation/tau-philosophy.md` (three-surface §).

**Steps:**
- [ ] **Step 1:** Confirm each file above exists on `main` with the 2026-09-01 content (the backlog PR train merged). If any is missing, STOP — this epic's deletions must not land before their ADRs.
- [ ] **Step 2:** Run the doc gates locally: `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build` → only `[INFO]` lines; `rm -rf docs/book`.
- [ ] **Step 3:** No commit (verification only). Record the verification in the PR description of Task 2's PR.

### Task 2: Delete the `tau-workflow` crate + CLI verbs

**Files:**
- Delete: `crates/tau-workflow/` (whole crate)
- Delete: `crates/tau-cli/src/cmd/workflow/` (`mod.rs`, `run.rs`, `log.rs`, `resume.rs`, list)
- Modify: `Cargo.toml` (workspace `members` — remove `crates/tau-workflow`; `[workspace.dependencies]` entry), `crates/tau-cli/Cargo.toml`, `crates/tau-cli/src/lib.rs` (the `prepare_workflow_run_layer` / minted-run-id wiring around lines 36–62 and the dispatch arm), `crates/tau-observe/Cargo.toml` (dev-dep cycle noted at its line ~45–47) + any `tau_observe::layers::workflow_run_log` consumers
- Modify: `ARCHITECTURE.md` (drop the `tau-workflow` row), `CHANGELOG.md` (removal entry naming ADR-0022's banner)

**Interfaces:**
- Removes: `tau workflow {list,run,log,resume}` CLI verbs; `tau-workflow` public API. Removal is sanctioned by ADR-0022's superseded banner (superseded twice; no deprecation cycle needed — design §10.8).
- Must NOT remove: `tau_runtime::Runtime::invoke_tool` (other callers exist — verify with `rg "invoke_tool" crates/ --type rust` before touching).

**Steps:**
- [ ] **Step 1 (red):** Add a tombstone test in `crates/tau-cli/tests/` asserting `tau workflow` is an **unknown subcommand** (clap error, exit ≠ 0, message does not panic). It fails while the verb still exists.
- [ ] **Step 2:** Delete the crate + cmd dir; strip the workspace member, deps, and the run-id layer wiring in `lib.rs`; fix `tau-observe`'s dev-dep (inline or delete the cycle-noted test helper — check what `workflow_run_log` layer tests need; the *layer* stays only if a non-workflow consumer exists, else it goes too).
- [ ] **Step 3 (green):** `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-cli && ... -p tau-observe`; then `timeout 300 ... cargo nextest run -p tau-cli workflow`; tombstone green.
- [ ] **Step 4:** `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p xtask` (architecture gate—forward-only, but run it) after updating `ARCHITECTURE.md`.
- [ ] **Step 5:** Commit: `refactor(cli,workflow)!: delete tau-workflow v1 (superseded twice; ADR-0022 banner)`.

### Task 3: Retire the tau-workflow docs surface

**Files:**
- Modify: `docs/explanation/workflows.md` (replace body with a short tombstone: what v1 was, why it's gone — ADR-0022 banner, ADR-0071/0073 — and where flow now lives: `pipelines/` + IR; keep the page so inbound links survive)
- Modify: `docs/SUMMARY.md` (retitle the entry "Workflows (superseded)"), any live explanation pages linking to workflow verbs (`rg -l "tau workflow" docs/ --glob '!superpowers/**'`)

**Steps:**
- [ ] **Step 1:** `rg -n "tau workflow|workflows.md" docs/ --glob '!superpowers/**' --glob '!decisions/**'` — inventory inbound links.
- [ ] **Step 2:** Write the tombstone page; fix inbound links; keep historical specs/plans untouched.
- [ ] **Step 3:** `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build` clean; `rm -rf docs/book`. Commit `docs(workflows): tombstone the tau-workflow v1 page`.

### Task 4: Delete `landlock-exec-repro`

**Files:**
- Delete: `crates/landlock-exec-repro/` (has its own `Cargo.lock` — a standalone repro, not a workspace member; verify with `rg "landlock-exec-repro" Cargo.toml` = no hit)
- Modify: any referencing docs/comments (`rg -n "landlock-exec-repro" --hidden -g '!.git' -g '!docs/superpowers/**'`), `ARCHITECTURE.md` if named

**Steps:**
- [ ] **Step 1:** Confirm nothing builds it: not a workspace member; check `.github/workflows/` for a dedicated job (`rg landlock-exec-repro .github/`).
- [ ] **Step 2:** Delete; clean references; run `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p xtask`.
- [ ] **Step 3:** Commit `chore(sandbox): delete landlock-exec-repro (issue resolved upstream; dead weight per design §11)`.

### Task 5: Delete the `embed_c` stubs

**Files:**
- Delete: `crates/tau-sdk-codegen/src/embed_c.rs`
- Modify: `crates/tau-sdk-codegen/src/lib.rs` (drop `pub mod embed_c;` + re-exports, line ~11/21), `crates/tau-cli/src/cmd/embed.rs` (the `--host c` arm at ~line 81 → replaced by a **named unsupported error** pointing at the wasm component + WIT path — an explicit error message is not a behavior change, it replaces a stub emitting dead glue)
- Modify: docs mentioning `tau embed --host c` (`rg -n "embed --host c|embed_c" docs/ --glob '!superpowers/**'`), `CHANGELOG.md`

**Steps:**
- [ ] **Step 1 (red):** CLI test: `tau embed --host c` exits non-zero with the named `EmbedHostUnsupported`-style message (exact existing error enum — locate via `rg "enum.*Error" crates/tau-cli/src/cmd/embed.rs`). Fails while the stub renders.
- [ ] **Step 2:** Delete + rewire; `timeout 300 ... cargo nextest run -p tau-sdk-codegen` and `-p tau-cli embed`.
- [ ] **Step 3:** Commit `refactor(sdk-codegen)!: delete embed_c stubs (unbuilt surface; C consumers ride the wasm component + WIT)`.

### Task 6: Decide-and-delete `tau-plugin-base` references (Docker base image)

**Files:**
- Delete: `crates/tau-plugin-base/` (a Dockerfile dir, not a crate — `is_crate()` is false for it, so the architecture gate ignores it)
- Modify: `crates/tau-sandbox-container/src/runner.rs` doc-comments (lines ~18/34/182 reference the base image), `docker/` + `.github/workflows/` image-build jobs (`rg -ln "tau-plugin-base" .github docker`), `docs/explanation/sandboxing.md`, per-plugin-images docs kept historical

**Steps:**
- [ ] **Step 1:** Inventory: `rg -n "tau-plugin-base" --hidden -g '!.git' -g '!docs/superpowers/**'`. If a CI job still *builds* the image for the container sandbox tier, the deletion is NOT dead weight — STOP and confirm scope against design §11's "dead weight" list before proceeding (record the finding in the PR either way).
- [ ] **Step 2:** Delete dir + references (or, per Step-1 finding, narrow to genuinely dead references); update `sandboxing.md` prose.
- [ ] **Step 3:** CI must stay green on the container-sandbox jobs; commit `chore(sandbox): remove tau-plugin-base <scope per step 1>`.

### Task 7: Sweep stale examples & references

**Files:**
- Review: `examples/dev-smoke-fan-monitor`, `examples/dev-smoke-fan-monitor-ts`, `examples/streaming-demo` — against ADR-0071 surfaces
- Modify: only what contradicts the design *today* (e.g. an example advertising `[steps]` or TS-defined agents as the way in). `dev-smoke-fan-monitor-ts` exercises the ADR-0041 lane which E-1 deletes: mark it clearly (README banner in the example dir) as legacy-lane, scheduled for E-1 removal — do not delete yet (it still smoke-tests the shipping code).

**Steps:**
- [ ] **Step 1:** `rg -n "\[steps\]|native\s*=" examples/ docs/how-to docs/tutorials docs/reference 2>/dev/null` — inventory live-doc contradictions; fix prose to name the deprecation (ADR-0071) without changing shipped syntax.
- [ ] **Step 2:** Add the legacy-lane banner to `examples/dev-smoke-fan-monitor-ts/`; verify referenced smoke jobs stay green.
- [ ] **Step 3:** `mdbook build` clean; commit `docs(examples): mark legacy-lane examples; align live docs with ADR-0071`.

### Task 8: Epic close-out

**Steps:**
- [ ] **Step 1:** Full scoped check battery over touched crates: `tau-cli`, `tau-observe`, `tau-sdk-codegen`, `xtask` (check + nextest + clippy, per Global Constraints).
- [ ] **Step 2:** Update the [authoring-surfaces tree](../implementation-trees/authoring-surfaces.md): flip E-0 nodes to ✅ with PR numbers; log discoveries (anything Step 1 of Tasks 4/6 surprised you with).
- [ ] **Step 3:** Update `vision-roadmap.md` E-0 stories to ✅; verify epic DoD: repo contradicts nothing in the design; CI green; zero behavior change (the `embed --host c` named error being the one sanctioned message change).
