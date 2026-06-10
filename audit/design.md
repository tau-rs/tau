# Design findings

Subsections: Code design, DX (developer experience), UX (end-user experience).
Severity scale: Critical / High / Medium / Low.

---

## Code design

### D1. `--idle-timeout` is plumbed end-to-end but never implemented

**Severity:** Medium
**Locations:**
- `crates/tau-cli/src/cli.rs:822-824` (CLI flag) → `crates/tau-cli/src/cmd/serve.rs:28` (sets `opts.idle_timeout`) → `crates/tau-app/src/serve/options.rs:39` (field exists)
- `crates/tau-app/src/serve/lifecycle.rs:16-85` (`run` never reads `idle_timeout`)

**Description:** `ServeOptions::idle_timeout` is set from the `--idle-timeout`
flag, but `lifecycle::run` only selects on `dispatcher.run` vs the
signal/EOF future — it never arms an idle timer. The serve-mode docs
(`docs/explanation/serve-mode.md`, "Shutdown … or `--idle-timeout` elapsed")
promise behaviour the code does not deliver.

**Impact:** A documented shutdown trigger is a silent no-op; embedders relying on
it to reap idle servers leak processes. The dead config field and flag are
misleading.

**Recommendation:** Implement the idle timer (reset on every inbound/outbound
message) or remove the flag/field and the doc claim until it lands.

### D2. `RunOutcome` / `RunEvent` are hand-serialized to JSON because `tau-app` doesn't enable the `serde` feature

**Severity:** Medium
**Locations:**
- `crates/tau-app/src/serve/dispatch_run.rs:169-216` (`outcome_to_json`, `token_usage_to_json`)
- `crates/tau-app/src/serve/dispatch_run.rs:269-387` (`emit_event` — manual field extraction, multiple "verified in Task 11 reconciliation" comments)

**Description:** Because `tau-app` does not turn on the upstream `serde` feature,
the serve layer manually rebuilds JSON for every runtime type, with comments
admitting the field names were reconciled by hand against the runtime source.
This is a leaky abstraction: the wire contract is duplicated away from the type
that owns it, and field renames in the runtime silently desync the protocol with
no compile-time check.

**Impact:** High drift risk; the JSON-RPC protocol can diverge from the runtime
types without any test/compiler signal. Maintenance tax on every runtime change.

**Recommendation:** Enable the `serde` feature on the runtime/domain types and
serialize them directly (or define explicit `#[serde]` DTOs in one place), so the
protocol shape is type-checked.

### D3. `Project::resolve` returns a stringly-typed "agent not found" error that the dispatcher pre-empts elsewhere

**Severity:** Low
**Locations:**
- `crates/tau-app/src/serve/project.rs:64-82` (doc says the dispatcher pattern-matches the `"agent not found: "` prefix)
- `crates/tau-app/src/serve/dispatch_run.rs:62-71` (the dispatcher actually pre-checks `config.agents.contains_key` and returns `UNKNOWN_AGENT` before ever calling `resolve`)

**Description:** The documented contract — dispatcher string-matches the error
message to map unknown agents to `-32010` — is both brittle and dead: the unknown
-agent case is already handled by a typed `contains_key` pre-check, so the string
contract in `Project::resolve` is never exercised for that purpose. Two
mechanisms claim ownership of the same decision.

**Impact:** Confusing, fragile error contract; a future refactor that trusts the
documented string-matching would reintroduce brittleness.

**Recommendation:** Return a typed `AgentNotFound` variant from `resolve` and
delete the string-prefix contract, or remove the redundant pre-check and keep one
typed path.

### D4. Package install cannot pin a commit SHA

**Severity:** Low
**Locations:**
- `crates/tau-pkg/src/git.rs:62-100` (`rev` becomes `--branch <rev> --single-branch`, which rejects 40-char SHAs)

**Description:** A `rev` is always translated to `--branch`, which accepts only
branch/tag names. Pinning to an immutable commit SHA — the security-relevant
case — fails with git's "remote branch not found." `resolve_head` records the
resolved SHA *after* the fact, so the lockfile captures it, but the user cannot
*request* a SHA.

**Impact:** Supply-chain pinning is weaker than it should be; tags/branches are
mutable.

**Recommendation:** Detect SHA-shaped revs and use clone-then-`git checkout <sha>`
(or `git fetch <sha>`), as the module's own TODO notes.

### D5. Synchronous install spins up a throwaway current-thread Tokio runtime mid-call

**Severity:** Low
**Locations:**
- `crates/tau-pkg/src/install.rs:409-427` (`Builder::new_current_thread().build()...block_on` inside the otherwise-sync `install_with_options`)

**Description:** The sync install pipeline bridges into async only for the
cross-check by constructing a one-shot runtime in the middle of the function.
This is an awkward sync/async seam that makes the install path hard to compose
(it cannot be called from within an existing async context without nested-runtime
panics).

