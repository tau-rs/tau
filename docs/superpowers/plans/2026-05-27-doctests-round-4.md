# Doctests Round 4 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `///` doctest fences to the load-bearing subset (~55 items) of bare public items across the 3 in-scope tier-2 crates (`tau-workflow`, `tau-app`, `tau-observe`), so `cargo doc` renders usage examples at every API entry point.

**Architecture:** Three PRs, one per crate, in ascending bare-count order. Each PR (a) audits its crate, (b) classifies each bare public item per spec §3 (include / skip-*), (c) writes doctests reusing round-2/3 fixture patterns, (d) extends a new round-4 inventory file. **No stacking** — each PR branches off current `main` only after the previous merges.

**Tech Stack:** Rust 2021, `cargo test --doc`, `tau_ports::fixtures::{MockLlmBackend, make_completion_response, make_token_usage}`, `tau_domain::fixtures::*`, `tempfile`, `tracing-subscriber`, `tokio-test`. No new workspace dependencies expected.

---

## Spec reference

Implements `docs/superpowers/specs/2026-05-27-doctests-round-4-design.md`. Key constraints recapped:

- **§3.1 — include:** constructors, public trait impls, methods returning non-trivial `Result<T,E>`, methods with 2+ non-self params or generics, free fns, enum variants with data, non-trivial conversions.
- **§3.2 — skip:** trivial getters, `Default::default()`, derived impls, type aliases, trivial setters, marker traits, `Display`/`Debug` impls, re-exports.
- **§3.3 — in doubt, include.**
- **§4 — style:** bare ` ``` ` default; `no_run` only for forbidden side effects; no `.unwrap()` on meaningful Results — use `.expect("msg")`; hidden `# `-prefixed setup encouraged.
- **§5 — no stacking:** each PR opens after its predecessor merges, branched fresh off main.
- **§6 — inventory:** every public item gets a row, including `pub fn` inside `impl` blocks. Use the **broader grep**:

  ```bash
  git grep -nE '^\s*pub (fn |async fn |struct |enum |trait |type |const |mod )' -- 'crates/<crate>/src/' | grep -v 'pub(crate)'
  ```

- **§8 — provisional `skip-binary-internal` category** for tau-app only.
- **§9 — success criteria:** doctests pass; ≥1 fixture example per crate; no `?`/`TBD` rows.

---

## Pre-flight (do this once, before Task 1)

- [ ] **Step 0.1: Verify spec branch state**

```bash
cd /Users/titouanlebocq/code/tau-worktrees/doctests-round-4-spec
git status -sb
```

Expected: `## feat/doctests-round-4-spec...origin/main` ahead by 1 commit (the spec).

- [ ] **Step 0.2: Verify baseline bare-item counts on main**

```bash
for crate in tau-workflow tau-observe tau-app; do
  pubs=$(git grep -hE '^\s*pub (fn |async fn |struct |enum |trait |type |const |mod )' "crates/$crate/src/" 2>/dev/null | grep -v 'pub(crate)' | wc -l | tr -d ' ')
  fences=$(git grep -c '```' -- "crates/$crate/src/" 2>/dev/null | awk -F: 'BEGIN{s=0} {s+=$NF} END {print s+0}')
  echo "$crate: pub_items=$pubs, doctest_blocks=$((fences/2))"
done
```

Expected (within ±2):

```
tau-workflow: pub_items=19, doctest_blocks=0
tau-observe: pub_items=59, doctest_blocks=1
tau-app: pub_items=44, doctest_blocks=0
```

If counts drift significantly, another session may have landed doctests — adjust per-task estimates accordingly.

- [ ] **Step 0.3: Push PR-spec, enable auto-merge**

```bash
git push --no-verify -u origin feat/doctests-round-4-spec
gh pr create --title "docs(spec+plan): doctests round 4" --body "$(cat <<'EOF'
## Summary
Spec + plan for round 4 of "doctests in /// comments" — extending coverage to tier-2 crates.

- [Spec](../docs/superpowers/specs/2026-05-27-doctests-round-4-design.md)
- [Plan](../docs/superpowers/plans/2026-05-27-doctests-round-4.md)

**Scope:** tau-workflow, tau-observe, tau-app. tau-infra excluded (0 public items).

