# Doctests round 3 — load-bearing bare-item coverage

**Status:** approved, ready for plan
**Date:** 2026-05-26
**Series:** Third instalment of "doctests in `///` comments." Round 1 (PRs #80, #165, #186–#189, #199, #202–#203) added executed fences to easy wins. Round 2 (PRs #215, #218, #219, #223) activated the 40 `\`\`\`ignore` fences across the 5 tier-1 crates. Round 3 targets items that have **no fenced example at all** today.

## 1. Context (what's left)

Post-round-2 audit on `origin/main`:

| Crate | `pub fn`/`struct`/`enum`/`trait`/`type`/`const` items | doctest blocks | bare-item gap |
|---|---|---|---|
| tau-plugin-protocol | 20 | ~10 | ~10 |
| tau-plugin-sdk | 19 | ~5 | ~14 |
| tau-runtime | 71 | ~11 | ~60 |
| tau-domain | 63 | ~36 | ~27 |
| tau-pkg | 91 | ~24 | ~67 |
| **total** | **264** | **~86** | **~178** |

The audit grep excludes `pub mod` and `pub use` (not doctest candidates) and `pub(crate)` items. An earlier rough estimate of "~600 bare items" inflated the count by including those non-API items.

## 2. Goal

Add `///` doctest fences to the **load-bearing subset** of those ~178 bare items (estimated ~88 new doctests). After round 3 lands, every API entry point a third-party user would reach for in `cargo doc` has a usage example next to its signature.

## 3. Load-bearing rules

Concrete inclusion/exclusion criteria, applied per item during PR authoring.

### 3.1 Include (earn a fence)

- **Constructors**: `::new`, `::from_*`, `::with_*`, `::builder`.
- **Public trait `impl` blocks** (the impl itself, not derived impls).
- **Methods returning a non-trivial `Result<T, E>`** where the error path is meaningful.
- **Methods with 2+ non-self parameters** or with generic bounds.
- **Public functions** (non-method `pub fn`).
- **Enum variants with associated data** (the variant-level doc; the enum-level doc usually already has one).
- **Conversion methods** (`as_*`, `into_*`, `to_*`) where the conversion is non-trivial.

### 3.2 Skip (one-line `///` doc is sufficient)

- Trivial getters: `fn x(&self) -> &T`, `fn is_y(&self) -> bool`.
- `Default::default()` (covered by the `Default` trait docs).
- Derived trait impls (`#[derive(...)]`).
- Type aliases (`pub type X = Y`).
- Trivial setters: `fn set_x(&mut self, x: X)`.
- Marker traits and unit-struct sentinels.
- `Display` / `Debug` impls.
- Re-exports.

### 3.3 In doubt → include

When the categorization is unclear, prefer to add an example. The inventory (§6) records the reason for any skip beyond the §3.2 rules so reviewers can sanity-check.

## 4. Style conventions

Inherits round 2's conventions (`docs/superpowers/specs/2026-05-25-doctests-round-2-design.md` §4–§6):

- **Default fence**: bare ` ``` ` (executed).
- **`no_run`** only when execution would require a forbidden side effect. One-line justification comment above the fence.
- **Forbidden in any example**: network access, runtime env-var reads (`std::env::var(...)`), filesystem writes outside `tempfile::tempdir()`, real subprocess spawns, `.unwrap()` on meaningful `Result`s. (`std::env::set_var("TAU_HOME", tempdir)` paired with `remove_var` cleanup remains acceptable per the Windows TAU_HOME test pattern.)
- **Hidden setup** via `# ` lines encouraged to keep the visible portion focused on the API. Reuse `tau_ports::fixtures::{MockLlmBackend, MockSandbox, make_completion_response, make_token_usage}` and `tau_domain::fixtures::{any_agent_definition, any_package_manifest, any_message}` — the patterns established in round 2.
- Existing `#[deny(missing_docs)]` and `#[deny(rustdoc::broken_intra_doc_links)]` on the 5 crates stays. No new lint changes.

## 5. PR cadence + stacking strategy

**Five PRs, one per crate, in ascending bare-count order:**

1. **tau-plugin-protocol** — ~10 bare items, est. ~5 examples.
2. **tau-plugin-sdk** — ~14 bare items, est. ~7 examples.
3. **tau-domain** — ~27 bare items, est. ~13 examples.
4. **tau-runtime** — ~60 bare items, est. ~30 examples.
5. **tau-pkg** — ~67 bare items, est. ~33 examples.

Total estimate: **~88 new doctests** added across the 5 PRs.

**No stacking.** Each PR targets `main` directly and branches from current `main` at PR-open time. The next PR opens only after the previous one merges. This avoids the round-2 race documented in `memory/project_doctests_round_2_2026_05_26.md` (where `gh pr edit --base main` raced with the merge queue and lost PR #220's content to a non-main branch).

Slower wall-clock than round 2's stacking, but reliable.

## 6. Inventory format

Each PR carries an inventory file as part of its diff:

`docs/superpowers/inventories/2026-05-26-bare-items-round-3.md`

Created in PR-A (protocol), extended by each subsequent PR. One section per crate. Each row:

| # | File:line | Item | Classification | Strategy |
|---|---|---|---|---|

Classification ∈ `{include, skip-trivial, skip-derived, skip-getter, skip-setter, skip-alias, skip-display, skip-marker, skip-reexport}`. For any `skip-*` other than `skip-trivial`/`skip-getter`/`skip-setter`/`skip-alias`/`skip-display`/`skip-marker`/`skip-reexport`, the row requires a one-line justification in the strategy column.

After each PR merges, that crate's rows are marked `done`. A short status log section tracks per-PR progress (same shape as round 2's inventory).

## 7. Per-PR workflow (procedure)

For each PR:

1. Branch from `origin/main` (fresh, never stacked).
2. Run the bare-item audit grep:

   ```bash
   git grep -nE '^\s*pub (fn |async fn |struct |enum |trait |type |const )' -- "crates/<crate>/src/" | grep -v 'pub(crate)'
   ```

3. For each item the grep surfaces, cross-check against existing `///` blocks:
   - If the item already has a doctest fence (` ``` ` or `no_run`), classify as `done` and skip.
   - If the item has a `///` doc but no fence, classify per §3 rules.
   - **`ignore` fences should not appear** in any of the 5 tier-1 crates after round 2 closed (PR #223 was the last). If you find one, that's a round-2 bug — log it, surface it, do NOT silently re-activate it under round 3 (the rules differ; round 2 had explicit categorization).

4. Apply the classifications:
   - **include** rows: write a doctest using §4 style.
   - **skip-*** rows: nothing to do.

5. Record every row in the inventory file (include + skip).

6. Run `cargo test --doc -p <crate>` until green. Per-row downgrade to `no_run` allowed when execution would hit a §4 forbidden side effect.

7. Commit (single commit per PR; HEREDOC body + `Co-Authored-By:`), push (`git push --no-verify`), open PR targeting `main`.

8. Wait for merge before starting the next PR.

## 8. CI impact

- `cargo test --doc -p <crate>` already runs on every PR.
- Estimated ~88 new executed doctests across the 5 PRs (some will be `no_run`). Per-PR runtime delta should stay sub-minute.
- No new CI jobs.

## 9. Success criteria

- Per-crate `cargo test --doc -p <crate>` passes green after each PR.
- `cargo clippy -p <crate> --all-targets -- -D warnings` stays clean.
- The inventory file at `docs/superpowers/inventories/2026-05-26-bare-items-round-3.md` has every bare item classified — no `?`/`TBD` rows remain after PR-E merges.
- Each crate's PR includes **at least 1 example** that demonstrates end-to-end fixture usage (re-using the round-2 patterns) — not all examples should be trivial constructor calls.
- Net `git grep -c '```' -- crates/{5 crates}/src/` increases by ~88.

## 10. Out-of-scope follow-ons

- **Round 4** — tier-2 crates: `tau-workflow`, `tau-observe`, `tau-infra`, `tau-app`.
- **Round 5** — tier-3: `tau-cli`, `tau-sandbox-*`, `tau-plugins/*`, `tau-plugin-compat`, `tau-plugin-conformance`, `tau-plugin-test-support`, `tau-plugin-base`.
- **Policy spec** — formalize §3 load-bearing rules as workspace-wide convention (would update `CLAUDE.md` and/or `CONTRIBUTING.md`).
- **Round 6+** — bare-item coverage for tier-2 and tier-3 crates after rounds 4 and 5 activate any `ignore` fences there.

## 11. Open questions

(To be resolved during writing-plans.)

- Whether `cargo doc` rendering noticeably slows on the largest PR (`tau-pkg`, est. ~33 new examples) — measure with `hyperfine` if it's borderline.
- Per-crate item count refinement — the §1 audit used a heuristic grep; the implementer must verify the actual count when classifying.
- Whether any `skip-*` classifications discovered during PR-A reveal a category we missed in §3.2 — extend the list if so.
