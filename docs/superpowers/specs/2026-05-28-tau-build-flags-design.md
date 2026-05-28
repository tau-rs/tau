# `tau build` flags (`--target` / `-o` / `--json`) — Phase 2 §C.2.1 design

**Status:** Accepted
**Date:** 2026-05-28
**Authors:** titouanlebocq
**Depends on:** §C.2 `tau build` MVP producer (PR #242, merged), ADR-0034 target triple registry

## 1. Goal

Flesh out `tau build`'s flag surface with the three CLI-only deferrals from §C.2:

- `--target <triple>` — build for a specific Available target (default: host).
- `-o` / `--output <path>` — custom output path (default: `<project>/<name>-<version>.tau`).
- `--json` — machine-readable artifact output (honors the existing global `--json` flag).

Pure CLI-surface work: `tau_pkg::bundle::build` already accepts `target` + `output_path` and is target-agnostic by design. Validating user input is a CLI-boundary concern, so no change to the builder.

Non-goal (deferred): `--agent <id>` per-agent bundle slicing — needs new build-side manifest-filtering logic, tracked as a separate follow-up.

## 2. Headline decisions

- **No change to `tau_pkg::bundle::build`.** It already takes `target` + `output_path`. The builder stays target-agnostic (it builds for whatever triple it's given); the CLI flag handler is where user input is validated.
- **`--target` validates against the ADR-0034 registry — Available only.** Parse the `<platform>-<adapter>-<tier>` grammar, then reject Reserved/unknown triples with an error listing the Available ones (mirrors `tau check --target`). Prevents building bundles no adapter can run.
- **`--json` is the existing global flag**, routed through the `Output` struct (as `tau verify`/`check` do). `tau build` currently ignores it and uses raw `println!`; this wires it to honor `Output`.
- **All logic in `tau-cli`.** If `tau-ports::target` lacks a public "is this triple Available" helper, add a small one there rather than duplicating the registry table in tau-cli.

## 3. CLI args + dispatch

```rust
// crates/tau-cli/src/cli.rs
Command::Build(BuildArgs),   // was: Build (unit variant)

/// Arguments for `tau build`.
#[derive(Args, Debug)]
pub struct BuildArgs {
    /// Target triple to build for (default: host). Must be an
    /// Available triple in the ADR-0034 registry.
    #[arg(long, value_name = "TRIPLE")]
    pub target: Option<String>,
    /// Output path (default: `<project>/<name>-<version>.tau`).
    #[arg(long, short = 'o', value_name = "PATH")]
    pub output: Option<std::path::PathBuf>,
}
```

`--json` is NOT a `BuildArgs` field — it's the existing global `Cli::json`, read via `Output`. The dispatcher (`lib.rs`) already holds an `Output` for other commands; `cmd::build::run` changes from `run()` to `run(args: &BuildArgs, output: &mut Output)`. The dispatch arm becomes `Command::Build(args) => cmd::build::run(args, output).await`.

## 4. Target resolution + output

### 4.1 Target resolution

Factored into a unit-testable helper (so `run`'s I/O + `process::exit` don't block testing):

```rust
fn resolve_target(args: &BuildArgs) -> Result<TargetTriple, String> {
    match &args.target {
        None => Ok(TargetTriple::host()),
        Some(s) => {
            let triple = s.parse::<TargetTriple>()
                .map_err(|e| format!("invalid target triple '{s}': {e}"))?;
            if !tau_ports::target::is_available(&triple) {  // exact helper TBD at plan time
                return Err(format!(
                    "target '{triple}' is not an Available build target; available: {}",
                    available_triples_joined(),
                ));
            }
            Ok(triple)
        }
    }
}
```

`run` calls `resolve_target`, and on `Err(msg)` prints `error: {msg}` to stderr and `std::process::exit(2)`. The Available-set lookup reuses the ADR-0034 registry in `tau-ports::target` (the same source `tau check --target` consults). If no public predicate exists, add `pub fn is_available(triple: &TargetTriple) -> bool` (and a way to list Available triples) to `tau-ports::target::registry` — don't duplicate the table.

### 4.2 Output

- **Human (default):** unchanged from today — `Building bundle…` + `Wrote bundle: <path> (sha256: a3b2…f1d4, N bytes)` to stderr, bare path on stdout.
- **JSON (`output.is_json()`):** the bare-path stdout line is replaced by `{"path": "<abs>", "sha256": "<full 64-hex>", "size_bytes": N}` via `output.json(&value)`. Stderr progress is suppressed under JSON (consistent with the `Output` struct's stderr-only `status` channel — match `tau verify`/`check`).

The renderers route through the `Output` struct (`output.is_json()`, `output.json(...)`, `output.status(...)`, `output.human(...)`) rather than raw `println!`/`eprintln!`, so tests can capture them and `--quiet`/`--json` behave consistently.

### 4.3 Exit codes

Existing `tau build` codes unchanged: 0 success / 2 config-parse / 3 install-state / 70 internal. New: invalid or Reserved `--target` → 2 (bad input). `-o` to an unwritable path surfaces as the existing `BuildError::WriteFailed` → 70.

## 5. Test plan

**Unit tests in `cmd::build` (`#[cfg(test)]`):** test `resolve_target` directly (it returns `Result`, no process::exit):
- `resolve_target_defaults_to_host` — `target: None` → `TargetTriple::host()`.
- `resolve_target_accepts_available_triple` — a valid Available triple string → `Ok`.
- `resolve_target_rejects_unparseable` — garbage → `Err`.
- `resolve_target_rejects_reserved_or_unknown` — parseable but not Available → `Err`.

**Registry helper test (if added to `tau-ports`):**
- `is_available_true_for_v1_available_and_false_for_reserved` — in `tau-ports::target::registry` tests.

**CLI integration tests (extend `crates/tau-cli/tests/cmd_build.rs`):**
- `build_with_output_flag_writes_to_custom_path` — `tau build -o custom.tau` → bundle at `custom.tau`, stdout = that path.
- `build_with_json_emits_artifact_object` — `tau build --json` → stdout parses as JSON with `path`/`sha256`/`size_bytes`; `sha256` is 64 hex chars (full, not abbreviated).
- `build_with_invalid_target_exits_two` — `tau build --target not-a-real-triple` → exit 2; stderr names the bad triple + lists available targets.
- `build_with_available_target_succeeds` — `tau build --target <non-host Available triple>` → exit 0; the written bundle's `[bundle].target` equals that triple (parse to confirm). Use a triple that builds cleanly on any host (build is target-agnostic; the bundle just records the triple — `passthrough` if Available, else any non-host Available triple the plan confirms builds host-independently).

**Help snapshot:** `Build(BuildArgs)` changes `tau build --help` (now shows `--target` + `-o`). Regenerate the `build_help` snapshot.

## 6. Out of scope

- `--agent <id>` per-agent bundle slicing (needs build-side manifest filtering + a decision on dropping unreferenced packages). Separate follow-up.
- `--rotation`-style or multi-target-in-one-invocation builds. YAGNI.

## 7. References

- §C.2 spec — `2026-05-27-tau-build-design.md`
- ADR-0034 — target triple registry (the Available/Reserved source of truth)
- `tau check --target` — the existing consumer of the registry's Available check