**Estimate:** ~55 new fences across 3 PRs.

Per spec §5 stacking discipline, this PR lands first; each per-crate implementation PR branches off `main` only after the previous merges.

## Test plan
- [x] Docs-only — no code changed.
- [ ] CI green.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
gh pr merge --auto --squash
```

After PR-spec merges, Task 1 (PR-A tau-workflow) starts in a fresh worktree off main.

---

## Task 1: PR-A — tau-workflow (~10 examples)

**Branch:** `feat/doctests-round-4-workflow`, branched from `origin/main` AFTER PR-spec merges.

**Files:**
- Modify: 2-5 files in `crates/tau-workflow/src/`.
- Create: `docs/superpowers/inventories/2026-05-27-bare-items-round-4.md` (inventory skeleton).
- Possibly modify: `crates/tau-workflow/Cargo.toml` (dev-deps if fixture examples need them).

- [ ] **Step 1.1: Create the worktree (after PR-spec merges)**

```bash
cd $(git worktree list | grep -v '(bare)' | head -1 | awk '{print $1}')
git fetch origin main --quiet
git worktree add /Users/titouanlebocq/code/tau-worktrees/doctests-round-4-workflow -b feat/doctests-round-4-workflow origin/main
cd /Users/titouanlebocq/code/tau-worktrees/doctests-round-4-workflow
```

- [ ] **Step 1.2: Enumerate the crate's public items (broader grep)**

```bash
git grep -nE '^\s*pub (fn |async fn |struct |enum |trait |type |const |mod )' -- 'crates/tau-workflow/src/' | grep -v 'pub(crate)' > /tmp/pub-items-workflow.txt
wc -l /tmp/pub-items-workflow.txt
```

Expected: ~19-30 lines (more than 19 if impl-block methods exist).

- [ ] **Step 1.3: Cross-reference against existing doctest blocks**

For each item in `/tmp/pub-items-workflow.txt`, open the file at the listed line and read ~25 lines back to check for an existing fence in the preceding `///` block.

- [ ] **Step 1.4: Classify each item per spec §3**

For each item: `include` (per §3.1), `skip-*` (per §3.2), or `done` (already has a fence). In doubt → include (§3.3).

**For tau-workflow specifically**, expected items + classifications:

| Item kind | Default classification |
|---|---|
| `pub fn load_workflow(...) -> Result<Workflow, …>` (or similar TOML loader) | `include` per §3.1 (non-trivial Result, FS-touching → hidden setup uses `tempfile::tempdir()`) |
| `Workflow::run(...)` / `run_pipeline(...)` style entry points | `include` — likely `no_run` because they execute real runtime+tools; OR with `MockLlmBackend` fixture if reachable |
| `WorkflowError` variants | `include` per §3.1 enum-variant rule (each error variant doc) |
| `Workflow` struct + getters | struct → `include`; getters → `skip-getter` |
| `pub fn list(...)` / `pub fn log(...)` / `pub fn resume(...)` | `include` if returns Result; otherwise classify per §3 |

- [ ] **Step 1.5: Create the inventory file**

Write `docs/superpowers/inventories/2026-05-27-bare-items-round-4.md` with this exact opening:

```markdown
# Bare-item coverage inventory — round 4

**Source:** tier-2 crate audit on 2026-05-27.
**Spec:** `docs/superpowers/specs/2026-05-27-doctests-round-4-design.md`.
**Plan:** `docs/superpowers/plans/2026-05-27-doctests-round-4.md`.

## Categories

- **include**: classification per spec §3.1 — adds a `///` doctest fence in this PR.
- **skip-trivial**: trivial item not requiring an example.
- **skip-getter / skip-setter**: trivial accessor / mutator.
- **skip-derived**: derived trait impl.
- **skip-alias**: `pub type X = Y`.
- **skip-display / skip-debug**: `Display` / `Debug` impl.
- **skip-marker**: marker trait or unit-struct sentinel.
- **skip-reexport**: `pub use`.
- **skip-feature-gated**: behind a cargo feature; doctest would need `--features <flag>`.
- **skip-needs-fixture**: requires non-trivial test fixtures (real sandbox, env injection, multi-thread) that exceed reasonable doctest scope.
- **done**: already had a fence before round 4 began.

