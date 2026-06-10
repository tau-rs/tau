# tau — Security & Design Audit

## Project overview

tau is a minimal, terminal-native Rust runtime that installs and runs agents
(solo or orchestrated) with LLM backends, tools, skills, MCP servers and
pipelines delivered as installable packages; core ships empty and everything
domain-specific is a plugin. Its two public surfaces are the `tau-runtime` Rust
crate and **serve mode** (JSON-RPC 2.0 over NDJSON stdio), and it sandboxes
plugin subprocesses across Linux/macOS with a capability model plus an egress
proxy. This audit covers the workspace at branch `audit/design-security`.

## Findings by severity

| Severity | Count |
|---|---|
| Critical | 0 |
| High | 1 |
| Medium | 8 |
| Low | 11 |
| Total | 20 |

(Plus several explicitly-checked non-findings / strengths documented inline.)

Breakdown by file:
- `security.md` — 9 findings (S1 High; S2–S5 Medium; S6–S9 Low)
- `design.md` — 10 findings (D1, D2, D6, D7 Medium; D3, D4, D5, D8, D9, D10 Low)
- `diagnostics.md` — 5 findings (O1, O2 Medium; O3, O4, O5 Low) + 1 strength (O6)
- `devops.md` — DevOps & CI/CD audit: 10 gaps (G1, G2 High; G3–G6 Medium; G7–G10 Low) vs. the canonical DevOps model, framed with tau as the most-mature reference / template source.

## Top 5 issues

1. **Env-var API keys never reach plugins; secrets get pushed into plaintext
   `tau.toml`** (High) — `crates/tau-runtime-tokio/src/plugin_host/process.rs:213`
   `env_clear()` strips `ANTHROPIC_API_KEY`/`OPENAI_API_KEY`, so the only working
   key path is the one the code itself marks test-only
   (`crates/tau-plugins/anthropic/src/config.rs:150-160`). See **S1**.
2. **`tau install` is install-time RCE with no sandbox** (Medium) — clone +
   `cargo build` (runs `build.rs`) + spawn-for-cross-check all run unsandboxed:
   `crates/tau-pkg/src/install.rs:245,598-664,409-427`. See **S2**.
3. **`.tau` bundle "verification" is integrity, not authenticity** (Medium) —
   the self-hash is recomputed over the attacker-supplied manifest, and the IR
   payload is never cross-checked against the local `tau.toml`:
   `crates/tau-pkg/src/bundle/verify.rs:46-74,222-270`. See **S3**.
4. **Serve mode runs every plugin fully unsandboxed** (Medium) — a public,
   embeddable surface ships with `sandbox_plan = None`:
   `crates/tau-app/src/serve/lifecycle.rs:94-100,142-184`. See **S4**.
5. **`--idle-timeout` is wired through CLI → options but never implemented**
   (Medium) — documented shutdown trigger is a silent no-op, leaking idle serve
   processes: `crates/tau-app/src/serve/lifecycle.rs:16-85`. See **D1/O1**.

## Scope: prioritized vs. not covered

**Prioritized (read in depth):**
- Serve-mode transport: framing, dispatch, handshake/concurrency state, cancel
  registry, per-request execution, error mapping, lifecycle/shutdown.
- Plugin host: subprocess spawn, env handling, sandbox validation ordering,
  length-prefixed MessagePack framer + frame decode.
- Package install lifecycle: `git clone` wrapper, `cargo build`, capability
  cross-check, lockfile mutation, file locking.
- `.tau` bundle ingestion: verify pipeline, reproduce/diff, IR payload decode and
  the `tau run --bundle` dispatch into the IR interpreter.
- Sandbox egress proxy (CONNECT + HTTP paths, SNI/host allowlist) and the
  Linux net-bridge bind.
- Secrets handling in the Anthropic/OpenAI plugins.

**Not covered (or only shallow):**
- Deep review of the sandbox enforcement internals: Landlock/seccomp filter
  construction (`tau-sandbox-native` strict/light), macOS `sandbox-exec` profile
  generation (`tau-sandbox-darwin`), Windows scaffold. Trust-boundary entry points
  were read; the OS-primitive correctness was not exhaustively verified.
- The orchestration/multi-agent kernel (`tau-runtime-core/orchestration`, budgets,
  task lists, virtual tools) beyond the dispatch surface.
- `tau-ir` lowering/typecheck/capability-fit correctness (only the parse + decode
  entry points and canonical encoding were reviewed).
- Container sandbox image internals beyond `tau-plugin-base`/Dockerfile hardening.
- Dependency CVE status: `deny.toml` policy was reviewed (it is reasonable —
  crates.io-only, license allowlist, advisories v2), but no `cargo deny` run was
  performed in this environment. `[bans]` uses `multiple-versions = "warn"` and
  `wildcards = "allow"`, which is lenient but defensible.
- Test code, fixtures, and the conformance crates.

## Picking up from here

- **Worktree:** `/Users/titouanlebocq/code/tau-worktrees/audit`
  (a dedicated git worktree; `.git` points at
  `/Users/titouanlebocq/code/tau/.git/worktrees/audit`).
- **Branch:** `audit/design-security`. This audit added only `audit/**`; no source
  was modified. A single commit on this branch carries these four files.
- **Do not** touch `/Users/titouanlebocq/code/tau` or sibling worktrees under
  `tau-worktrees/` — work only in this `audit` worktree.
- **Build/test caveats:** follow `CLAUDE.md` CARGO RULES — always prefix cargo
  with `CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/<role>` and `-p <crate>` and a
  `timeout`. Note D6: the shared target-lock convention is heavy and leaks into
  spawned builds.
- **Git identity caveat (from `CLAUDE.md`):** the lefthook test suite can leave the
  worktree `user.name/email` set to `Test User <test@example.com>`. Commit with
  inline `-c user.name=... -c user.email=...` overrides (this audit commit did so).
- **Suggested remediation order:** S1 (secrets path) → S4 (serve sandbox) →
  D1/O1 (implement or remove idle-timeout) → S5 (proxy HTTP parity) →
  S3 (bundle authenticity model) → D2 (typed serve serialization).
- Every finding cites concrete `path:line` locations; start from the file
  references in each section.