**Impact:** Reentrancy hazard (`block_on` inside an async caller panics);
duplicated runtime setup cost.

**Recommendation:** Make the install pipeline `async` end-to-end, or hoist the
cross-check out to the async caller so the core stays sync.

---

## DX

### D6. Cargo invocation rules impose heavy, error-prone ceremony

**Severity:** Medium
**Locations:**
- `CLAUDE.md` "CARGO RULES" (six mandatory rules: per-caller `CARGO_TARGET_DIR`, `-p` scoping, per-command timeouts, `CARGO_INCREMENTAL=0`, pre-build pgrep, nextest)
- `crates/tau-pkg/src/install.rs:614-623` (the build path must defensively `env_remove("CARGO_TARGET_DIR")` to undo this very convention leaking into spawned builds)

**Description:** Building/testing requires a six-rule preamble, and the
convention is leaky enough that production code must actively strip
`CARGO_TARGET_DIR` from spawned cargo processes to avoid mis-locating plugin
binaries. The shared `target/.cargo-lock` contention being worked around is real,
but the mitigation pushes complexity onto every contributor and every
subprocess.

**Impact:** High onboarding friction; easy to get wrong; a class of "binary not
found after build" bugs traceable directly to the env leak.

**Recommendation:** Encode the policy in `.cargo/config.toml` / a thin `xtask`
wrapper (or sccache-only) so contributors don't hand-manage `CARGO_TARGET_DIR`,
and so the env var never leaks into plugin builds.

### D7. The only working secret-provisioning path is the one the code discourages

**Severity:** Medium (DX facet of S1)
**Locations:** `crates/tau-plugins/anthropic/src/config.rs:39-43, 150-160`; `crates/tau-runtime-tokio/src/plugin_host/process.rs:213`

**Description:** See S1. From a DX standpoint: a new user follows the docs, sets
`ANTHROPIC_API_KEY`, and gets `InvalidEnvVar` with no hint that the runtime
stripped the environment. The error text ("set it or use config.api_key
(test-only)") nudges them toward the insecure path.

**Recommendation:** Either forward the env var or make the error explain that the
runtime clears the environment and document the supported mechanism.

### D8. Two different `run_id` schemes coexist

**Severity:** Low
**Locations:**
- `crates/tau-cli/src/cmd/run.rs:186-192` (timestamp-nanos string, with a comment explaining the deliberate avoidance of the `uuid` dep)
- the workspace already depends on `uuid` (v7) and `ulid` (`Cargo.toml` workspace deps) and uses them elsewhere

**Description:** `run` mints a `tau-run-<nanos>` id "to avoid a uuid dep," but the
workspace already pulls in both `uuid` and `ulid`. The bespoke scheme is weaker
(collisions under fast successive runs / clock adjustment) and inconsistent with
the rest of the codebase.

**Impact:** Minor correlation ambiguity in traces; inconsistency.

**Recommendation:** Use the existing `uuid` v7 / `ulid` for run ids uniformly.

---

## UX

### D9. Bundle-verify failure bypasses the structured error renderer and hard-exits

**Severity:** Low
**Locations:**
- `crates/tau-cli/src/cmd/run.rs:84-87` (`eprintln!("error: {e}"); std::process::exit(bundle_verify_exit_code(&e));`)
- contrast with `crates/tau-cli/src/cmd/error_render.rs` (the structured renderer used by other commands)

**Description:** When bundle verification fails, `run` prints a bare
`error: <Display>` and calls `process::exit` directly, short-circuiting the CLI's
normal error-rendering and the `tau-cli/src/exit.rs` exit-code mapping used
elsewhere. Users get a less helpful message and the inconsistency makes scripting
exit codes harder to reason about.

**Impact:** Inconsistent diagnostics and exit-handling for a security-relevant
failure (a tampered/foreign bundle).

**Recommendation:** Route through the shared error renderer and return an
`ExitCode` rather than calling `process::exit` from inside the command body.

### D10. Capability/kind validation at install warns-only and is easy to miss

**Severity:** Low
**Locations:**
- `crates/tau-pkg/src/install.rs:712-754` (`warn_unknown_kind`, `warn_non_namespaced_custom_capabilities` use bare `eprintln!`)

**Description:** These advisory checks print directly with `eprintln!`
(not the tracing/diagnostics pipeline), so they neither participate in log
levels nor in any machine-readable output mode. Buried in a long build log they
are easy to miss.

**Impact:** Low signal for genuinely useful warnings (unnamespaced capabilities,
unknown package kinds).

**Recommendation:** Emit through `tracing` at `warn` (consistent with the rest of
the codebase) so they honor `RUST_LOG`, JSON output, and capture.