## tau-workflow

| # | File:line | Item | Classification | Strategy |
|---|---|---|---|---|

(Rows populated by PR-A.)

## Status log
```

Then populate the `## tau-workflow` section: one row per item from `/tmp/pub-items-workflow.txt`.

Append to the Status log:

```markdown

- 2026-05-27 — tau-workflow classifications + <N> includes (PR-A).
```

(Replace `<N>` with the actual include row count after step 1.6.)

- [ ] **Step 1.6: Write doctest fences for `include` rows**

For each `include` row:
- Bare ` ``` ` (executed) by default.
- Visible body 3–8 lines focused on the API.
- At least one `assert_eq!`/`assert!`.
- `.expect("msg")` not `.unwrap()` on meaningful Results.
- For FS-touching examples: hidden `# let tmp = tempfile::tempdir().expect("tempdir");` + use `tmp.path()`.
- For Runtime-flow examples: reuse the round-2/3 `MockLlmBackend` fixture pattern:

```rust
/// ```
/// # tokio_test::block_on(async {
/// # use tau_runtime::{Runtime, RunOptions};
/// # use tau_ports::fixtures::{MockLlmBackend, make_completion_response, make_token_usage};
/// # use tau_domain::fixtures::{any_agent_definition, any_package_manifest, any_message};
/// # use tau_domain::StopReason;
/// # let resp = make_completion_response("hi".into(), vec![], StopReason::EndTurn, Some(make_token_usage(1, 1)));
/// # let llm = MockLlmBackend::new("test-pkg").with_response(resp);
/// # let runtime = Runtime::builder().with_llm_backend(llm).build().expect("build");
/// // visible body using runtime + tau-workflow API
/// # });
/// ```
```

- [ ] **Step 1.7: Verify dev-deps**

```bash
grep -E 'tau-ports.*test-fixtures|tau-domain.*test-fixtures|tokio-test|tempfile' crates/tau-workflow/Cargo.toml
```

Verify these dev-deps exist (or are reachable via transitive features). Add to `[dev-dependencies]` if missing:

```toml
tau-ports = { workspace = true, features = ["test-fixtures"] }
tau-domain = { workspace = true, features = ["test-fixtures"] }
tokio-test = "0.4"
tempfile = { workspace = true }
```

- [ ] **Step 1.8: Run doctests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-workflow
```

Expected: passing count = number of include rows, 0 failed.

- [ ] **Step 1.9: Commit + push + open PR-A**

```bash
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" add crates/tau-workflow/ docs/superpowers/inventories/2026-05-27-bare-items-round-4.md
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "$(cat <<'EOF'
test(workflow): doctest fences for bare public items (round 4)

Round 4 of "doctests in /// comments" — load-bearing bare-item
coverage for tier-2 crates. Adds <N> fenced examples to previously
fenceless public items in tau-workflow, per the inventory at
docs/superpowers/inventories/2026-05-27-bare-items-round-4.md.

Refs: docs/superpowers/specs/2026-05-27-doctests-round-4-design.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
git push --no-verify -u origin feat/doctests-round-4-workflow

gh pr create --title "test(workflow): doctest fences for bare public items (round 4)" --body "Round 4 — see [spec](../docs/superpowers/specs/2026-05-27-doctests-round-4-design.md) and [inventory](../docs/superpowers/inventories/2026-05-27-bare-items-round-4.md). Adds <N> new doctests to load-bearing bare items.

## Test plan
- [x] cargo test --doc -p tau-workflow green locally.
- [ ] CI green.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"

gh pr merge --auto --squash
```

Replace `<N>` with actual include count.

- [ ] **Step 1.10: WAIT for PR-A to merge before Task 2.**

Per spec §5 no-stacking discipline.

---

## Task 2: PR-B — tau-app (~20 examples)

**Branch:** `feat/doctests-round-4-app`, branched from `origin/main` AFTER PR-A merges.

Same procedure as Task 1, with these differences:

- [ ] **Step 2.1: Create the worktree (after PR-A merges)**

```bash
cd $(git worktree list | grep -v '(bare)' | head -1 | awk '{print $1}')
git fetch origin main --quiet
git worktree add /Users/titouanlebocq/code/tau-worktrees/doctests-round-4-app -b feat/doctests-round-4-app origin/main
cd /Users/titouanlebocq/code/tau-worktrees/doctests-round-4-app
```

