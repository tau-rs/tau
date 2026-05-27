# Doctests round 4 — tier-2 crate coverage

**Status:** approved, ready for plan
**Date:** 2026-05-27
**Series:** Fourth instalment of "doctests in `///` comments." Round 1 added easy executed fences (May 14–19). Round 2 (May 25, PRs #215–#223) cleared 40 `ignore` fences. Round 3 (May 26, PRs #225–#232) added 149 load-bearing fences to tier-1 crates. Round 4 extends to tier-2 crates.

## 1. Context

Tier-2 crates are operator/library facing crates one layer below the tier-1 stable surface (`tau-runtime`, `tau-domain`, `tau-pkg`, `tau-plugin-protocol`, `tau-plugin-sdk`). Post-round-3 audit on `origin/main`:

| Crate | Pub items | Doctest blocks | Bare gap |
|---|---|---|---|
| tau-workflow | 19 | 0 | **19** |
| tau-observe | 59 | 1 (ignored) | **58** |
| tau-infra | **0** | 0 | 0 — excluded |
| tau-app | 44 | 0 | **44** |
| **total (3 in-scope crates)** | **122** | **1** | **~121** |

`tau-infra` has no public surface (workspace plumbing / re-exports only) and is excluded.

## 2. Goal

Add load-bearing `///` doctest fences to ~50% of the ~121 bare public items across the 3 in-scope tier-2 crates. Estimated **~55 new fences** across 3 PRs.

## 3. Rules (inherited from round 3 verbatim)

This spec inherits the load-bearing rules from `docs/superpowers/specs/2026-05-26-doctests-round-3-design.md`:

- **§3.1 — include:** constructors (`::new`/`::from_*`/`::with_*`/`::builder`); public trait impls (not derived); methods returning non-trivial `Result<T, E>` with meaningful error path; methods with 2+ non-self params OR generic bounds; free functions; enum variants with associated data; non-trivial conversions.

- **§3.2 — skip:** trivial getters, `Default::default()`, derived impls, type aliases, trivial setters, marker traits, `Display`/`Debug` impls, re-exports.

- **§3.3 — in doubt → include.** Non-§3.2 skip reasons need justification in inventory strategy column.

- **§4 — style:** bare ` ``` ` default; `no_run` only when execution would hit a forbidden side effect (network, runtime env-var reads, FS-outside-tempdir, real subprocess spawns); no `.unwrap()` on meaningful Results — use `.expect("msg")`; hidden `# `-prefixed setup encouraged.

- **§6 — inventory:** every public item gets a row, **including `pub fn` methods inside `impl` blocks**. Use the broader grep (per round-3 lesson):

  ```bash
  git grep -nE '^\s*pub (fn |async fn |struct |enum |trait |type |const |mod )' -- 'crates/<crate>/src/' | grep -v 'pub(crate)'
  ```

## 4. PR cadence

3 PRs in ascending bare-count order:

1. **tau-workflow** (~10 examples estimated). Depends on `tau-runtime`. Fixture examples can reuse the `MockLlmBackend` pattern from round 2/3.
2. **tau-app** (~20 examples estimated). Binary crate; library code inside it is the target. Many items may be CLI/init helpers — some will naturally be `no_run` (`std::process::exit`, real argv parsing).
3. **tau-observe** (~25 examples estimated). Tracing vocabulary (span/event constants), preview helpers (e.g., `redact_secret`), and the `Captor` test subscriber. Mostly pure data — easy executable fences.

**Why this order:** smallest first, biggest last; tau-app's binary-crate edge cases get resolved before the largest tau-observe sweep.

## 5. No-stacking discipline (verbatim from round 3 §5)

Each PR targets `main` directly and branches from current `main` at PR-open time. The next PR opens only after the previous one merges. Avoids the round-2 race documented in `memory/project_doctests_round_2_2026_05_26.md`.

Slower wall-clock than stacking, but reliable.

## 6. Inventory file

`docs/superpowers/inventories/2026-05-27-bare-items-round-4.md` — separate from round 3's. Same structure (Categories list + per-crate sections + Status log).

Created in PR-A (tau-workflow), extended by each subsequent PR.

## 7. Crate-specific notes

### 7.1 tau-workflow

- Depends on `tau-runtime`. Fixture examples need a `Runtime` + workflow TOML + a temp file. Reuse the round-2/3 `MockLlmBackend` + `make_completion_response` + `tempfile::tempdir()` patterns.
- Public surface: workflow loading, running, log/resume APIs.
- Verify `tau-workflow`'s `[dev-dependencies]` includes `tau-ports/test-fixtures`, `tau-domain/test-fixtures`, `tokio-test`, `tempfile`. Add if missing.

### 7.2 tau-app

- **Binary crate** with library code. Public items are reachable from the binary's integration tests.
- Many items may be CLI command handlers / setup glue. Some genuinely deserve `include` (config loaders, exit codes, helpers); others are binary-internal and should be `skip-binary-internal` (see §8).
- Examples that call into the real argv/process surface (`std::process::exit`, `std::env::args`) get `no_run` with one-line justification per §4.
- Verify dev-deps; the crate likely has `tau-runtime`, `tau-pkg`, `clap` (or similar) as dev/regular deps that fixtures can use.

### 7.3 tau-observe

- Tracing crate. Most items are span/event vocabulary constants (`SPAN_*`, `EVENT_*`), preview helpers (`redact_secret`, `truncate_for_logging`), and the `Captor` test subscriber.
- Already has `tracing-subscriber` in dev-deps. The `Captor` subscriber lives in this crate behind the `test-fixtures` feature — examples can use it to assert on emitted spans/events.
- Pure-data items (constants, simple helpers) → executable fences with `assert_eq!`.
- `Captor`-based examples (anything verifying instrumented behavior) → executable fences using `tracing::subscriber::with_default(Captor::default(), || { ... })` pattern.

## 8. New category (provisional)

**`skip-binary-internal`** — used **only in tau-app** for items that are `pub` for integration-test reachability but not a stable embedder API. The label is provisional; if no row in tau-app fits, the label is never added. If used, the row's strategy column explains the binary-vs-library distinction explicitly.

The category is named for symmetry with `skip-feature-gated` and `skip-needs-fixture` from rounds 2-3.

## 9. CI impact

- `cargo test --doc -p <crate>` already runs on every PR (ROADMAP §12-E doc-tests block).
- Estimated ~55 new executed doctests across 3 PRs. Per-PR runtime delta should stay sub-minute.
- No new CI jobs, no workflow changes.

## 10. Success criteria

After all 3 PRs merge:

- `cargo test --doc -p tau-workflow -p tau-observe -p tau-app` passes green.
- `cargo clippy -p tau-workflow -p tau-observe -p tau-app --all-targets -- -D warnings` clean.
- Inventory file at `docs/superpowers/inventories/2026-05-27-bare-items-round-4.md` has every bare item classified — no `?`/`TBD` rows.
- Each crate's PR includes **at least 1 example** demonstrating end-to-end fixture usage (re-using round-2/3 patterns).
- Net `git grep -c '` + triple-backtick + `' -- crates/{3 crates}/src/` increases by ~55.

## 11. Out-of-scope follow-ons

- **Round 5** — tier-3: `tau-cli`, `tau-sandbox-*` (5 crates), `tau-plugins/*` (7 crates), `tau-plugin-compat`, `tau-plugin-conformance`, `tau-plugin-test-support`, `tau-plugin-base`. Combined surface much larger; warrants its own decomposition.
- **`tau-infra` revisit** — if any items become public in the future, fold into round 5 or a future round.
- **Recurring `tau-cli` chat-test flake** (`echo-llm` plugin load failure in macOS queue runner) — confirmed across rounds 2 + 3. Investigate in a dedicated PR; not blocking this work.
- **Policy spec** — codify §3 load-bearing rules in `CLAUDE.md` or `CONTRIBUTING.md` as workspace-wide convention. Still deferred.

## 12. Open questions (resolved during writing-plans)

- Whether `tau-app` includes items behind `#[cfg]` gates the basic grep didn't catch. Re-grep at plan time.
- Whether the `Captor` subscriber in `tau-observe` is reachable from a doctest under default features. Verify Cargo.toml at plan time.
- Whether `tau-workflow`'s dev-deps need adjustment for fixture examples. Verify at plan time.
