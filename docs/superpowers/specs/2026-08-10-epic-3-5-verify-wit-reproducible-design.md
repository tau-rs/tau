# EPIC 3.5 — `tau verify --wasm`: WIT-world reproducibility

**Status:** design approved (2026-08-10)
**Roadmap:** `docs/superpowers/plans/vision-roadmap.md`, EPIC 3, story 3.5.
**Epic DoD:** "an ungranted cap is un-importable at the ABI; wasm caps == `[allow]`-bounded set."
3.5 is the *reproducibility half*: prove the shipped WIT == `generate_world(declared caps)`.

## Problem

`tau build wasm <project>` (`crates/tau-cli/src/cmd/build_wasm.rs`) writes two
artifacts: `<out>.wasm` and a `<out>.wit` sidecar. The `.wit` is the guest's
capability-derived ABI manifest — the exact WIT world the guest was compiled
against (#543 made it load-bearing via `TAU_WORLD_WIT`). Nothing today lets a
consumer prove that a shipped `.wit` is the honest output of
`generate_world(declared caps)` rather than a hand-edited or drifted file.

3.5 closes that gap: a reproducibility check that re-derives the world from the
project's declared capabilities and byte-compares it against the shipped `.wit`.

## Confirmed constraints (verified against `main`, 2026-08-10)

- **`tau build wasm` produces NO bundle.** `build_wasm.rs:125` states it
  explicitly. There is no `.tau` bundle in the wasm path — only `<out>.wasm` +
  `<out>.wit`. So "verify the bundle's WIT" has no bundle; the source-of-truth
  for re-derivation is the **project source tree**.
- **Declared caps live only in the IR** — `module.workflow.capability_table`
  (`declared` sets). `world_from_module()` (`build_wasm.rs:103`) already
  re-derives the world from exactly this table.
- **The documented `tau.caps` custom section is dead** — doc comments only
  (`tau-ir/src/capability.rs:37`, `module.rs:116`); no writer, no reader. 3.5
  does **not** implement it.
- **`generate_world` is deterministic** (BTreeSet-sorted; order-independent —
  see `wit_world.rs` tests `output_is_deterministic_regardless_of_cap_order`).
- **`generate_world(&[])` is byte-identical to the committed
  `crates/tau-wasm-guest/wit-baseline/runner.wit`** — the empty-cap invariant,
  already load-bearing (guest build falls back to it, `build.rs:74`).

## Chosen approach — A: re-lower source, byte-compare against shipped `.wit`

Directly mirrors the existing `tau verify --bundle` "rebuild-from-source-and-
compare" harness (`tau-pkg::bundle::reproduce`, #250), reusing the
already-`pub` test seam `wasm_world_for_project`. **No IR-format bump, no new
artifact, no `tau.caps` section.**

Rejected alternatives (out of scope for the reproducibility half):
- **B — extract IR back out of the `.wasm`** (implement `tau.caps` write+read),
  re-derive, compare. Self-contained (no source tree needed) but a real
  IR-format decision — 3.6 (guest-effect-ABI) territory.
- **C — read the `.wasm`'s actual component imports** (wasmparser) and compare
  to the `.wit`. That is a *conformance* check (`.wasm` imports vs `.wit`),
  which is 3.6's job, not 3.5's reproducibility.

Approach A does not prove the `.wit` matches the `.wasm` binary — only that the
`.wit` matches the source. Because #543 compiles the guest *against*
`TAU_WORLD_WIT`, `.wasm` imports ≡ the `.wit` used to build it by construction;
the independent-tamper gap (B/C close it) is explicitly out of scope here.

## Architecture

Hexagonal: two existing seams do the work; 3.5 wires a comparison branch plus a
pure comparator.

```
read <shipped.wit>  ─────────────────────────┐
                                              ├─► compare_wit(shipped, rederived)
wasm_world_for_project(project) ─► rederived ─┘        │
   (lower_to_wasm_ir → world_from_module)               ▼
                                                  WitReproReport ─► exit 0 | 2
```

- `build_wasm::wasm_world_for_project(project) -> Result<String>` — **exists**,
  `pub`, built as a test seam. Re-lowers the project for `any-wasi-strict` and
  runs `world_from_module`. Reused unchanged.
- `compare_wit(shipped: &str, rederived: &str) -> WitReproReport` — **new**,
  pure (no I/O). Lives in `tau-cli` next to the verify command, because the
  re-lowering dependencies (`load_project`, `lower_project`) are cmd-layer and
  cannot sink into `tau-pkg` without dragging them along — matches where
  `wasm_world_for_project` already lives.
- `cmd/verify.rs` gains a wasm branch, mirroring the existing `--bundle` branch
  at `verify.rs:34`.

## CLI surface

Extend `VerifyArgs` (`crates/tau-cli/src/cli.rs:539`):

```
tau verify --wasm <PROJECT> --wit <PATH>
```

| Flag | Meaning | clap constraints |
|---|---|---|
| `--wasm <PROJECT>` | source tree to re-lower | `conflicts_with = ["package", "bundle"]` |
| `--wit <PATH>` | shipped sidecar to compare against | `requires = "wasm"` |
| `--json` | structured output (existing flag) | — |

Human output:
```
✓ WIT world reproducible (sha256 3f9a…)
```
or on drift:
```
✗ WIT world NOT reproducible
  shipped:   sha256 3f9a…
  rederived: sha256 be21…
  first diff at line 4:
    shipped:   import wasi:sockets/instance-network@0.2.3;
    rederived: import wasi:http/types@0.2.3;
```

JSON output (one object): `{ "event": "verify_wasm_wit", "reproducible": bool,
"shipped_sha256": …, "rederived_sha256": …, "first_diff": { "line": n,
"shipped": …, "rederived": … } | null }`.

## Report shape

Mirrors `ReproReport` (`tau-pkg::bundle::reproduce`):

```rust
pub struct WitReproReport {
    pub reproducible: bool,
    pub shipped_sha256: String,
    pub rederived_sha256: String,
    /// First differing line, on mismatch. None when reproducible.
    pub first_diff: Option<WitLineDiff>,
}

pub struct WitLineDiff {
    /// 1-indexed line number of the first divergence.
    pub line: usize,
    /// The shipped line (None if shipped has fewer lines).
    pub shipped: Option<String>,
    /// The re-derived line (None if re-derived has fewer lines).
    pub rederived: Option<String>,
}
```

Verdict is byte-for-byte string equality. Hashes reuse the `hex_lower` +
sha256 helper already in `cmd/build`. `first_diff` walks lines in lockstep to
the first mismatch (either side may run out of lines first).

## Error handling / exit codes

Mirror `run_reproducibility_check`'s explicit `std::process::exit`:

| Exit | Condition |
|---|---|
| `0` | reproducible (shipped `.wit` == re-derived) |
| `2` | drift (bytes differ) — print first-diff + both hashes |
| `1` | operational error (surfaced as `anyhow::Error`): project won't load; `--wit` file missing/unreadable; **project isn't wasm-buildable** (capability-fit refuses `process-exec`/`agent-spawn` → existing `LowerError::CapabilityFitFailed` message) |

The not-wasm-buildable case MUST exit `1` (operational), never `2` (drift) — a
project that cannot target wasm has no `.wit` to be reproducible.

## Testing (TDD)

Unit (new module beside `compare_wit`):
1. `compare_wit` — identical strings → `reproducible: true`, `first_diff: None`.
2. `compare_wit` — one injected import line → `reproducible: false`, `first_diff`
   names the correct 1-indexed line and both sides.
3. `compare_wit` — shipped has an extra trailing line → `first_diff` with
   `rederived: None`.

Integration (`crates/tau-cli/tests/cmd_verify.rs`):
4. Happy path — fixture project → `wasm_world_for_project` → write `.wit` →
   `tau verify --wasm <p> --wit <f>` exits 0.
5. Drift — tamper the `.wit` (inject `import wasi:sockets/...`) → exit 2; output
   names the differing line.
6. Empty-cap invariant — a host-only project re-derives to a world byte-equal to
   committed `crates/tau-wasm-guest/wit-baseline/runner.wit` (guards the frozen
   baseline against generator drift).
7. Not-wasm-buildable — a project with a `process-exec` tool → exit 1 with the
   capability-fit message, **not** exit 2.
8. `--wit` file missing → exit 1 with a clear "cannot read <path>" error.

## Out of scope (YAGNI)

- No `tau.caps` custom section; no IR-format bump.
- No reading the `.wasm` binary's imports (3.6 conformance).
- No runtime capability gate (3.4, merged).
- No change to `tau build wasm` output (the `.wit` sidecar already exists).