- [ ] **Step 2.2: Enumerate**

```bash
git grep -nE '^\s*pub (fn |async fn |struct |enum |trait |type |const |mod )' -- 'crates/tau-app/src/' | grep -v 'pub(crate)' > /tmp/pub-items-app.txt
wc -l /tmp/pub-items-app.txt
```

Expected: ~44+ lines.

- [ ] **Step 2.3: Classify per spec §3 + provisional `skip-binary-internal`**

For each item, classify:

- **include**: matches §3.1.
- **skip-*** per §3.2.
- **skip-binary-internal**: ONLY for items that are `pub` for the binary's integration-test reachability but are NOT a stable library API (e.g., command handlers like `cmd_run`, `cmd_init`; internal setup glue). The label is provisional — if no row in tau-app fits, don't use it.

**For tau-app specifically**, expected patterns:

| Item kind | Default classification |
|---|---|
| `cmd_*` command-handler functions | `skip-binary-internal` (with strategy: "binary command handler; not stable embedder API") |
| `Cli` struct (clap derive) | `skip-derived` (clap derives Parser/Args) — but if it has non-derived methods, those classify per §3 |
| Subcommand enum variants | `skip-derived` if clap-derived |
| Helper fns / config loaders / exit-code mappers | `include` per §3.1 if they have non-trivial Result or 2+ params |
| `init` / `main`-style entry points | `skip-binary-internal` |
| Error types | `include` per §3.1 enum-variant rule |
| Path resolution / scope helpers | `include` |

If you use `skip-binary-internal`, ADD it to the Categories list in the inventory (between `skip-needs-fixture` and `done` alphabetically OR at the end of skip-* labels):

```markdown
- **skip-binary-internal**: item is `pub` for the binary's integration-test reachability, but not a stable library API for embedders.
```

- [ ] **Step 2.4: Extend the inventory**

Open `docs/superpowers/inventories/2026-05-27-bare-items-round-4.md` (created by PR-A, now on main). Add `## tau-app` section AFTER `## tau-workflow` and BEFORE `## Status log`. One row per item from `/tmp/pub-items-app.txt`.

Append to Status log:

```markdown
- 2026-05-27 — tau-app classifications + <N> includes (PR-B).
```

If you introduced `skip-binary-internal`, also note "introduced skip-binary-internal category" in the status log line.

- [ ] **Step 2.5: Write fences for `include` rows**

Same style as Task 1.6. For tau-app specifically:
- Items that interact with `std::process::exit`, `std::env::args`, or real argv → `no_run` with one-line justification.
- Items with reasonable fixture stories (config loaders with `tempdir`, exit-code mappers, path helpers) → executed.

- [ ] **Step 2.6: Verify dev-deps**

```bash
cat crates/tau-app/Cargo.toml | grep -A 30 '\[dev-dependencies\]'
```

Note what's available. tau-app likely already has `tau-runtime`, `tau-pkg`, `clap` accessible. Add missing fixture-feature deps to `[dev-dependencies]` if needed.

