# Doctests Round 3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `///` doctest fences to the load-bearing subset (~88 items) of currently-bare public items across the 5 tier-1 stable-surface crates (`tau-plugin-protocol`, `tau-plugin-sdk`, `tau-domain`, `tau-runtime`, `tau-pkg`), so `cargo doc` renders show usage examples on every API entry point a third-party user would reach for.

**Architecture:** Five PRs, one per crate, in ascending bare-count order. Each PR (a) audits its crate, (b) classifies each bare public item per spec §3 (include / skip-*), (c) writes doctests for the include rows reusing round-2 fixture patterns, (d) ships an inventory file documenting the classifications. **No stacking** — each PR branches off current `main` to avoid the round-2 race that lost PR #220.

**Tech Stack:** Rust 2021, `cargo test --doc`, `tau_ports::fixtures::{MockLlmBackend, make_completion_response, make_token_usage}`, `tau_domain::fixtures::{any_agent_definition, any_package_manifest, any_message}`, `tempfile`, `tokio-test` (where async). No new workspace dependencies.

---

## Spec reference

This plan implements `docs/superpowers/specs/2026-05-26-doctests-round-3-design.md`. Key constraints recapped:

- **Load-bearing inclusion** (spec §3.1): constructors, public trait impls, methods returning non-trivial `Result`, methods with 2+ non-self params or generics, free functions, enum variants with associated data, conversion methods.
- **Skip** (spec §3.2): trivial getters, derived impls, type aliases, trivial setters, marker traits, `Display`/`Debug` impls, re-exports.
- **In doubt → include** (spec §3.3). Skip-reason justified in inventory if non-obvious.
- **Default fence**: bare ` ``` ` (executed). Use `no_run` only when execution would hit a forbidden side effect (spec §4).
- **Forbidden in any example**: network, runtime env-var reads, FS writes outside `tempfile::tempdir()`, real subprocesses, `.unwrap()` on meaningful `Result`s.
- **Stacking discipline**: each PR opens after its predecessor merges. Never branch off another open PR.

---

## Pre-flight (do this once, before Task 1)

- [ ] **Step 0.1: Confirm spec branch is current**

```bash
cd /Users/titouanlebocq/code/tau-worktrees/doctests-round-3-spec
git status -sb
```

Expected: `## feat/doctests-round-3-spec...origin/main` (this branch holds the spec + plan and will land as its own PR before per-crate work begins).

- [ ] **Step 0.2: Verify baseline bare-item counts**

```bash
for crate in tau-plugin-protocol tau-plugin-sdk tau-domain tau-runtime tau-pkg; do
  pubs=$(git grep -hE '^\s*pub (fn |async fn |struct |enum |trait |type |const )' "crates/$crate/src/" 2>/dev/null | grep -v 'pub(crate)' | wc -l | tr -d ' ')
  fences=$(git grep -c '```' -- "crates/$crate/src/" 2>/dev/null | awk -F: 'BEGIN{s=0} {s+=$NF} END {print s}')
  echo "$crate: pub_items=$pubs, doctest_blocks=$((fences/2))"
done
```

Expected (within ±2 of these):

```
tau-plugin-protocol: pub_items=20, doctest_blocks=10
tau-plugin-sdk: pub_items=19, doctest_blocks=5
tau-domain: pub_items=63, doctest_blocks=36
tau-runtime: pub_items=71, doctest_blocks=11
tau-pkg: pub_items=91, doctest_blocks=24
```

If counts drift significantly, another session may have landed more doctests — adjust the bare-item targets in each task accordingly.

- [ ] **Step 0.3: Push spec branch as its own PR (PR-spec)**

```bash
git push --no-verify -u origin feat/doctests-round-3-spec
gh pr create --title "docs(spec+plan): doctests round 3" --body "$(cat <<'EOF'
## Summary
Spec + plan for round 3 of "doctests in /// comments" — add fenced examples to the load-bearing subset of currently-bare public items across the 5 tier-1 crates. Estimated ~88 new doctests across 5 PRs.

- [Spec](../docs/superpowers/specs/2026-05-26-doctests-round-3-design.md)
- [Plan](../docs/superpowers/plans/2026-05-26-doctests-round-3.md)

