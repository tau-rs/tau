# Doctests round 2 — activate ignored examples across stable surfaces

**Status:** draft, ready for review
**Date:** 2026-05-25
**Series:** "doctests in /// comments" — round 1 shipped via PRs #80, #165, #186, #187, #188, #189, #199, #202, #203 (2026-05-14 … 2026-05-19). This spec is round 2.

## 1. Context (what already shipped)

Round 1 added executed (` ``` `) doctests to the easy wins: pure-data types, pattern-matching shapes, and items that needed no fixtures. Counts on `origin/main` today:

| Crate | Round-1 PRs | Pub-ish items | `///` lines | Executed fences | `ignore` fences | `no_run` fences |
|---|---|---|---|---|---|---|
| tau-runtime | #187 | 155 | 1,785 | 20 | **3** | 0 |
| tau-domain | #188, #203 | 132 | 992 | 70 | **12** | 2 |
| tau-plugin-sdk | #189 | 26 | 238 | 10 | **3** | 0 |
| tau-plugin-protocol | #199 | 48 | 273 | 18 | **3** | 0 |
| tau-pkg | #186, #202 | 152 | 1,599 | 48 | **19** | 0 |
| tau-ports | #80, #165 | 135 | 1,041 | 38 | 0 | 0 |

PR #187's body states the explicit deferral:

> The other 7 ignored doctests in `tau-runtime/src/` either depend on external setup (Runtime construction with backends + plugins, full agent flows) or were placeholders with `/* ... */` — none of those are activatable without significant additional test machinery.

Round 2 picks that up.

## 2. Goal

Activate the **40 currently-`ignore`d doctests** in `tau-runtime`, `tau-domain`, `tau-plugin-sdk`, `tau-plugin-protocol`, and `tau-pkg`. Where activation needs a fixture (MockLlmBackend, MockSandbox, MockStorage), introduce the minimal in-crate shim or dev-dep wiring once, then reuse it across all affected items in that crate.

## 3. Scope

**In scope:**

- The 40 `ignore` fences enumerated by `git grep '```ignore' -- crates/{runtime,domain,plugin-sdk,plugin-protocol,pkg}/src/` on `origin/main`.
- Adding a small dev-dep fixture wiring on `tau-runtime` (MockLlmBackend from `tau-plugin-test-support`, MockSandbox from `tau-ports::test_fixtures`) if and only if needed to activate ≥2 ignored items.
- Converting `ignore` → executed (` ``` `) by default; only converting to `no_run` if the example would be expensive/hermetic-unfriendly at test time (file I/O outside `tempdir`, real network — which §6 forbids anyway, so this should rarely happen).
- Deleting placeholder `/* ... */`-only examples that PR #187 mentioned and replacing them with a real fence, or removing them entirely if the surrounding `///` doc reads fine without an example.

**Out of scope:**

- Adding doctests to public items that currently have **no** fenced example (that's round 3 — a much larger surface, ~600 items workspace-wide).
- Other crates (`tau-cli`, `tau-app`, `tau-infra`, `tau-workflow`, `tau-observe`, sandbox crates, plugin crates). Round 4+.
- Style/policy changes: `no_run`-as-default, `examples/*.rs` directories, custom rustdoc lints beyond what already ships on main.
- Rewriting `//!` module-level prose.

## 4. Inventory + categorization (phase 1)

Phase 1 of the implementation plan produces a checked-in inventory at `docs/superpowers/inventories/2026-05-25-ignored-doctests.md` with one row per ignored item:

| crate | file:line | item | category | activation strategy |
|---|---|---|---|---|

Categories (per-item):

- **A — pure activation**: the example body is correct; flipping `ignore` → ` ``` ` works as-is. Expected for ≥40% of items based on the round-1 pattern.
- **B — needs hidden setup**: the example needs a hidden `# let runtime = …;` preamble using fixtures from `tau-plugin-test-support` / `tau-ports::test_fixtures`. Expected for the Runtime-flow items PR #187 deferred.
- **C — placeholder**: example body is `/* ... */` or similar. Either write a real example (preferred) or delete the fence and rely on the prose `///` doc.
- **D — genuinely-can't-execute**: e.g. requires a real OS sandbox, real network, real subprocess. Convert `ignore` → `no_run` so the example still typechecks. Document why in a `///` comment above the fence.

A row in category D requires a one-line justification; we expect very few of these.

## 5. Fixture strategy

`tau-runtime` already has `tau-plugin-test-support` and `tau-ports/test-fixtures` reachable (`tau-ports` is a regular dep with `test-fixtures` feature enabled in `Cargo.toml` per CLAUDE.md TODO note; `tau-plugin-test-support` needs adding as dev-dep — verify at plan time).

Per-crate dev-dep additions expected:

- `tau-runtime`: `tau-plugin-test-support` (dev), `tokio-test` (dev) if not already present.
- `tau-domain`: likely none — domain items are data types.
- `tau-plugin-sdk`: dev-dep on the workspace's own mock crate if SDK ignored items document trait-impl flows.
- `tau-plugin-protocol`: likely none.
- `tau-pkg`: `tempfile` (dev) if any ignored items document filesystem flows (likely — lockfile/install code). Probably already present.

These are confirmed during phase 1 inventory, not assumed now.

## 6. Forbidden in any activated example

- Reading env vars (unless under `# std::env::set_var(…)` hidden setup inside the example's own scope).
- Network access (real DNS / sockets).
- Filesystem writes outside `tempfile::tempdir()`.
- Spawning real subprocesses (sandbox/plugin children).
- `.unwrap()` on a meaningful `Result`; use `?` with the `# async fn demo() -> Result<…>` wrapper.

If activation would require any of these, the item goes in category D (`no_run` conversion + justification comment).

## 7. PR cadence

Match the round-1 cadence: one PR per crate. Five PRs total, in this order (smallest first so the fixture pattern lands somewhere visible early):

1. **tau-plugin-protocol** (3 ignored) — likely all category A; smoke-test the workflow.
2. **tau-plugin-sdk** (3 ignored).
3. **tau-runtime** (3 ignored) — establishes the MockLlmBackend fixture pattern.
4. **tau-domain** (12 ignored).
5. **tau-pkg** (19 ignored) — largest, lands last.

Each PR runs `cargo test --doc -p <crate>` locally before push, and CI runs the same. No cross-crate batching — keeps blast radius small and matches the proven round-1 pattern.

## 8. CI impact

- `cargo test --doc` already gates every PR (ROADMAP §12-E). 40 new executed doctests add minutes-not-hours; verify with a `hyperfine` before/after on the largest PR (tau-pkg).
- No new CI job.
- No new lint changes — `tau-runtime` and `tau-domain` already ship with `#![deny(missing_docs)]` + `#![deny(rustdoc::broken_intra_doc_links)]` on main.

## 9. Drift discipline

Existing — each activated doctest catches signature drift at compile time. Category D (`no_run`) still typechecks. No new policy needed.

## 10. Out-of-scope follow-ons (named for future specs)

- **Round 3 — bare-item coverage:** add doctests to public items that have no fenced example today. ~600-item surface across the six tier-1 crates. Needs its own scope decomposition.
- **Round 4 — tier-2 crates:** `tau-workflow`, `tau-observe`, `tau-infra`, `tau-app`.
- **Round 5 — tier-3 / opt-out:** `tau-cli`, `tau-sandbox-*`, plugin crates.
- **Policy spec:** decide whether to make `no_run`-as-default the convention going forward, or stay with the current "executed or ignored, never no_run" pattern. Right now mainline has 0 `no_run` doctests outside tau-domain's 2 — there's no precedent either way.

## 11. Success criteria

- `cargo test --doc -p tau-runtime -p tau-domain -p tau-plugin-sdk -p tau-plugin-protocol -p tau-pkg` runs zero `ignore`d items (modulo category-D conversions documented in the inventory).
- `git grep '```ignore' crates/{tau-runtime,tau-domain,tau-plugin-sdk,tau-plugin-protocol,tau-pkg}/src/` returns 0 matches after all 5 PRs merge. (Category-D items convert to `no_run`, not `ignore`, so this grep is the canonical "no leftover round-1 deferrals" check.)
- The inventory file at `docs/superpowers/inventories/2026-05-25-ignored-doctests.md` is committed and every row has a final status (activated / no_run / deleted).
- All 5 PRs merge green; no new flakes attributable to the new doctests in the following week's CI runs.

## 12. Open questions

(Resolved during writing-plans, not blocking spec approval.)

- Whether `tau-plugin-test-support` exports a `MockLlmBackend` (PR #83 says yes; confirm exact import path at plan time).
- Whether any of the 19 `tau-pkg` ignored items are duplicates of patterns already activated by PR #202 (helpers). Phase 1 inventory deduplicates.
- Whether `tau-runtime` already has `tau-plugin-test-support` as a dev-dep (check Cargo.toml at plan time).