- [ ] **Step 2.7: Run doctests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-app
```

- [ ] **Step 2.8: Commit + push + PR-B**

Same shape as 1.9 with `tau-app` substitutions. Commit subject: `test(app): doctest fences for bare public items (round 4)`.

- [ ] **Step 2.9: WAIT for PR-B to merge before Task 3.**

---

## Task 3: PR-C — tau-observe (~25 examples)

**Branch:** `feat/doctests-round-4-observe`, branched from `origin/main` AFTER PR-B merges. Largest PR; closes round 4.

- [ ] **Step 3.1: Create the worktree (after PR-B merges)**

```bash
cd $(git worktree list | grep -v '(bare)' | head -1 | awk '{print $1}')
git fetch origin main --quiet
git worktree add /Users/titouanlebocq/code/tau-worktrees/doctests-round-4-observe -b feat/doctests-round-4-observe origin/main
cd /Users/titouanlebocq/code/tau-worktrees/doctests-round-4-observe
```

- [ ] **Step 3.2: Enumerate**

```bash
git grep -nE '^\s*pub (fn |async fn |struct |enum |trait |type |const |mod )' -- 'crates/tau-observe/src/' | grep -v 'pub(crate)' > /tmp/pub-items-observe.txt
wc -l /tmp/pub-items-observe.txt
```

Expected: ~59+ lines.

- [ ] **Step 3.3: Classify per spec §3**

**For tau-observe specifically**, expected patterns:

| Item kind | Default classification |
|---|---|
| `SPAN_*` / `EVENT_*` `pub const &str` vocabulary constants | `skip-trivial` (constants are documented by their docstring; no example needed) |
| `pub fn redact_secret(...)` / `truncate_for_logging(...)` / preview helpers | `include` per §3.1 (free fns, behavior-verifying assertion) |
| `pub fn install_layers(...)` / setup functions | `include` if returns Result; `no_run` if installs a global subscriber that conflicts with test harness |
| `Captor` test subscriber (gated behind `test-fixtures` feature) | `skip-feature-gated` OR `include` if reachable from default doctest harness |
| `WorkflowRunLogLayer`, `PluginRecordingLayer` (Logging Sub-project D, PR #226) | `include` per §3.1 — uses `tracing_subscriber::with_default(Captor::default(), || { … })` |
| Layer-config builders | `include` per §3.1 constructor rule |

- [ ] **Step 3.4: Extend the inventory**

Add `## tau-observe` section AFTER `## tau-app` and BEFORE `## Status log`. One row per item from `/tmp/pub-items-observe.txt`.

Append to Status log:

```markdown
- 2026-05-27 — tau-observe classifications + <N> includes (PR-C). Closes round 4.
```

- [ ] **Step 3.5: Write fences for `include` rows**

For preview helpers (`redact_secret`, etc.):

```rust
/// ```
/// use tau_observe::redact_secret;
///
/// let masked = redact_secret("sk-1234567890abcdef");
/// assert!(!masked.contains("1234567890abcdef"));
/// assert!(masked.starts_with("sk-") || masked.starts_with("***"));
/// // exact mask shape: verify by reading the impl + adjust assertion
/// ```
```

For Captor-based behavioral fences:

```rust
/// ```
/// # use tracing::Level;
/// # use tracing_subscriber::layer::SubscriberExt;
/// # use tracing_subscriber::util::SubscriberInitExt;
/// // (Captor-based example demonstrating that the layer emits the expected event)
/// ```
```

(Verify the actual `Captor` API at plan-time. If it's feature-gated and the doctest harness can't reach it without `--features test-fixtures`, fall back to a simpler example or classify the item `skip-feature-gated`.)

- [ ] **Step 3.6: Verify dev-deps**

```bash
cat crates/tau-observe/Cargo.toml | grep -A 30 '\[dev-dependencies\]'
```

`tracing-subscriber` is likely already a dev-dep. Verify the `test-fixtures` feature setup if Captor examples need it.

- [ ] **Step 3.7: Run doctests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-observe
```

- [ ] **Step 3.8: Commit + push + PR-C (last one!)**

Same shape as 1.9 / 2.8 with `tau-observe` substitutions. Commit subject: `test(observe): doctest fences for bare public items (round 4)`. PR body includes "closes round 4" framing.

- [ ] **Step 3.9: WAIT for PR-C to merge before Task 4.**

---

## Task 4: Final verification

**Files:** none modified — verification only.

- [ ] **Step 4.1: Confirm bare-item gap closure on main**

```bash
cd $(git worktree list | grep -v '(bare)' | head -1 | awk '{print $1}')
git fetch origin main --quiet
for crate in tau-workflow tau-observe tau-app; do
  pubs=$(git grep -hE '^\s*pub (fn |async fn |struct |enum |trait |type |const |mod )' origin/main -- "crates/$crate/src/" 2>/dev/null | grep -v 'pub(crate)' | wc -l | tr -d ' ')
  blocks=$(($(git grep -c '```' origin/main -- "crates/$crate/src/" 2>/dev/null | awk -F: 'BEGIN{s=0} {s+=$NF} END {print s+0}') / 2))
  echo "$crate: pub_items=$pubs, doctest_blocks=$blocks, gap=$((pubs - blocks))"
