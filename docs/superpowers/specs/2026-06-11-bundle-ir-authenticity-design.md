# Bundle IR authenticity: cross-check the executed IR against the verified source

- **Date:** 2026-06-11
- **Status:** accepted
- **Findings addressed:** S3 (bundle verification proves self-consistency, not
  authenticity) — full fix this cycle. S2 (`tau install` builds + runs untrusted
  code with no sandbox) — documented trust boundary only; code mitigation
  deferred to a follow-up.

## Problem

`tau run --bundle <file.tau>` verifies a `.tau` bundle before executing it
(`crates/tau-pkg/src/bundle/verify.rs::verify_bundle`). The verification
pipeline proves several relationships independently:

- step 3 — the bundle's recorded `self_hash` matches its own canonical content;
- step 6 — the cwd `tau.toml` hashes to `project.tau_toml_sha256`;
- step 9 — the embedded IR payload bytes hash to `ir_payload.canonical_ir_hash`.

When a v2 bundle carries an `ir_payload`, the run path
(`crates/tau-cli/src/cmd/run.rs:78-118`) decodes that IR and executes it via
`crate::cmd::ir_dispatcher::run_via_ir`. The IR — not the cwd `tau.toml` — drives
what the workflow can do: `workflow.agents[*].tool_refs` decides which tools the
agent is wired to call (`agent_loop.rs:401`), and `workflow.capability_table`
carries the per-tool capability requirements (`node.rs`, `capability.rs`).

The gap: **nothing ties the executed IR to the `tau.toml` the user inspected.**
Both hashes the pipeline checks were written by the bundle's builder, so a
malicious builder can satisfy both while making them describe *different*
workflows:

```
benign tau.toml (1 tool, no fs caps)   ← what the user reads
+ IR with extra tool_refs / wider capability_table
+ recompute self_hash over the doctored manifest
= verify_bundle: all green → escalated IR executes
```

`verify_self_hash` is an **integrity** check (did the bytes change since the
builder sealed them?) currently being read as an **authenticity** check (is this
what the source says?). The two are not the same, and the word "verified" in the
run path overstates the guarantee.

### Verification graph (today)

```
                       .tau bundle
  ┌────────────────────────────────────────────────────┐
  │  self_hash ───────────── covers whole manifest ✅   │
  │  project.tau_toml_sha256                            │
  │        ▼ step 6 ✅                                   │
  │   cwd tau.toml  ◄── what the USER inspects          │
  │  ir_payload.canonical_ir_hash                       │
  │        ▼ step 9 ✅                                   │
  │   ir_payload.bytes ──► decoded IrModule ──► EXECUTED │
  └────────────────────────────────────────────────────┘

           cwd tau.toml  ◄────✗ NO EDGE ✗────►  executed IR
```

## Decision

Add the missing edge: re-lower the verified `tau.toml` and require the
recomputed canonical IR hash to equal the bundle's recorded
`ir_payload.canonical_ir_hash`.

```
   cwd tau.toml  (proven byte-clean by step 6)
        │  lower_ir(cwd, bundle.target)   ← same fn `tau build` / `tau verify` use
        ▼
   recomputed canonical_ir_hash
        │  NEW step 10: must equal
        ▼
   bundle.ir_payload.canonical_ir_hash ══ step 9 ══ executed IR bytes
```

Transitively: `source_relower_hash == stored_ir_hash` (new step 10) combined
with `stored_ir_hash == actual_IR_bytes_hash` (existing step 9) proves
**executed IR ≡ the lowering of the `tau.toml` the user inspected**.

This is the same equivalence `tau verify --bundle` already commits to: that
command re-lowers the local tree (`verify.rs:385-399`) and rejects on any drift,
including `tau_version` skew. We are folding that guarantee into the run gate so
`tau run --bundle` cannot execute an IR that the local source does not produce.

### Why full-hash and not a capability projection

A narrower "compare only `capability_table` + `tool_refs`" check was considered
and rejected:

| | full hash (chosen) | capability projection |
|---|---|---|
| Closes named escalation (caps / tool_refs) | ✅ | ✅ |
| Closes unnamed divergence (swapped prompt/model, edge rewiring) | ✅ — IR ≡ source | ❌ — only projected fields |
| Future IR fields (β.4 context, new node types) | covered automatically | projection must be maintained forever; a forgotten field is a silent new hole |
| Cross-tau-version bundles | ❌ rejected (tau_version ∈ canonical bytes) | ✅ tolerated |
| Failure mode when the check itself is wrong | false **reject** (loud, safe) | false **accept** (silent, unsafe) |
| Consistency with `tau verify --bundle` | identical equivalence | a second, weaker notion |

The projection is an allowlist of "fields we remembered to check" — the
escape-hatch-by-default shape tau avoids. The full hash is fail-closed: the
artifact either provably corresponds to the source or it does not run. The only
cost is that a bundle built by tau version X must be run by tau version X — the
same pinning `tau verify --bundle` already enforces, and acceptable because
bundles are already host-sealed (step 5 rejects a foreign target triple). If
cross-version tolerance ever becomes a real need, a projection can be added
*later* as a deliberate, explicit relaxation; the reverse (tightening B→A) would
be breaking.

## Implementation shape

### Layering

`tau-ir` depends on `tau-pkg`, so the re-lowering (which lives in
`tau-cli::cmd::build::lower_ir`) cannot move into `tau-pkg`. The split mirrors
the existing `verify_reproducible` precedent, which takes a caller-supplied
`ir_payload` in `ReproOptions`:

- **tau-cli (`run.rs`)** re-lowers the verified source and hands the result to
  the verifier.
- **tau-pkg (`verify.rs`)** owns the comparison + the typed error, keeping the
  typed-error discipline in the same crate as the rest of the pipeline.

### tau-pkg changes (`crates/tau-pkg/src/bundle/`)

1. `VerifyOptions` gains a field:

   ```rust
   /// The canonical IR hash recomputed by re-lowering the cwd source,
   /// supplied by the caller (tau-cli owns lowering — see layering).
   /// `None` means the caller could not lower the source.
   pub recomputed_ir_hash: Option<String>,
   ```

2. `verify_bundle` gains **step 10** (`verify_ir_matches_source`), run after the
   existing IR-payload integrity check (step 9):

   - If `manifest.ir_payload` is `None` (v1 bundle): no-op. v1 bundles have no
     IR to diverge; the cwd path already executes the proven-clean `tau.toml`.
   - If `manifest.ir_payload` is `Some` and `recomputed_ir_hash` is `Some`:
     the two hashes **must** be equal. Mismatch → `IrSourceDivergence`.
   - If `manifest.ir_payload` is `Some` and `recomputed_ir_hash` is `None`
     (the source no longer lowers, or the caller declined to lower): **fail
     closed** → `IrSourceUnverifiable`. A v2 bundle whose source cannot be
     re-lowered cannot be authenticated and must not run.

3. New `VerifyError` variants, both mapped to **exit 3** (the
   integrity/escalation bucket) in `run.rs::bundle_verify_exit_code`:

   ```rust
   /// The IR embedded in the bundle does not match what the local
   /// (verified) tau.toml lowers to — a capability/workflow divergence.
   IrSourceDivergence { bundle_hash: String, source_hash: String },
   /// The bundle carries an IR payload, but the local source could not
   /// be re-lowered to authenticate it. Fail-closed.
   IrSourceUnverifiable,
   ```

### tau-cli changes (`crates/tau-cli/src/cmd/run.rs`)

In the `--bundle` branch, before calling `verify_bundle`, re-lower the cwd
source for the host target and pass the recomputed hash in:

```rust
// Bundles are host-sealed (verify step 5), so lowering for the host
// target is correct; a foreign-target bundle is rejected by step 5
// before the divergence check is reached.
let empty_mcp_cache = std::collections::BTreeMap::new();
let recomputed_ir_hash = crate::cmd::build::lower_ir(
    &cwd, &tau_ports::target::TargetTriple::host(), &empty_mcp_cache, None,
).map(|p| p.canonical_ir_hash);

let report = tau_pkg::bundle::verify_bundle(tau_pkg::bundle::VerifyOptions {
    bundle_path: bundle_path.clone(),
    project_root: cwd.clone(),
    recomputed_ir_hash,
})?; // existing error → exit-code mapping handles the new variants
```

