# `tau check` — design

**Date:** 2026-05-18
**Status:** Draft (pending user review)
**ADR:** None required — adding a CLI subcommand isn't in Constitution QG18's ADR-required change list (CLI flags are "intermediate stability" per QG12).
**Scope:** Phase 2 sub-project A. Aggregator CLI verb wrapping existing tau validation logic.
**Roadmap entry:** [ROADMAP.md Phase 2 §A](../../../ROADMAP.md#phase-2--tau-as-a-compiled-language-for-agentic-workflows).

## 1. Problem

Tau has plenty of validation logic, but it's scattered across:

- `tau verify` — recomputes install-tree hashes vs lockfile (catches drift)
- `tau resolve --check-sandbox` — validates sandbox plans without installing
- `tau install`'s install-time cross-checks (plugin compat, skill manifest)
- `ProjectConfig::validate` (silent — only runs as a side effect of parsing)

CI gates, IDE extensions, and pre-commit hooks need a *single command* that runs ALL the available pre-flight validation, with structured output and a meaningful exit code. Today they have to script around three or four commands and merge results. `tau check` is that single verb.

ROADMAP §A frames it as "Layer 3 validation as a first-class CLI verb." This spec turns that line into a concrete design.

## 2. Goals (v1)

- One CLI command that runs every available pre-flight check on a tau project.
- No new validation logic — pure orchestration over existing `tau-pkg` / `tau-runtime` APIs.
- Three output formats: human (default), `--json` (JSONL stream), `--sarif` (industry-standard for IDE/CI scanning).
- Granular exit codes that distinguish "your project is broken" from "your project needs setup."
- Sub-second `--fast` mode suitable for IDE Problems panels.
- No side effects unless `--auto-resolve` opts in.

## 3. Non-goals (v1)

- New validators beyond what tau-pkg / tau-runtime already expose.
- Auto-fix (`--fix`) — out of scope; possible v2 addition.
- Watch mode (`--watch`) — out of scope; possible v2.
- Schema validation for the manifest file format itself — already handled by tau-pkg parsers.
- Replacing `tau verify` or `tau resolve --check-sandbox` — those keep their current semantics; `tau check` calls them under the hood.
- Constitution / governance changes — not needed.
- ADR — not in QG18's ADR-required list. CLI subcommands are intermediate-stability per QG12.

## 4. Architecture

```
                     tau-cli/src/cmd/check/
                     ──────────────────────
                       mod.rs           CLI dispatch (clap)
                       runner.rs        orchestrator: Vec<Check> → Vec<CheckResult>
                       result.rs        CheckFinding, CheckResult, CheckStatus, Severity
                       categories/
                         config.rs      ProjectConfig validate
                         lockfile.rs    tau_pkg::verify_all wrapper
                         packages.rs    presence + version-req resolve
                         sandbox.rs     plan validation (lifted from resolve --check-sandbox)
                         plugins.rs     tau_pkg::sandbox_check::cross_check_plugin_capabilities
                         skills.rs      tau_pkg::skill_check::cross_check_skill_package
                       output/
                         human.rs       ANSI table
                         json.rs        JSONL events
                         sarif.rs       SARIF 2.1.0 document builder (hand-rolled)
```

**Pure orchestration in tau-cli.** No new code in tau-pkg or tau-runtime. Each category adapter is a thin wrapper that calls existing validators and translates their errors into a uniform `CheckFinding`.

**Sequential execution.** v1 runs checks one at a time. Future parallelization possible; v1 avoids races on plugin host fixtures during cross-checks.

**Each `Check` is an async fn** taking a shared `CheckCtx` (resolved Scope, parsed ProjectConfig, etc.) and returning a `CheckResult`. The runner builds the context once, iterates the selected categories, collects results, dispatches to the output renderer, exits.

```
                         ┌────────────────┐
   tau check [args] ──►  │  CLI dispatch  │ ──► determine category list
                         └────────┬───────┘     (all OR single OR fast variant)
                                  ▼
                         ┌────────────────┐
                         │    runner      │ ──► for each Check: invoke run()
                         │  (sequential)  │     → push CheckResult onto Vec
                         └────────┬───────┘
                                  ▼
                         ┌────────────────┐
                         │  output picker │ ──► dispatch to human/json/sarif
                         └────────┬───────┘
                                  ▼
                          stdout (format chosen)
                          stderr (tracing logs)
                          exit code (from Vec<CheckResult>)
```

## 5. CLI surface

```
tau check [OPTIONS] [SUBCOMMAND]
```

### Top-level form

Bare `tau check` runs all 6 categories:

```text
$ tau check
running 6 checks against project at /Users/.../proj …
  ✓ config          ok (12 ms)
  ✓ lockfile        ok — 5 packages, 0 drift  (43 ms)
  ✗ packages        1 missing  (3 ms)
        missing-tool 0.1.0  →  run `tau resolve` to install
  ✓ sandbox         ok — 2 agents, 2 plans built  (210 ms)
  ✗ plugins         1 plugin failed cross-check  (1.8 s)
        fs-read 0.3.0  manifest says network.http; describe says ∅
  ✓ skills          ok — 1 skill verified  (520 ms)

3 checks failed (1 needs setup, 2 fixable). exit 3
```

### Subcommands (one category each)

```
tau check config           # ProjectConfig validate
tau check lockfile         # drift detection
tau check packages         # presence + version-req check
tau check sandbox          # sandbox plan validation
tau check plugins          # plugin compat cross-check (slow)
tau check skills           # skill manifest cross-check (slow)
```

### Flags (accepted on all forms)

| Flag | Meaning |
|---|---|
| `--fast` | Use reduced-I/O variant where one exists (see §6 table). Always accepted. |
| `--auto-resolve` | Run a lazy resolve (install missing packages) before checks. Affects `packages` outcomes. |
| `--json` | Emit JSONL stream to stdout. Mutually exclusive with `--sarif`. |
| `--sarif [<path>]` | Emit SARIF 2.1.0 document. With path → write file; without → stdout. Mutually exclusive with `--json`. |
| `--project <path>` | Project root override (default: cwd, walking up to find `tau.toml`). |
| `--no-color` | Disable ANSI in human output. Honors global `--color` flag too. |

### Usage error cases

| Invocation | Result |
|---|---|
| `tau check` outside a tau project | exit 64 — "not inside a tau project (no tau.toml in cwd or ancestors)" |
| `tau check --json --sarif` | exit 64 — "--json and --sarif are mutually exclusive" |
| `tau check nonexistent-category` | exit 64 — clap default with help text listing valid categories |
| `tau check sandbox --fast` on a check with no fast variant | no error; flag is no-op. Documented behavior, not surprising. |

### Examples by workflow

```bash
# PR gate
tau check --fast --json

# Nightly / pre-release
tau check

# IDE — Problems panel via SARIF
tau check --sarif=/tmp/tau.sarif

# Targeted debugging
tau check sandbox
tau check plugins --fast   # static-only on plugins
```

## 6. Check categories

Each category has a default (full) mode and a `--fast` mode. `--fast` always reduces I/O — never silently skips a category.

| # | Category | Default (full) | `--fast` |
|---|---|---|---|
| 1 | `config` | `ProjectConfig::from_path(root.join("tau.toml"))` — parse + validate. | No-op. No slow portion exists. |
| 2 | `lockfile` | `tau_pkg::verify_all_with_options(&scope, /*anthropic_strict=*/false)` — recompute tree-hash for each installed package, diff vs lockfile.binary_sha256. | No-op. |
| 3 | `packages` | For each agent in `project.config.agents.values()`, for each `requires.tools` entry, resolve against lockfile. Mark missing or version-incompatible. | No-op. |
| 4 | `sandbox` | For each agent: read `[sandbox]` from scope config, resolve adapter, `build_plan(agent)` + `validate_plan_against_adapter(plan, adapter)`. Fail-collecting. | Build the plan only; skip adapter probe + `validate_plan_against_adapter`. Catches structural errors in capability overrides without spawning probes. |
| 5 | `plugins` | For each installed plugin: spawn binary, `meta.handshake`, `tool.describe_capabilities`. Bidirectionally diff against manifest's declared `provides` / `requires`. Reuses `tau_pkg::sandbox_check::cross_check_plugin_capabilities`. | Check that binary exists at lockfile-recorded path, is executable; verify manifest static-field consistency. No live spawn. |
| 6 | `skills` | For each installed skill: parse manifest, run `tau_pkg::skill_check::cross_check_skill_package`. Spawn backing plugin if any, enumerate live grants, diff vs manifest. | Parse manifest + verify capability declarations are internally consistent. No live spawn. |

### Empty-input handling

- `tau check skills` with no skills installed → result is `Status::Skipped { reason: "no skills installed" }`, not an error.
- `tau check plugins` with no plugins → same.
- `tau check sandbox` with no agents → same.

Skipped is informational; doesn't affect exit code.

## 7. Output formats

### Human (default)

ANSI-colored summary as shown in §5. Footer summarizes pass/fail counts and exit code. Findings indent under their category.

### JSON / JSONL (`--json`)

One JSON object per line. Stable schema. Stream-friendly.

```jsonc
{"type":"run_started","project_root":"/abs/path","categories":["config","lockfile","packages","sandbox","plugins","skills"],"fast":false}
{"type":"check_started","category":"config","timestamp":"2026-05-18T10:30:00Z"}
{"type":"check_finished","category":"config","status":"ok","duration_ms":12,"findings":[]}
{"type":"check_finished","category":"packages","status":"failed","duration_ms":3,
 "findings":[
   {"severity":"needs-setup","category":"packages","rule_id":"tau.packages.missing",
    "summary":"missing-tool ^0.1 required by agent reviewer; run `tau resolve`",
    "structured":{"agent_id":"reviewer","package":"missing-tool","version_req":"^0.1",
                  "installed_version":null},
    "location":{"path":"tau.toml","line":17,"column":5},
    "remediation":"tau resolve"}
 ]}
{"type":"run_finished","duration_ms":2588,"summary":{"ok":4,"failed":2,
  "by_severity":{"error":2,"needs-setup":1}},"exit_code":3}
```

`type` discriminator + flat fields. No `data` wrapper. Each event self-contained.

### SARIF 2.1.0 (`--sarif [<path>]`)

```jsonc
{
  "version": "2.1.0",
  "$schema": "https://schemastore.azurewebsites.net/schemas/json/sarif-2.1.0.json",
  "runs": [{
    "tool": {
      "driver": {
        "name": "tau",
        "version": "0.7.0",
        "informationUri": "https://tau.dev",
        "rules": [
          {"id":"tau.config.invalid", "shortDescription":{"text":"Invalid tau.toml"}},
          {"id":"tau.lockfile.drift", "shortDescription":{"text":"Install tree drifted from lockfile"}},
          {"id":"tau.packages.missing", "shortDescription":{"text":"Required package not installed"}},
          {"id":"tau.sandbox.plan_invalid", "shortDescription":{"text":"Sandbox plan validation failed"}},
          {"id":"tau.plugins.mismatch", "shortDescription":{"text":"Plugin contract mismatch"}},
          {"id":"tau.skills.mismatch", "shortDescription":{"text":"Skill manifest mismatch"}}
        ]
      }
    },
    "results": [{
      "ruleId": "tau.packages.missing",
      "level": "warning",
      "message": {"text": "missing-tool ^0.1 required by agent reviewer; run `tau resolve`"},
      "locations": [{"physicalLocation": {
        "artifactLocation": {"uri": "tau.toml"},
        "region": {"startLine": 17, "startColumn": 5}
      }}],
      "properties": {"severity": "needs-setup", "category": "packages"}
    }]
  }]
}
```

**Level mapping (SARIF `level` field):**

| tau severity | SARIF level |
|---|---|
| `error` | `"error"` |
| `needs-setup` | `"warning"` |
| `warning` | `"note"` |

Custom `properties` carry tau-specific severity for consumers that care.

**Implementation:** hand-roll using `serde_json::Value` (~80 LOC). No SARIF crate dependency.

**With bare `--sarif` (no path): emit to stdout.** Matches `cargo --message-format=json` style.

### File-line location

Best-effort:
- `config` findings → `tau.toml` with parser-supplied line/col.
- `packages` findings → the `[[agents.<id>.requires.tools]]` entry when resolvable; else `tau.toml:1`.
- `lockfile` findings → `tau.lockfile` (a synthetic line — the file is auto-generated).
- `sandbox` findings → scope `config.toml` or per-agent capability override location in `tau.toml`.
- `plugins` / `skills` findings → the plugin/skill manifest file at its lockfile-recorded path.

## 8. Error model

```rust
pub struct CheckFinding {
    pub category: CheckCategory,
    pub severity: Severity,
    pub rule_id: &'static str,                  // "tau.lockfile.drift" etc.
    pub summary: String,
    pub detail: Option<String>,
    pub location: Option<FindingLocation>,
    pub remediation: Option<String>,            // e.g. "tau resolve"
    pub structured: serde_json::Value,          // category-specific
}

pub enum CheckCategory { Config, Lockfile, Packages, Sandbox, Plugins, Skills }

pub enum Severity { Error, NeedsSetup, Warning, Note }

pub struct CheckResult {
    pub category: CheckCategory,
    pub status: CheckStatus,                    // Ok | Failed | Skipped { reason }
    pub findings: Vec<CheckFinding>,
    pub duration: Duration,
}

pub struct FindingLocation {
    pub path: PathBuf,    // relative to project root
    pub line: Option<u32>,
    pub column: Option<u32>,
}
```

### Source-to-finding mapping

Each category adapter (`categories/<name>.rs`) translates errors:

| Source error | → | Finding shape |
|---|---|---|
| `ProjectConfigError::*` | → | category=Config, severity=Error, rule_id="tau.config.invalid", location=tau.toml + variant's line/col |
| `VerifyStatus::TreeDrift / BinaryDrift / Missing` | → | category=Lockfile, severity=Error, rule_id="tau.lockfile.drift", structured carries variant |
| `AgentResolutionError::PackageNotFound` | → | category=Packages, severity=NeedsSetup, rule_id="tau.packages.missing", remediation="tau resolve" |
| `SandboxValidationFailed { details }` | → | category=Sandbox, severity=Error, rule_id="tau.sandbox.plan_invalid", structured carries details |
| `CrossCheckError::*` | → | category=Plugins, severity=Error, rule_id="tau.plugins.mismatch" |
| skill cross-check error | → | category=Skills, severity=Error, rule_id="tau.skills.mismatch" |

### Exit-code computation

```rust
fn compute_exit(results: &[CheckResult]) -> i32 {
    let mut has_error = false;
    let mut has_setup = false;
    for r in results {
        for f in &r.findings {
            match f.severity {
                Severity::Error => has_error = true,
                Severity::NeedsSetup => has_setup = true,
                Severity::Warning => {}
            }
        }
    }
    match (has_error, has_setup) {
        (false, false) => 0,    // clean
        (true, _)      => 2,    // real bug wins
        (false, true)  => 3,    // only setup needed → CI can auto-resolve and retry
    }
}
```

`Severity::Error` beats `NeedsSetup` — surface real bugs to the developer before they get masked by a "needs setup" wall.

| Exit | Meaning |
|---|---|
| `0` | All selected checks passed |
| `2` | At least one fixable failure (drift, config error, capability problem, …) |
| `3` | Only setup-needed failures (missing packages); run `tau resolve` and retry |
| `64` | Usage error (sysexits E_USAGE) |
| `70` | Internal error / panic (sysexits E_SOFTWARE) |

**A freshly scaffolded project exits 2, by design.** `tau init` writes an
example agent with empty `package` / `model` for the user to fill in; those
blanks fail `ProjectConfig` validation, so `config` emits `Severity::Error`.
Exit 3 is *not* the right code for this: it is scoped above to "missing
packages; run `tau resolve` and retry" — conditions a **command** resolves.
Nothing but the user editing the file resolves a blank field. A 2026-08-23 QA
sweep read this as a defect; it is not one, and
`cmd_check_exit_codes.rs::fresh_scaffold_exits_2_by_design` pins it.

Runner-level panics → 70 + log to stderr. A check failing *to even run* (e.g., plugin binary crashed during cross-check) → that's a Finding with `Severity::Error` and `rule_id="tau.plugins.cross_check_internal_error"`, NOT exit 70. Runner-level errors are about the orchestrator itself.

## 9. Testing strategy

### Layer 1 — Unit tests (~30 tests)

In `crates/tau-cli/src/cmd/check/*.rs`:

| Module | Coverage |
|---|---|
| `result.rs` | `compute_exit` precedence (6 tests covering 0/2/3 mapping). |
| `output/human.rs` | Rendered fixture results → `insta` snapshots (4 snapshots). |
| `output/json.rs` | JSONL emission shape — fixture findings → expected event sequence (5 tests). |
| `output/sarif.rs` | SARIF document builder snapshot (5 tests). |
| `categories/<each>.rs::mapper` | One test per tau-pkg error variant → CheckFinding shape (~10 tests). |

Pure-logic tests use hand-constructed values, no I/O.

### Layer 2 — Integration tests (~15 tests)

In `crates/tau-cli/tests/cmd_check_*.rs`. Use fixture projects under `tests/fixtures/check/`. Drive via `Command::cargo_bin("tau")` (mirrors `cmd_resolve.rs`).

**Fixtures:**

| Fixture | Purpose |
|---|---|
| `clean-project/` | All checks pass. Tests exit 0 + happy-path rendering. |
| `bad-config-project/` | Malformed tau.toml. Tests `config` failure + accurate line numbers. |
| `drifted-lockfile/` | Lockfile present but installed files modified. Tests `lockfile` drift. |
| `missing-package-project/` | tau.toml requires a tool with no lockfile entry. Tests `packages` + exit 3. |
| `invalid-sandbox-override/` | Capability override that doesn't subset manifest grants. |

**Test files:**

| File | Scenarios |
|---|---|
| `cmd_check_clean.rs` | bare `tau check` → exit 0 |
| `cmd_check_config.rs` | parse + validate failures; `--fast` no-op |
| `cmd_check_lockfile.rs` | drift; `--fast` no-op |
| `cmd_check_packages.rs` | missing, version mismatch, `--auto-resolve` |
| `cmd_check_sandbox.rs` | plan validation failure; `--fast` skips probe |
| `cmd_check_subcommands.rs` | single-category subcommands run only that category |
| `cmd_check_output_formats.rs` | --json, --sarif, mutual exclusion |
| `cmd_check_exit_codes.rs` | 0/2/3/64/70 precedence (error-over-setup) |

Each test sets `$TAU_HOME` to a per-process tempdir via the OnceLock pattern from PR #143 ([feedback memory](../../../docs/superpowers/specs/2026-05-17-tau-serve-mode-design.md)).

**Plugin + skill checks (slow) — Layer 2 deferred.** Building real plugin fixtures with manifest+binary+capability declarations requires substantial test infra (controlled-env-binary style). Defer Layer 2 coverage of `plugins`/`skills` categories. Covered indirectly through the existing `cross_check_plugin_capabilities` tests in tau-pkg.

### Layer 3 — End-to-end (deferred)

No Layer 3 in v1. Reason: requires real plugin fixtures with real lockfile entries. The existing fixture infrastructure (`tau-plugin-compat`) is plumbed only for sandbox tests. Building check-specific Layer 3 fixtures is its own sub-project. Layer 2 with fake fixtures is sufficient for v1 confidence.

### Snapshot strategy

`insta` for:
- Human output format (ANSI-stripped) per fixture project
- SARIF document structure per fixture

JSON event sequence verified by event-counting + key assertions (snapshot is fragile due to timestamps).

### CI coverage

No new CI job needed. Layer 1 + Layer 2 tests get picked up by the existing `test-stable / linux,macos,windows` matrix workspace-wide nextest run. Pattern confirmed working for PR #143.

### Performance budget

`tau check --fast` on the `clean-project` fixture should complete in **<100ms** (5 fast checks + 1 fast variant of sandbox). Asserted via `tokio::time::timeout(Duration::from_millis(500), ...)` — generous margin to absorb CI noise. Bare `tau check` (full) on the same fixture: no formal budget, but should be <2s without slow plugins/skills checks (which need real fixtures to trigger).

## 10. Risks & tradeoffs

- **`--auto-resolve` ergonomics across categories.** When running just `tau check sandbox --auto-resolve`, do we resolve missing packages even though the sandbox check doesn't strictly need them installed? Decision: yes, always run resolve before any check when `--auto-resolve` is set. Consistent behavior; trivial cost.
- **SARIF `level=warning` for needs-setup.** Some GitHub Code Scanning configs only show errors. Decision documented; users who want missing-package issues to surface as scanning errors can use `--json` instead.
- **Human format and color in CI.** Honor existing tau-cli `--color` global flag + `NO_COLOR` env var conventions. The example output uses Unicode `✓`/`✗`; fall back to `OK`/`FAIL` ASCII when terminal can't render Unicode.
- **Performance regression risk.** Adding `tau check` to PR-gate CI scripts increases per-PR wall time. v1's `--fast` mode keeps the addition <200ms on a clean project. Heavier full-run mode is opt-in.
- **Layer 3 absence.** Without e2e tests, real-world `tau check` behavior on a project with real plugins is untested. Mitigation: Layer 2 with fake fixtures covers the orchestration path; the underlying validators (cross_check_plugin_capabilities, etc.) have their own e2e coverage in tau-pkg. The unique surface in tau-cli is orchestration + output formatting, both well-covered by Layer 1+2.

## 11. Out of scope (forever or until concrete demand)

- `--fix` auto-remediation (writes to files)
- `--watch` (continuous mode)
- Per-finding suppression via comments in tau.toml
- Custom rule sets / `.tau-checkrc` config files for tuning severity
- `tau check --dry-run` (already pure-read by default)
- Diff mode (`tau check --since main`) — Phase 2 sub-project A is enough; this becomes a separate effort

## 12. Open implementation questions

- Exact file-line location reporting for `lockfile` findings — the file is auto-generated, so a "line" is synthetic. Decision: report the file path only, no line.
- Whether `sandbox` finding locations should point at scope `config.toml` (where the adapter is configured) or `tau.toml` (where the agent's capability overrides live). Decision: when the failure stems from a capability override mismatch, point at `tau.toml`; otherwise point at `config.toml`. Adapter decides per-error.
- Whether to color-fade the `(123 ms)` timing on the human output — minor aesthetic. Decision deferred to implementation.

## 13. Companion follow-ups (not in this PR)

- `tau check --since <ref>` (diff mode) — Phase 2 future sub-project.
- A built-in `--watch` mode pairs naturally with serve-mode v2's `runtime.events` notification. Consider together when serve mode v2 lands.
- SARIF `helpUri` for each rule — point at user-doc anchors once those docs exist. Today the doc anchors don't all exist; including stale URIs is worse than including none.
