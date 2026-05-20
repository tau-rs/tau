# Run mutation testing

cargo-mutants mutates production code (`<` → `>`, `Some(x)` → `None`,
`x?` → `x.unwrap()`, etc.) and runs the test suite against each
mutant. A mutant is **caught** if a test fails, **missed** if every
test still passes (a test gap).

## When to run it

- **Before adding tests to "untested" code**: surface which expressions
  aren't yet exercised by any test.
- **After a large refactor**: catch behaviour-equivalent typos.
- **Before bumping a crate's "stability" tier**: measure objective
  test catch-rate.

Not run in CI by default — a full workspace mutation run is hours.

## One-time setup

```bash
cargo install cargo-mutants --locked
```

## Run

```bash
# List mutants without running them (fast — seconds).
cargo mutants --list

# Whole-workspace run (hours).
cargo mutants --in-place

# One crate at a time (recommended for first runs — minutes per crate).
cargo mutants --in-place -p tau-domain
cargo mutants --in-place -p tau-pkg
```

The `--in-place` flag mutates the actual source tree; without it,
cargo-mutants copies the workspace to a scratch dir per worker. In-place
is faster but you must have a clean working tree (uncommitted changes
will be reverted on mutant rollback).

Useful flags:
- `--jobs N` — parallel workers (default 1 on machines with low free
  memory; bump to 4–8 on dev boxes)
- `--timeout N` — per-mutant timeout in seconds (default in
  `mutants.toml` is 300)
- `--no-shuffle` — deterministic ordering for diff comparison
- `-f <path>` — restrict to mutants in a specific file

## Interpreting results

cargo-mutants writes `mutants.out/` with:
- `caught.txt` — mutants that at least one test caught (good)
- `missed.txt` — mutants no test caught (test gap)
- `timeout.txt` — mutants that timed out (suspicious; often indicates
  the mutation triggered an infinite loop the tests don't cover)
- `unviable.txt` — mutants that don't compile (safe to ignore)

The catch rate is `caught / (caught + missed + timeout)`. Healthy
target for a crate: ≥ 80%.

## Triaging a missed mutant

Each entry in `missed.txt` looks like:

```
src/lockfile.rs:435: replace LockFile::from_toml_str -> Result<Self, RegistryError> with Ok(Default::default())
```

Steps:
1. Read the line referenced. Decide if the mutation matters:
   - **Yes, this changes behaviour** → add a test that fails when the
     line is mutated (typically the same shape as the original assert
     but on a slightly different input).
   - **No, this is dead code or a defensive fallback no caller can
     hit** → consider deleting the dead code, or add a `// mutants:
     skip` comment.

The `// mutants: skip` annotation tells cargo-mutants to skip a specific
mutant. Use sparingly — every skipped mutant is a test gap by another
name.

## Config

`mutants.toml` at the workspace root excludes:

- All `tests/`, `benches/`, `fuzz/` paths (mutating tests is meaningless)
- `xtask/`, `tau-plugin-test-support/`, `tau-plugin-conformance/`
  (test infrastructure)
- Logging macros (`tracing::*`, `println!`, etc.) — uncatchable by
  any practical test

If you find a class of false-positive mutants that aren't worth
catching, add a regex to `exclude_re` in `mutants.toml` rather than
sprinkling `// mutants: skip` comments throughout the codebase.

## CI integration

Not enabled by default. A `workflow_dispatch`-only nightly run is the
natural follow-up — uploads `mutants.out/missed.txt` as an artifact
per-crate, doesn't block PRs.