`lower_ir` returning `None` flows through as `recomputed_ir_hash: None`, which
step 10 turns into the fail-closed `IrSourceUnverifiable` for a v2 bundle. The
existing `verify --bundle` reproduce path (`verify.rs:385`) and the
`verify_reproducible` callers are unaffected — they construct their own options
type.

Note (MCP limitation, fail-closed): `lower_ir` uses an empty MCP cache here,
matching the existing `run_reproducibility_check` precedent. `tau build`, by
contrast, resolves MCP contracts (live or pinned) and embeds the expanded
server-tool nodes, so a v2 bundle built from a project that uses MCP tools
re-lowers to a *different* hash and is conservatively rejected with
`IrSourceDivergence` — it cannot currently be run via `--bundle`. (This is
*not* the `IrSourceUnverifiable` path, which only fires when `lower_ir` returns
`None`; an MCP project still lowers to `Some`, just with a divergent hash.)
This is safe — fail-closed, never fail-open, and the same limitation
`verify --bundle` already has. Wiring the pinned MCP cache
(`build::resolve_mcp_cache(cwd, offline = true)`) into this re-lowering so
honest MCP bundles verify is a tracked follow-up, listed alongside S2.

### Integrity-vs-authenticity language (S3, second half)

- Rename / re-document `verify_self_hash` and the run-path log lines so
  "verified" never implies a signature. The self-hash is an **integrity
  checksum** the builder computed over its own output — it proves the bundle has
  not been corrupted in transit, not that its contents are authentic or that the
  author is trusted.
- Add a module-level doc note on `verify.rs` distinguishing the three guarantees
  the pipeline now provides: **integrity** (self-hash, step 3), **source
  correspondence** (tau.toml + IR ↔ executed IR, steps 6/9/10), and the absence
  of **authenticity** (no signature / no author trust — that is S2's domain).

### S2 — documented trust boundary, code deferred

`tau install <url>` clones an arbitrary repo, runs `cargo build` (executing
`build.rs` + proc macros), and spawns the freshly built binary for the Layer-2
capability cross-check — all outside the Layer-4 sandbox. This is RCE-by-design
for an untrusted package and is a larger change (running the install-time build
+ cross-check under a sandbox tier, or a network-restricted build) that warrants
its own design cycle.

This cycle ships only a **loud trust-boundary doc**: `tau install <source>`
executes the author's code on your machine before any capability or sandbox
enforcement applies — installing a package is equivalent to trusting its author.
No `install.rs` code changes, so there is no collision with the install-path
diagnostics session (45).

## Testing (TDD — failing test first)

1. **Divergence rejected (the core test):** build a v2 bundle, then construct a
   manifest whose `ir_payload` declares a capability / tool_ref **not** present
   in what the `tau.toml` lowers to, recompute the bundle self-hash so step 3
   still passes, and assert `verify_bundle` returns `IrSourceDivergence` (exit
   3) — i.e. the doctored bundle is rejected *before* execution. This is the
   test that fails against `main` today.
2. **Matching bundle passes:** an untampered v2 bundle whose IR is the genuine
   lowering of its `tau.toml` verifies clean (recomputed hash == stored hash).
3. **Fail-closed on unlowerable source:** v2 bundle + `recomputed_ir_hash: None`
   → `IrSourceUnverifiable`.
4. **v1 bundle unaffected:** a bundle with no `ir_payload` verifies regardless of
   `recomputed_ir_hash` (no IR to diverge).
5. Existing `verify.rs` and `verify --bundle` suites stay green.

## Out of scope

- S2 install-time sandboxing (deferred, documented above).
- Cryptographic signing / authorship attestation for bundles (would be the real
  "authenticity" layer; not in this finding).
- Cross-tau-version bundle portability (explicitly rejected by the full-hash
  decision).
- Any refactor of the install pipeline or the lowering pass beyond what the
  cross-check requires.