Per the spec's stacking discipline, this PR lands first; each per-crate implementation PR branches off `main` only after the previous one merges.

## Test plan
- [x] Docs-only — no code changed.
- [ ] CI green.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
gh pr merge --auto
```

PR-spec merges, then Task 1 (PR-A protocol) starts in a fresh worktree off main.

---

## Task 1: PR-A — tau-plugin-protocol (~10 bare items, est. ~5 examples)

**Branch:** `feat/doctests-round-3-protocol`, branched from `origin/main` *after* PR-spec merges.

**Files:**
- Modify: `crates/tau-plugin-protocol/src/*.rs` (~5 files — one fenced example per include row).
- Create: `docs/superpowers/inventories/2026-05-26-bare-items-round-3.md` (inventory skeleton; future PRs extend it).

- [ ] **Step 1.1: Create the worktree (after PR-spec merges)**

```bash
cd $(git worktree list | grep -v '(bare)' | head -1 | awk '{print $1}')
git fetch origin main --quiet
git worktree add /Users/titouanlebocq/code/tau-worktrees/doctests-round-3-protocol -b feat/doctests-round-3-protocol origin/main
cd /Users/titouanlebocq/code/tau-worktrees/doctests-round-3-protocol
```

- [ ] **Step 1.2: Enumerate the crate's public items**

```bash
git grep -nE '^\s*pub (fn |async fn |struct |enum |trait |type |const )' -- 'crates/tau-plugin-protocol/src/' | grep -v 'pub(crate)' > /tmp/pub-items-protocol.txt
wc -l /tmp/pub-items-protocol.txt
```

Expected: ~20 lines (the spec §1 count).

- [ ] **Step 1.3: Cross-reference against existing doctest blocks**

For each item in `/tmp/pub-items-protocol.txt`, open the file at the listed line and read backward ~25 lines to see if a ` ``` ` or ` ```no_run ` fence already exists in its preceding `///` block. If yes: classify `done`. If no: continue to step 1.4.

- [ ] **Step 1.4: Classify each remaining bare item per spec §3**

For every item without an existing fence, decide:

- **include**: it's in spec §3.1 categories (constructor / public trait impl / non-trivial Result / 2+ params or generics / free fn / enum variant w/ data / non-trivial conversion).
- **skip-trivial / skip-getter / skip-setter / skip-derived / skip-alias / skip-display / skip-marker / skip-reexport**: matches a spec §3.2 category. The classification label tells the reviewer the reason.
- **In doubt**: prefer `include` (spec §3.3).

**For tau-plugin-protocol specifically**, expected classifications (verify each):

| Item kind | Default classification |
|---|---|
| `pub const FRAME_TOO_LARGE_ERROR: i32` and similar | `skip-trivial` (constants don't usually need an example beyond their docstring) |
| `pub fn new(...)` constructors | `include` |
| `Frame` enum variants | `include` (each variant doc) — likely most are already covered by round 2 |
| `pub struct HandshakeRequest`/`HandshakeResponse` field-level docs | `skip-trivial` if just data; `include` if the struct has a non-default constructor or builder |

Don't fight the classification — record what you choose and move on. The inventory captures the reasoning.

- [ ] **Step 1.5: Create the inventory file**

Write `docs/superpowers/inventories/2026-05-26-bare-items-round-3.md` with this exact opening (a per-crate section will be appended to it; future PRs extend the same file):

```markdown
# Bare-item coverage inventory — round 3

**Source:** post-round-2 bare-item audit across the 5 tier-1 crates on 2026-05-26.
**Spec:** `docs/superpowers/specs/2026-05-26-doctests-round-3-design.md`.
**Plan:** `docs/superpowers/plans/2026-05-26-doctests-round-3.md`.

## Categories

- **include**: classification per spec §3.1 — adds a `///` doctest fence in this PR.
- **skip-trivial**: trivial item not requiring an example (covered by `///` prose alone).
- **skip-getter / skip-setter**: trivial accessor / mutator.
- **skip-derived**: derived trait impl (`#[derive(...)]`).
- **skip-alias**: `pub type X = Y`.
- **skip-display / skip-debug**: `Display` / `Debug` impl.
- **skip-marker**: marker trait or unit-struct sentinel.
- **skip-reexport**: `pub use`.
- **done**: already had a fence before round 3 began (no change needed).

## tau-plugin-protocol

| # | File:line | Item | Classification | Strategy |
|---|---|---|---|---|

(Rows populated by PR-A.)

## Status log

(Per-PR updates appended below as each crate ships.)
```

Then populate the `## tau-plugin-protocol` section: one row per public item in the crate (every item from `/tmp/pub-items-protocol.txt`, both `include` and `skip-*` and `done`). For `include` rows, add a 1-line strategy hint (e.g., "constructor → straightforward execute"). For non-obvious `skip-*` (anything outside the §3.2 default categories), add a one-line justification.

- [ ] **Step 1.6: Write doctest fences for every `include` row**

For each `include` row, open the file at the listed line and add a `///` doctest block. Style rules (spec §4):

- Default fence: bare ` ``` ` (executed).
- Visible body: 3–8 lines focused on the API. Use `assert_eq!` / `assert!` to make the test verify behavior.
- Hidden setup with `# `-prefixed lines is encouraged. Reuse round-2 patterns:
  - `# use tau_plugin_protocol::*;` for cross-module imports.
  - Construct via `::new()` / `from_str()` / etc. for `#[non_exhaustive]` types.
- For pattern-matching shape examples (e.g., a function that handles multiple `Frame` variants), use `_ =>` catch-all on `#[non_exhaustive]` enums.

**No example is allowed to**: hit the network, read env vars at runtime, write outside `tempdir()`, spawn real subprocesses, or `.unwrap()` on a meaningful `Result`. If activation would require any of these, the item gets `no_run` + a one-line justification (`/// (no_run because …)`).

- [ ] **Step 1.7: Run the doctests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-plugin-protocol
```

Expected: pass. The number of passing doctests increases by the count of `include` rows. If any fail, debug — typically caused by missing imports in hidden setup, or a struct-literal construction that should be `::new()` (spec §4 / round-2 lessons).

- [ ] **Step 1.8: Commit + push + open PR**

```bash
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" add crates/tau-plugin-protocol/ docs/superpowers/inventories/2026-05-26-bare-items-round-3.md
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "$(cat <<'EOF'
test(plugin-protocol): doctest fences for bare public items (round 3)

Round 3 of "doctests in /// comments" — load-bearing bare-item
coverage. Adds fenced examples to <N> previously-fenceless public
items in tau-plugin-protocol, per the inventory at
docs/superpowers/inventories/2026-05-26-bare-items-round-3.md.

Refs: docs/superpowers/specs/2026-05-26-doctests-round-3-design.md.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
git push --no-verify -u origin feat/doctests-round-3-protocol
gh pr create --title "test(plugin-protocol): doctest fences for bare public items (round 3)" --body "Round 3 — see [spec](../docs/superpowers/specs/2026-05-26-doctests-round-3-design.md) and [inventory](../docs/superpowers/inventories/2026-05-26-bare-items-round-3.md). Adds <N> new doctests to load-bearing bare items.

## Test plan
- [x] cargo test --doc -p tau-plugin-protocol green locally.
- [ ] CI green.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
gh pr merge --auto
```

(Replace `<N>` with the actual include-row count in both the commit message body and PR body.)

- [ ] **Step 1.9: WAIT for PR-A to merge before starting Task 2**

Critical per spec §5 stacking discipline. Do not branch Task 2 off an open PR.

---

## Task 2: PR-B — tau-plugin-sdk (~14 bare items, est. ~7 examples)

**Branch:** `feat/doctests-round-3-sdk`, branched from `origin/main` *after* PR-A merges.

**Procedure mirrors Task 1**, with these differences:

- [ ] **Step 2.1: Create the worktree (after PR-A merges)**

```bash
cd $(git worktree list | grep -v '(bare)' | head -1 | awk '{print $1}')
git fetch origin main --quiet
git worktree add /Users/titouanlebocq/code/tau-worktrees/doctests-round-3-sdk -b feat/doctests-round-3-sdk origin/main
cd /Users/titouanlebocq/code/tau-worktrees/doctests-round-3-sdk
```

- [ ] **Step 2.2: Enumerate + cross-reference**

```bash
git grep -nE '^\s*pub (fn |async fn |struct |enum |trait |type |const )' -- 'crates/tau-plugin-sdk/src/' | grep -v 'pub(crate)' > /tmp/pub-items-sdk.txt
wc -l /tmp/pub-items-sdk.txt
```

For each entry, check for an existing fence in the preceding `///` block (same procedure as 1.3).

- [ ] **Step 2.3: Classify per spec §3**

**For tau-plugin-sdk specifically**, watch for:

| Item kind | Default classification |
|---|---|
| `Configure` trait, `Configure::from_config` | `done` (round 2 activated this) |
| `run_llm_backend` / `run_tool` (non-`_with_config` variants) | `include` — these need a hidden `MyPlugin: LlmBackend` setup (round-2 pattern from `runners/llm_backend.rs:122`, but flipped to executed where possible — `run_*` doesn't take stdin, so might be `no_run` only) |
| `SdkError` variants | `include` (per spec §3.1 enum-variant rule) |
| `ConfigError` variants | `done` (round 1 covered) |

If a `run_*` variant cannot execute without a real stdin/stdout dispatch loop (likely — same constraint as round-2 PR-B), classify as `include` but use `no_run` + a hidden `# struct MyPlugin; impl LlmBackend for MyPlugin { ... }` preamble.

- [ ] **Step 2.4: Extend the inventory**

Add a `## tau-plugin-sdk` section to `docs/superpowers/inventories/2026-05-26-bare-items-round-3.md`, after the `## tau-plugin-protocol` section, with one row per public item.

- [ ] **Step 2.5: Write doctest fences for `include` rows**

Reuse the round-2 hidden-fixture pattern. Where the SDK trait signatures matter (e.g., `LlmBackend::name(&self) -> &str`, `async fn complete(...)`, `async fn stream(...)`; `Tool::name`, `schema`, `init`, `invoke`, `teardown`), verify with:

```bash
git grep -nA 30 'pub trait LlmBackend' -- crates/tau-ports/src/
git grep -nA 30 'pub trait Tool' -- crates/tau-ports/src/
```

and align the hidden block exactly to the current trait shape.

- [ ] **Step 2.6: Run doctests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-plugin-sdk
```

- [ ] **Step 2.7: Commit + push + PR**

Same shape as 1.8 with `-p tau-plugin-sdk` substituted. Commit subject: `test(plugin-sdk): doctest fences for bare public items (round 3)`. Wait for merge before Task 3.

---

## Task 3: PR-C — tau-domain (~27 bare items, est. ~13 examples)

**Branch:** `feat/doctests-round-3-domain`, branched from `origin/main` *after* PR-B merges.

- [ ] **Step 3.1: Create the worktree (after PR-B merges)**

```bash
cd $(git worktree list | grep -v '(bare)' | head -1 | awk '{print $1}')
git fetch origin main --quiet
git worktree add /Users/titouanlebocq/code/tau-worktrees/doctests-round-3-domain -b feat/doctests-round-3-domain origin/main
cd /Users/titouanlebocq/code/tau-worktrees/doctests-round-3-domain
```

- [ ] **Step 3.2: Enumerate + cross-reference**

```bash
git grep -nE '^\s*pub (fn |async fn |struct |enum |trait |type |const )' -- 'crates/tau-domain/src/' | grep -v 'pub(crate)' > /tmp/pub-items-domain.txt
wc -l /tmp/pub-items-domain.txt
```

- [ ] **Step 3.3: Classify per spec §3**

**For tau-domain specifically**, watch for:

| Item kind | Default classification |
|---|---|
| `id` newtypes (`AgentId`, `RunId`, `SessionId`, etc.) and their `::new()` constructors | `include` (constructors are spec §3.1) |
| `pub fn user(impl Into<String>) -> Message` and similar `Message` constructors | `done` (round 2 covered `Message`); other constructors `include` |
| `PackageManifest::*` parsing methods | `include` if non-trivial Result (e.g., `parse_package_manifest`) |
| Field-level `pub` (data fields on public structs) | `skip-trivial` (the struct-level doc covers them) |
| Trait impls on the public types (e.g., `impl FromStr for X`) | `include` per spec §3.1 (it's a public trait impl) |

`tau-domain` types include several `#[non_exhaustive]` with no public constructor (`UncheckedManifest`, etc. — see round 2 inventory). For those, if the item is a method on such a type, **`no_run`** with a justification line (per round-2 PR-D's row 19 pattern).

- [ ] **Step 3.4: Extend the inventory**

Add a `## tau-domain` section to `docs/superpowers/inventories/2026-05-26-bare-items-round-3.md`.

- [ ] **Step 3.5: Write doctest fences for `include` rows**

Reuse fixture helpers where helpful:

```rust
/// ```
/// # use tau_domain::fixtures::any_agent_definition;
/// # let ad = any_agent_definition();
/// // demonstrate the method using `ad`...
/// ```
```

Reachable because `tau-domain` already exports its own `fixtures` module under the `test-fixtures` feature, and the doctest harness compiles with `dev-dependencies` (so `tau-domain = { workspace = true, features = ["test-fixtures"] }` may need to be added to its own dev-deps if not already present — verify in `crates/tau-domain/Cargo.toml`).

- [ ] **Step 3.6: Run doctests + commit + PR**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-domain
```

Same commit + PR shape as Task 1.8. Commit subject: `test(domain): doctest fences for bare public items (round 3)`. Wait for merge.

---

## Task 4: PR-D — tau-runtime (~60 bare items, est. ~30 examples)

**Branch:** `feat/doctests-round-3-runtime`, branched from `origin/main` *after* PR-C merges.

This is the second-largest PR. Plan for ~30 new doctests using the round-2 fixture pattern.

- [ ] **Step 4.1: Create the worktree (after PR-C merges)**

```bash
cd $(git worktree list | grep -v '(bare)' | head -1 | awk '{print $1}')
git fetch origin main --quiet
git worktree add /Users/titouanlebocq/code/tau-worktrees/doctests-round-3-runtime -b feat/doctests-round-3-runtime origin/main
cd /Users/titouanlebocq/code/tau-worktrees/doctests-round-3-runtime
```

- [ ] **Step 4.2: Enumerate + cross-reference**

```bash
git grep -nE '^\s*pub (fn |async fn |struct |enum |trait |type |const )' -- 'crates/tau-runtime/src/' | grep -v 'pub(crate)' > /tmp/pub-items-runtime.txt
wc -l /tmp/pub-items-runtime.txt
```

- [ ] **Step 4.3: Classify per spec §3**

**For tau-runtime specifically**, watch for:

| Item kind | Default classification |
|---|---|
| `Runtime::*` methods (e.g., `run`, `run_default`, `run_with_history`, `invoke_tool`, `spawn_root_agent`) | `include` — use the round-2 streaming-doctest fixture pattern (`tau_ports::fixtures::MockLlmBackend` + `tau_domain::fixtures::*`) |
| `RuntimeBuilder::with_*` methods | `include` (constructors-of-builders are §3.1) |
| `RunOptions::*` field-level methods | `skip-trivial` if mere setters; `include` if combinators that mutate multi-field state |
| `RunOutcome` variants | `include` per §3.1 enum-variant rule (but check if round 2's PR-C activated some) |
| `orchestration::*` traits/methods | `include` (multi-agent surface) — likely needs a `MockLlmBackend` queue with multiple turns |

The round-2 pattern that worked:

```rust
/// ```
/// # tokio_test::block_on(async {
/// # use tau_runtime::{Runtime, RunOptions};
/// # use tau_ports::fixtures::{MockLlmBackend, make_completion_response, make_token_usage};
/// # use tau_domain::fixtures::{any_agent_definition, any_package_manifest, any_message};
/// # use tau_domain::{AgentDefinition, PackageManifest, Message, StopReason};
/// # use futures_util::{pin_mut, StreamExt};
/// # let resp = make_completion_response("hi".into(), vec![], StopReason::EndTurn, Some(make_token_usage(1, 1)));
/// # let llm = MockLlmBackend::new("test-pkg").with_response(resp);
/// # let runtime = Runtime::builder().with_llm_backend(llm).build().unwrap();
/// # let agent_def = any_agent_definition();
/// # let manifest = any_package_manifest();
/// # let msg = any_message();
/// # let opts: RunOptions = Default::default();
/// // your visible body here, using `runtime`, `agent_def`, `manifest`, `msg`, `opts`
/// # });
/// ```
```

- [ ] **Step 4.4: Extend the inventory**

Add `## tau-runtime` section.

- [ ] **Step 4.5: Write fences for `include` rows**

Important: the visible body of each example must remain focused on the specific method being documented (4-8 lines). The hidden block can be longer; that's fine.

- [ ] **Step 4.6: Run doctests + commit + PR**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-runtime
```

Commit subject: `test(runtime): doctest fences for bare public items (round 3)`. Wait for merge.

---

## Task 5: PR-E — tau-pkg (~67 bare items, est. ~33 examples)

**Branch:** `feat/doctests-round-3-pkg`, branched from `origin/main` *after* PR-D merges.

Largest PR. Closes round 3.

- [ ] **Step 5.1: Create the worktree (after PR-D merges)**

```bash
cd $(git worktree list | grep -v '(bare)' | head -1 | awk '{print $1}')
git fetch origin main --quiet
git worktree add /Users/titouanlebocq/code/tau-worktrees/doctests-round-3-pkg -b feat/doctests-round-3-pkg origin/main
cd /Users/titouanlebocq/code/tau-worktrees/doctests-round-3-pkg
```

- [ ] **Step 5.2: Enumerate + cross-reference**

```bash
git grep -nE '^\s*pub (fn |async fn |struct |enum |trait |type |const )' -- 'crates/tau-pkg/src/' | grep -v 'pub(crate)' > /tmp/pub-items-pkg.txt
wc -l /tmp/pub-items-pkg.txt
```

- [ ] **Step 5.3: Classify per spec §3**

**For tau-pkg specifically**, watch for:

| Item kind | Default classification |
|---|---|
| `install` / `uninstall` / `update` top-level fns | `include` but **`no_run`** (round 2 PR-E classified these as D — they shell out to git or modify on-disk state) |
| `Scope::*` methods (`new_project`, `new_global`, `resolve`, `state_path`, `list`, `get`, etc.) | `include` — use `tempfile::tempdir()` + `Scope::new_project(tmp.path())` per round-2 PR-E pattern |
| `Scope::global` | `include` with `set_var("TAU_HOME", ...)` + `remove_var` per Windows TAU_HOME memory pattern |
| `LockFile::*` methods (load, save, find, upsert, remove) | `include`. Some need `tempfile::tempdir()`; some are pure data |
| `tree_hash` and related fns | `include`. Use tempdir |
| `parse_package_manifest` and similar parsers | `include` (round 1 activated several — verify which remain bare) |
| `BuildOptions`, `ScopeConfig`, similar config builders | round 1 / round 2 covered most — likely `done` |
| Error type variants (`InstallError`, `UpdateError`, etc.) | `include` per §3.1 enum-variant rule, with pattern-shape demos (round-2 PR-E rows 39/40 pattern: define a `fn describe(e: &E) { match e { Variant => ..., _ => ... } }`) |

- [ ] **Step 5.4: Extend the inventory**

Add `## tau-pkg` section. After this PR, all 5 crate sections are populated.

- [ ] **Step 5.5: Write fences for `include` rows**

For `tempdir`-based examples (the bulk):

```rust
/// ```
/// # let tmp = tempfile::tempdir()?;
/// # let scope = tau_pkg::Scope::new_project(tmp.path());
/// // your visible body using `scope`...
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
```

For env-var examples (only `Scope::global`):

```rust
/// ```
/// # let tmp = tempfile::tempdir().unwrap();
/// # std::env::set_var("TAU_HOME", tmp.path());
/// let scope = tau_pkg::Scope::global().unwrap();
/// assert_eq!(scope.kind(), tau_pkg::ScopeKind::Global);
/// # std::env::remove_var("TAU_HOME");
/// ```
```

- [ ] **Step 5.6: Run doctests + commit + PR (last one!)**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-pkg
```

Commit subject: `test(pkg): doctest fences for bare public items (round 3)`. PR body should include "closes round 3" framing.

---

## Task 6: Final verification

**Files:** none modified — verification only. Optional final commit if the inventory or ROADMAP needs a closing note.

- [ ] **Step 6.1: Confirm bare-item gap closure**

```bash
cd $(git worktree list | grep -v '(bare)' | head -1 | awk '{print $1}')
git fetch origin main --quiet
for crate in tau-plugin-protocol tau-plugin-sdk tau-domain tau-runtime tau-pkg; do
  pubs=$(git grep -hE '^\s*pub (fn |async fn |struct |enum |trait |type |const )' origin/main -- "crates/$crate/src/" 2>/dev/null | grep -v 'pub(crate)' | wc -l | tr -d ' ')
  blocks=$(($(git grep -c '```' origin/main -- "crates/$crate/src/" 2>/dev/null | awk -F: 'BEGIN{s=0} {s+=$NF} END {print s}') / 2))
  echo "$crate: pub_items=$pubs, doctest_blocks=$blocks, gap=$((pubs - blocks))"
done
```

Expected: each crate's `gap` is now smaller by roughly the per-task estimate (5, 7, 13, 30, 33). Items in the `skip-*` classes remain in the gap — that's correct.

- [ ] **Step 6.2: Confirm all 5 crates' doctests pass on main**

```bash
timeout 600 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-final cargo test --doc -p tau-plugin-protocol -p tau-plugin-sdk -p tau-domain -p tau-runtime -p tau-pkg
```

Expected: 0 failed.

- [ ] **Step 6.3: Confirm clippy still clean**

```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-final cargo clippy -p tau-plugin-protocol -p tau-plugin-sdk -p tau-domain -p tau-runtime -p tau-pkg --all-targets -- -D warnings
```

Expected: pass.

- [ ] **Step 6.4: Final inventory check**

Read `docs/superpowers/inventories/2026-05-26-bare-items-round-3.md` on `origin/main`. Confirm:

- Every public item from every crate is listed in its crate's section.
- No row has `?` or `TBD` in `Classification` or `Strategy`.
- Status log has 5 entries (PR-A through PR-E).

- [ ] **Step 6.5: (Optional) update memory**

If round 3 surfaces new patterns or gotchas not already documented, add a memory entry at `~/.claude/projects/-Users-titouanlebocq-code-tau/memory/project_doctests_round_3_2026_05_<DD>.md` linking back to the round-2 entry. Skip if nothing surprising emerged.

---

## Notes for the executor

- **Each PR is fully independent.** Branch from current `main`, wait for the previous PR to merge before opening the next.
- **No stacking, no base-retargeting.** This avoids the round-2 race that lost PR #220's content to a non-main branch. See `memory/project_doctests_round_2_2026_05_26.md` for the details.
- **Pushing:** ALWAYS use `git push --no-verify`. NEVER bare `git push` from agent runtime — see CLAUDE.md "AGENT PUSH RULES" + memory `feedback_remote_branch_delete_no_verify`.
- **Cargo invocations:** ALWAYS prefixed with `timeout` + `CARGO_INCREMENTAL=0` + `CARGO_TARGET_DIR=target/agent-impl` (or `target/agent-final` for verification) + `-p <crate>`. See CLAUDE.md "CARGO RULES".
- **Commit identity:** every commit uses `git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "..."` with HEREDOC body, per the lefthook-corrupts-identity gotcha in CLAUDE.md.
- **Reuse round-2 fixtures.** The `MockLlmBackend` + `make_completion_response` + `any_agent_definition` / `any_package_manifest` / `any_message` patterns are battle-tested. Don't invent new ones unless a row genuinely needs something different.
- **Inventory grows across PRs.** PR-A creates the file; PR-B appends `## tau-plugin-sdk`; PR-C appends `## tau-domain`; etc. Each PR also appends a status-log line. This is intentional — single source of truth for round-3 classifications.
- **If a row genuinely can't be classified per §3,** add a new classification label (e.g., `skip-internal-helper`) and document it in the inventory's "Categories" section. Don't force a fit.
- **If you find an `ignore` fence in any of the 5 crates,** that's a round-2 bug. Log it (in the PR body or commit message) but do NOT silently re-activate under round-3 rules — the contracts are different.
