# tau

Agent kernel in Rust: seven syscalls, an event-sourced reducer, capability
security. `docs/adr/` records every decision; `CONTRIBUTING.md` is the long form.

## Commands

Prerequisites: `just`, `cargo-nextest`, `cargo-insta` (each via `cargo install`).

- `just check` — pre-commit gate: fmt, clippy `-D warnings`, quick tests. Seconds.
- `just test` — everything Tier 1 runs (`ci` profile plus doctests). Before a PR.
- `just abi` — wire-format snapshots and invariants for `kernel/src/abi/`.
- `just abi-review` — `cargo insta review`. A snapshot diff means the frozen
  surface moved: read it, never accept it reflexively.

## Layout

- `kernel/` — `tau-kernel`. `src/abi/` is frozen (ADR-0004); `src/bridge/` is the
  model-driver vocabulary the kernel carries for its neighbours but never uses.
- `drivers/` — the only code that touches the outside world (HTTP, model providers).
- `libtau/` — userspace over the seven syscalls: `infer()` and the tool loop.
  No special powers; anything expressible over the seven belongs here.
- `sim/` — deterministic seeded simulation of the reducer. Test-only.
- `fuzz/` — cargo-fuzz targets. Not a workspace member on purpose: needs nightly
  and would put libFuzzer's C++ runtime on the `just check` path. Own lockfile.
- `docs/adr/` — decisions. Read the ADR before touching what it governs.

## Invariants (lints, not review comments)

- `unsafe_code = "forbid"` workspace-wide.
- `HashMap`/`HashSet` and `SystemTime::now`/`Instant::now` are denied in
  `clippy.toml`. The reducer is a pure fold: a hash-ordered container in state
  is a replay bug that surfaces as a state-hash mismatch on another machine.
  Use `BTreeMap`/`BTreeSet`; time arrives as `Tick` log entries.
- `unwrap`, `expect`, `panic`, indexing, integer division: denied. `missing_docs` warns.
- ADR-0004: `kernel/src/abi/` evolves additively or not at all. A PR touching it
  bumps `ABI` or carries the `abi-change` label with a linked ADR.
- ADR-0006: the model-driver contract lives in `tau_kernel::bridge`, versioned by
  its own `v`, not by `ABI`. Driver failures are reply payloads, not exceptions.

## Testing

- Every test lives in `tests/`. The nextest `quick` profile kills a test at 5 s.
- The macOS smoke runner is 2-3x slower than a laptop in debug: keep quick-profile
  sim tests under about 1 s locally, or they time out only in CI.
- Slower tests go to the `ci` profile (60 s). `retries = 0`: a flake is a bug.
- `sim/tests/determinism.rs` pins three seeds. CI tests the merge commit with
  main, so compute new pins on top of `origin/main` and say why in the commit.

## Shipping

- `main` takes changes through the merge queue only; `gh pr merge` fails.
- Conventional commits, squash merge. The PR body carries `Closes #NNN`.
- Once `tier1` is green on the head and `gh pr view N --json mergeStateStatus`
  says `CLEAN`, enqueue with
  `gh api graphql -f query='mutation { enqueuePullRequest(input:{pullRequestId:"<gh pr view N --json id -q .id>"}) { mergeQueueEntry { state } } }'`
  and poll `gh pr view N --json state` until `MERGED`.