done
```

Expected: each gap is meaningfully smaller than pre-round-4 (workflow 19→~9, observe 58→~33, app 44→~24). Items classified `skip-*` remain in the gap — that's correct.

- [ ] **Step 4.2: Confirm all 3 crates' doctests pass on main**

Create a fresh worktree off current main (avoids stale-worktree pitfalls):

```bash
git worktree add /Users/titouanlebocq/code/tau-worktrees/round-4-verify origin/main
cd /Users/titouanlebocq/code/tau-worktrees/round-4-verify
timeout 600 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-verify cargo test --doc -p tau-workflow -p tau-observe -p tau-app
```

Expected: 0 failed, total passing count ~55+ (sum of per-PR include counts).

- [ ] **Step 4.3: Confirm clippy clean**

```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-verify cargo clippy -p tau-workflow -p tau-observe -p tau-app --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 4.4: Final inventory check**

```bash
awk '/^\| [0-9]+ \|/' docs/superpowers/inventories/2026-05-27-bare-items-round-4.md | wc -l
grep -E "TBD|\| \? \|" docs/superpowers/inventories/2026-05-27-bare-items-round-4.md | wc -l
```

Expected: row count ≥ sum of per-crate item counts. TBD count = 0.

- [ ] **Step 4.5: Cleanup worktrees + branches**

```bash
cd $(git worktree list | grep -v '(bare)' | head -1 | awk '{print $1}')
for wt in doctests-round-4-spec doctests-round-4-workflow doctests-round-4-app doctests-round-4-observe round-4-verify; do
  git worktree remove --force "/Users/titouanlebocq/code/tau-worktrees/$wt" 2>&1 | head -1
done
for b in feat/doctests-round-4-spec feat/doctests-round-4-workflow feat/doctests-round-4-app feat/doctests-round-4-observe; do
  git push --no-verify origin --delete "$b" 2>&1 | tail -1
  git branch -D "$b" 2>&1 | tail -1
done
```

(Per memory `feedback_remote_branch_delete_no_verify`: branch deletions need `--no-verify`.)

- [ ] **Step 4.6: (Optional) save memory entry**

If the round surfaces new patterns or gotchas, write `~/.claude/projects/-Users-titouanlebocq-code-tau/memory/project_doctests_round_4_2026_05_<DD>.md`. Otherwise skip — the existing rounds-2 and rounds-3 memory entries cover most lessons.

---

## Notes for the executor

- **No stacking.** Branch each PR from current `main`, wait for the previous PR to merge. Avoids the round-2 base-retarget race documented in `memory/project_doctests_round_2_2026_05_26.md`.
- **`tau-cli` chat-test flake.** Per memory, the `chat_clear_*` / `chat_unknown_slash_*` tests in `crates/tau-cli/tests/cmd_chat*.rs` flake on macOS in the merge queue (`echo-llm` plugin load failure). If a queue attempt fails on these tests with this error, wait — the queue auto-retries and usually passes on retry 2-3.
- **Inventory completeness.** ALWAYS use the broader grep that includes impl-block methods (per round-3 lesson — every PR's spec review caught this gap on the first pass). Cross-check with `wc -l` against the inventory's row count for the target section.
- **Pushing:** ALWAYS `git push --no-verify`. NEVER bare `git push`.
- **Cargo invocations:** ALWAYS prefixed with `timeout` + `CARGO_INCREMENTAL=0` + `CARGO_TARGET_DIR=target/agent-impl` + `-p <crate>`.
- **Commits:** `git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "..."` with HEREDOC body. Required to dodge the lefthook identity-corruption gotcha (CLAUDE.md).
- **Reuse fixtures.** The `MockLlmBackend` + `make_completion_response` + `any_*` patterns are battle-tested through rounds 2-3. Don't invent new ones unless a row genuinely needs something different.
- **If a row genuinely needs a new classification label**, add it to the Categories list in the inventory + use it consistently. The `skip-binary-internal` label is the prepared one for tau-app; if you find a different need (e.g., `skip-test-only`), add and document.
- **Fence-enabling production code additions** (e.g., adding `::new()` to a `#[non_exhaustive]` type) are acceptable per round-2/3 precedent — keep them minimal (just `Self { … }`-style wrappers).
