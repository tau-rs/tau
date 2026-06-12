# DevOps & CI/CD audit

Focus: the CI/CD pipeline, local developer-experience tooling, supply-chain
posture, and how tau aligns with the **canonical DevOps model** shared verbatim
across the four sibling projects (cairn, cairn-ui, tau, tau-ui).

**Framing.** tau is the MOST MATURE CI of the four and is effectively the
reference implementation. Its existing workflow set (changes-detection,
feature/no-std matrices, conformance, fuzz-nightly, mutants-scheduled,
docs-deploy, ci-summary grace logic, xtask, lefthook, least-privilege
`permissions`) is largely the TEMPLATE the other three should converge toward.
So this section is less "fix tau" and more **(a)** the small remaining gaps in
tau, and **(b)** documenting tau as the canonical source the synced template is
extracted FROM. Real gaps in tau are still flagged honestly.

Severity / priority scale: High / Medium / Low.

| Priority | Count |
|---|---|
| High | 2 |
| Medium | 5 |
| Low | 4 |
| Total | 11 |

---

## 1. Current state

### 1.1 What is already best-in-class (credit where due)

These are genuine strengths and are the parts other repos should copy verbatim.

- **Changes-detection fast-path.** `ci.yml:50-109` computes `skip_heavy_jobs`
  with a deliberately conservative two-filter `dorny/paths-filter` gate
  (`every` quantifier for the safe-set AND `some` quantifier requiring an actual
  test-file change). The reasoning — why workspace-root `Cargo.toml` is excluded
  and why per-crate manifests are enumerated at two depths to dodge a picomatch
  glob bug (PR #158) — is documented inline. This is exemplary.
- **`ci-summary` as the single required check + grace logic.**
  `ci-summary.yml:57-129` polls the GitHub API for the CI run on the exact head
  SHA rather than relying on `workflow_run` (which only fires on default-branch
  history — the bug that let PR #92 merge red). It reports success on
  `success|skipped|neutral`, keeps waiting on `cancelled` (successor expected
  under `cancel-in-progress`), and treats "no run after 120 s" as a docs-only
  paths-ignore skip (`ci-summary.yml:83,94-101`). This is the green-if-all-pass
  OR-all-skip contract the canonical model's T1 tier requires, done correctly.
- **rust-cache save-only-on-main.** `.github/actions/setup-rust/action.yml:128`
  sets `save-if: ${{ github.ref == 'refs/heads/main' }}`, and
  `mutants-scheduled.yml:98` sets `save-if: "false"` for read-only reuse — PRs
  restore but never write, eliminating write contention between parallel
  same-key jobs. The `concurrency` block (`ci.yml:29-31`) correctly EXEMPTS main
  from `cancel-in-progress` precisely so a cache write isn't aborted mid-flight.
  This pairing is exactly the canonical hardening rule.
- **no_std / executor-agnostic + feature matrices.**
  `ci.yml:218-235` (per-crate `--no-default-features`),
  `ci.yml:237-267` (`tau-runtime-core` no-std build plus a grep gate forbidding
  module-level `tokio/embassy/smol/async_std/std::` imports plus an
  executor-agnostic smoke test). This is correctness coverage most repos lack.
- **Heavy correctness suite.** Conformance against all 3 plugins
  (`ci.yml:350-370`), plugin-compat layers incl. the `#[ignore]`'d layer-4
  sandbox-boundary tests promoted DARK→LIT (`ci.yml:416-498`), native-sandbox
  and runtime e2e (`ci.yml:500-548`).
- **Scheduled drift-catchers.** `fuzz-nightly.yml` (nightly cargo-fuzz, per-target
  corpus cache, crash-artifact upload) and `mutants-scheduled.yml` (weekly
  per-crate cargo-mutants, missed-mutant report artifact). Both have
  `workflow_dispatch` with inputs and `timeout-minutes` (30 / 240).
- **xtask + lefthook tiered local gate.** `xtask/src/main.rs:29-39` exposes
  `build-base-image` / `build-plugin-images` (container-image build, runtime
  auto-detect, buildx-cache passthrough). `lefthook.yml` runs a fast parallel
  pre-commit (fmt / clippy `-D warnings` / nextest / musl cross-check) and a
  deep pre-push gate that reproduces EVERY Linux CI job inside one Podman
  container — local == CI by construction.
- **Least-privilege permissions.** Workflow-level `permissions: contents: read`
  on `ci.yml:46-47`, `ci-summary.yml:54-55`, `coverage.yml:33-34`,
  `docs-check.yml:14-15`; per-job write scoping in `docs-deploy.yml:41,170,265-267`
  and `claude.yml:81-86`; `pull_request` (not `pull_request_target`) chosen to
  keep fork PRs away from secrets (`claude.yml:24-31`, `claude-review.yml:41-47`).
- **Supply-chain policy + automation present.** `deny.toml` (advisories v2,
  `yanked = "deny"`, license allowlist, crates.io-only sources),
  `EmbarkStudios/cargo-deny-action@v2` in T1 (`ci.yml:136-145`), Dependabot
  configured for BOTH `github-actions` and `cargo` ecosystems with thoughtful
  grouping (`.github/dependabot.yml`).
- **Lockfile + MSRV checks exist.** `msrv-check` runs `cargo check --locked` at
  the pinned MSRV `1.91` (`ci.yml:180-197`) — both `--locked` enforcement and an
  MSRV signal are already present in T1.
- **Timeouts where it matters most.** `coverage.yml:40`, `fuzz-nightly.yml:47`,
  `mutants-scheduled.yml:56`, `ci-summary.yml:61`, `auto-rerun-flaky.yml:43`.

### 1.2 Honest remaining gaps vs the canonical model

- **G1 (High) — No explicit `v*`-tag HEAVY / release anchor.** The canonical T2
  tier wants the heavy release-blocking suite gated on `push` of a `v*` tag (the
  user's "heavy lifting on feature release") plus `workflow_dispatch`. Today
  tau's heavy work is split between **per-PR inline** (conformance, sandbox/runtime
  e2e, feature matrix, layer-4 — `ci.yml:350-548`) and **schedule**
  (`fuzz-nightly.yml`, `mutants-scheduled.yml`). No workflow triggers on `v*`
  tags except `docs-deploy.yml:5-7` and the `release`-event Claude summary. There
  is no single consolidated release gate that says "this tag is releasable." A
  `v*` tag can be pushed and a Release published without the fuzz/mutants/MSRV
  suite ever having run against that exact commit.
- **G2 (High) — Actions are tag-pinned, not SHA-pinned.** Every third-party
  action is pinned by mutable tag/branch: `actions/checkout@v6` (×25),
  `actions/upload-artifact@v7` (×6), `actions/cache@v5` (×4),
  `anthropics/claude-code-action@beta` (×3 — a MOVING branch),
  `taiki-e/install-action@v2`, `Swatinem/rust-cache@v2`,
  `dtolnay/rust-toolchain@stable` and `@nightly`, `dorny/paths-filter@v4`,
  `docker/setup-buildx-action@v4`, `actions/download-artifact@v8`,
  `rui314/setup-mold@v1`, `peaceiris/actions-gh-pages@v4`,
  `mozilla-actions/sccache-action@v0.0.10`, `EmbarkStudios/cargo-deny-action@v2`.
  The canonical model mandates **SHA-pinned + Renovate/Dependabot**. The good
  news: Dependabot is already wired and updates SHA pins as readily as tags
  (`.github/dependabot.yml:10-13` even references the pending SHA-pin
  recommendation "PR #70"), so this is finishing started work, not new design.
  `@beta` is the sharpest edge — a moving branch on a security-sensitive action
  with `contents: write` (`claude.yml:82`).
- **G3 (Medium) — No SBOM / supply-chain artifact.** The canonical T2 marks SBOM
  (CycloneDX) as CORE. tau has `cargo-deny` (policy gate) but emits no SBOM and
  produces no release build / GitHub Release artifact. `cargo cyclonedx` (or
  `anchore/sbom-action`) generating an SBOM in the heavy/release workflow is
  absent.
- **G4 (Medium) — No `justfile` universal wrapper.** The model standardizes
  identical verbs (`just fmt/lint/test/deny/ci/heavy/fix`) across all four repos
  so local == CI and cross-repo muscle memory holds. tau has xtask + lefthook but
  no `justfile` (confirmed: none at repo root). The verbs live implicitly inside
  `lefthook.yml` run-strings and `ci.yml` steps, duplicated rather than shared.
- **G5 (Medium) — MSRV runs in T1 but is not a distinct release gate.**
  `msrv-check` (`ci.yml:180-197`) runs per-PR, which is good, but the canonical
  T2 wants MSRV explicitly re-asserted as a release-blocking gate on the `v*`
  tag (the version you ship is the version whose MSRV claim you guarantee). Today
  nothing re-checks MSRV at tag time.
- **G6 (Medium) — `cargo-deny` and `cargo-mutants` install unpinned at runtime.**
  `mutants-scheduled.yml:105` runs `cargo install cargo-mutants --locked` and
  `fuzz-nightly.yml:94` runs `cargo +nightly install cargo-fuzz --locked` with no
  version pin — `--locked` pins the build's own deps but not the tool version, so
  a breaking tool release can turn the scheduled job red unpredictably. Lower
  blast radius (schedule only) but still drift.
- **G7 (Low) — `timeout-minutes` missing on the entire `ci.yml` matrix.** None of
  the jobs in `ci.yml` (fmt, clippy, test-stable, the e2e/conformance jobs,
  build-fixtures, etc.) set `timeout-minutes`. The canonical hardening rule is
  "timeout-minutes on EVERY job"; a hung test or cargo lock-wait currently rides
  GitHub's 6-hour default. `docs-deploy.yml` jobs likewise lack timeouts.
- **G8 (Low) — `claude.yml` has no `concurrency` group.** `claude-review.yml`,
  `ci.yml`, etc. all have concurrency groups; `claude.yml` (the @mention bot,
  `contents: write`) does not, so two near-simultaneous @claude mentions can race
  on the same PR branch.
- **G9 (Low) — `merge_group` queue covered by CI but coverage workflow is not in
  the queue.** `ci.yml`/`ci-summary.yml` handle `merge_group:` correctly, but
  `coverage.yml:15-22` runs on `pull_request` + `push: main` only — fine (coverage
  is measurement-not-gating per its own header) but worth noting the queue's
  required-check surface is exactly `ci-summary` and nothing else.
- **G10 (Low) — cosign / SLSA provenance absent.** Canonical-optional phase-2;
  no signing or provenance attestation on (currently non-existent) release
  artifacts. Tracked here only for completeness; not actionable until G1 lands a
  release build.
- **G11 (Medium) — pre-push `deep-gate` is a heavy podman gate in a git hook.**
  **✅ Resolved by #305 (commit `bcbcfc3`).** `lefthook.yml` no longer has a
  `pre-push:` section at all (top-level keys are `pre-commit:` and `deep-gate:`);
  the `deep-gate:` group is now opt-in, run on demand via `lefthook run deep-gate`,
  so a plain `git push` runs no hook and no longer hard-fails in podman-less
  environments. Original finding retained below for the record.
  `lefthook.yml:74` defines a `pre-push: deep-gate:` command that runs a
  privileged podman/container-based check on EVERY push, reproducing every Linux
  CI job inside one container (~3-4 min warm, ~15-20 min cold per its own header).
  This (a) violates the lightweight-hooks principle (heavy/container work belongs
  in the T2 `v*` heavy tier, not on `git push`), and (b) hard-fails in any
  environment without a podman socket (`/run/podman/podman.sock`), so any
  contributor or agent runtime lacking podman cannot push without `--no-verify` —
  the gate relocates CI latency onto the developer and is routinely bypassed
  anyway (CLAUDE.md's AGENT PUSH RULES exist precisely because the gate silently
  kills agent-driven `git push`). Compounding this, a local `core.hooksPath`
  override makes lefthook skip its hook-sync, and the lefthook integration-test
  suite corrupts the worktree git identity to `Test User <test@example.com>`
  (both documented in CLAUDE.md), so the hooks installed locally drift from
  `lefthook.yml` and commits silently pick up the wrong author.
  **Recommendation:** relocate `deep-gate` to the T2 heavy CI tier (the
  `v*`-tag `heavy.yml` of G1) or a dedicated CI job; keep pre-push fast (a `just
  ci` subset) or absent. Fix the local `core.hooksPath` override with `lefthook
  install --reset-hooks-path` so lefthook owns hook sync again.

---

## 2. Target model (canonical model applied to tau)

tau = the template source. The model below is what the other three repos converge
TO; tau already implements most of it, so "target" here means "tau plus G1–G10."

### Diagram 1 — Anti-drift "B+C" (tau as the SOURCE of the synced template)

```
                         ┌───────────────────────────────────────────┐
                         │  tau  (CANONICAL SOURCE — most mature CI)   │
                         │  .github/workflows/*  +  .github/actions/*  │
                         └───────────────────┬───────────────────────┘
                                             │  extract template
                                             ▼
                         ┌───────────────────────────────────────────┐
                         │  synced workflow template (B+C)            │
                         │  • each repo owns FULL self-contained .yml │
                         │  • thin SHA-pinned composite actions only  │
                         └───────┬───────────────┬───────────────┬────┘
              sync-bot PRs       │               │               │
        (Renovate / repo-file-sync / multi-gitter; drift = VISIBLE open PR)
              ┌─────────────────┘               │               └──────────────┐
              ▼                                  ▼                              ▼
      ┌──────────────┐                  ┌──────────────┐               ┌──────────────┐
      │ cairn        │                  │ cairn-ui     │               │ tau-ui       │
      │ ci.yml(self) │                  │ ci.yml(self) │               │ ci.yml(self) │
      └──────┬───────┘                  └──────┬───────┘               └──────┬───────┘
             │ uses ./.github/actions/setup-rust (composite, SHA-pinned)      │
             └────────────────────────────┬────────────────────────────────┘
                                          ▼
                         ┌───────────────────────────────────────────┐
                         │  thin composite actions (stable atomics)   │
                         │  setup-rust  ◀── tau ALREADY HAS THIS       │
                         │  (place-fixture-binaries, cache)            │
                         │  pinned by COMMIT SHA                        │
                         └───────────────────────────────────────────┘

   NO runtime SPOF: no repo `workflow_call`s a central moving-tag workflow.
   REJECTED: central reusable workflows invoked at runtime via a moving tag
             (blast radius — one bad push reds all repos; + indirection).
```

tau's `setup-rust` (`.github/actions/setup-rust/action.yml`) is exactly the
composite-action layer the model wants: a stable atomic step (toolchain + cache +
optional nextest/sccache/mold) consumed by every job via
`uses: ./.github/actions/setup-rust`. It is the exemplar the other repos copy.

### Diagram 2 — Tiered pipeline T0–T3 (heavy re-anchored on `v*`)

```
 T0  LOCAL          lefthook + just/xtask
     (seconds)      fmt · clippy -D warnings · fast unit on staged
                    └─ tau: lefthook.yml pre-commit (parallel) ✔

 T1  PR / merge_group   FAST GATE  (<10 min, cancel-in-progress≠main)
     ┌────────────────────────────────────────────────────────────┐
     │ changes-detection → fmt + clippy(-D warnings) → unit + doc  │
     │ → cargo-deny → lockfile --locked → build → ONE ci-summary   │
     │ required check (green if all pass OR all skip)              │
     └────────────────────────────────────────────────────────────┘
        tau: ci.yml + ci-summary.yml ✔ (largely complete)

 T2  HEAVY   on push of v* tag  +  workflow_dispatch     ◀── GAP G1
     ┌────────────────────────────────────────────────────────────┐
     │ full OS matrix · MSRV(gate) · feature-powerset · fuzz ·     │
     │ mutation · coverage · conformance/sandbox/runtime e2e ·     │
     │ SBOM (cyclonedx) CORE · release build → GitHub Release ·    │
     │ [cosign + SLSA provenance — phase-2 optional]               │
     └────────────────────────────────────────────────────────────┘
        tau TODAY: heavy work runs INLINE in T1 + on SCHEDULE.
        MODEL: ALSO anchor the release-blocking suite on the v* tag.

 T3  SCHEDULED  (nightly / weekly) — drift-catchers
        tau: fuzz-nightly.yml (daily) + mutants-scheduled.yml (weekly) ✔
        KEEP as-is; they catch drift between releases.
```

### Diagram 3 — `just` wrapping tau's existing xtask

```
   developer / lefthook / CI   ── all call the SAME verbs ──▶
        ┌──────────────────────────────────────────────┐
        │  justfile  (universal verbs, identical x4 repos)│
        │  just fmt   → cargo fmt --all -- --check        │
        │  just lint  → cargo clippy --workspace -Dwarnings│
        │  just test  → cargo nextest run --workspace     │
        │  just deny  → cargo deny check --all-features    │
        │  just ci    → fmt + lint + test + deny           │
        │  just heavy → xtask build-plugin-images + e2e    │  ◀ WRAPS xtask
        │  just fix   → cargo fmt + clippy --fix           │
        └───────────────┬──────────────────────────────────┘
                        │ delegates container/image work to
                        ▼
                ┌──────────────────────────┐
                │ xtask  (build-base-image, │  tau ALREADY HAS xtask;
                │  build-plugin-images)     │  justfile WRAPS it, never
                └──────────────────────────┘  replaces it.

   lefthook.yml run-strings  ─┐
   ci.yml step `run:`        ─┼─▶ all reduce to `just <verb>`  ⇒ local == CI
```

### Diagram 4 — tau-specific building-block status

```
   CANONICAL BUILDING BLOCK                 tau STATUS
   ─────────────────────────────────────    ───────────────────────────────
   changes-detection fast-path              ✔ HAVE  (ci.yml:50)
   ci-summary single required check         ✔ HAVE  (ci-summary.yml)
   rust-cache save-if main                  ✔ HAVE  (setup-rust:128)
   no_std / executor-agnostic gate          ✔ HAVE  (ci.yml:237)
   feature / no-default-features matrix     ✔ HAVE  (ci.yml:218)
   conformance + e2e + layer-4 promotion    ✔ HAVE  (ci.yml:350-548)
   fuzz nightly  +  mutants weekly          ✔ HAVE  (T3 drift-catchers)
   cargo-deny policy gate                   ✔ HAVE  (ci.yml:136)
   Dependabot (actions + cargo)             ✔ HAVE  (.github/dependabot.yml)
   least-privilege permissions              ✔ HAVE  (contents: read everywhere)
   composite action (setup-rust)            ✔ HAVE  (the exemplar)
   xtask task runner                        ✔ HAVE  (xtask/src/main.rs)
   lefthook T0 + deep pre-push gate         ✔ HAVE  (lefthook.yml)
   ─────────────────────────────────────    ───────────────────────────────
   v*-tag HEAVY / release anchor            ✘ MISSING  (G1, High)
   actions SHA-pinned                       ✘ TAG-PINNED (G2, High)
   SBOM (cyclonedx) / supply-chain artifact ✘ MISSING  (G3, Medium)
   justfile universal-verb wrapper          ✘ MISSING  (G4, Medium)
   MSRV as release gate (on v*)             ◐ PARTIAL  (T1 only — G5, Medium)
   pinned CI-tool installs (mutants/fuzz)   ◐ UNPINNED (G6, Medium)
   timeout-minutes on every job             ◐ PARTIAL  (G7, Low — ci.yml gaps)
   cosign / SLSA provenance                 ✘ MISSING  (G10, Low — phase-2)
```

---

## 3. Anti-drift & local DX (B+C applied to tau)

- **Self-contained files (B).** Each repo keeps its FULL `ci.yml` etc. so one bad
  change can never red all four at once and every `ci.yml` is debuggable locally.
  tau already does this — there is no runtime `workflow_call` to a central repo.
  This is the property the model wants preserved as the template propagates.
- **Thin composite actions (C).** Stable atomic steps only. tau's
  `setup-rust` (toolchain + cache + nextest/sccache/mold) and
  `place-fixture-binaries` are the model's composite layer. Action: SHA-pin the
  third-party actions THEY call (G2) so the composite layer is itself immutable.
- **tau is the sync SOURCE.** Extract tau's `.github/workflows/*` +
  `.github/actions/*` as the canonical template; a sync bot
  (Renovate / `BetaHuhn/repo-file-sync-action` / `multi-gitter`) opens PRs into
  cairn / cairn-ui / tau-ui. Drift becomes a VISIBLE open PR, not silent rot.
- **REJECTED (documented).** Central reusable workflows called at runtime via
  `workflow_call` with a moving tag — blast radius (one push reds all repos) and
  indirection (can't read one repo's CI in isolation). Considered and rejected in
  favor of B+C.
- **Phase-2 (optional).** A projen-style generator with a `synth`-diff CI check
  that fails if a repo's committed workflows drift from the generated source.
- **`just` universal wrapper.** Add a `justfile` exposing
  `fmt / lint / test / deny / ci / heavy / fix` with byte-identical verbs in all
  four repos. In tau it WRAPS xtask (delegates image/e2e work) and lefthook + CI
  both call the same `just <verb>` so local == CI by construction, not by
  parallel maintenance of two copies of each command string.
- **Git hooks stay lightweight.** Pre-commit runs ONLY the fast `just` verbs
  (fmt, lint, fast staged tests) — seconds, never blocking. NO heavy or
  container-based checks belong in git hooks. Heavy correctness work runs in the
  T2 `v*`-tag heavy CI tier and T3 schedules, never on `git commit` / `git push`.
  A pre-push hook, if present, runs at most a fast `just ci` subset. Rationale:
  pushes must stay fast; a slow pre-push gate just relocates CI latency onto the
  developer and gets bypassed with `--no-verify` anyway. (tau currently violates
  this — see G11.)

---

## 4. Implementation checklist (ordered, tau-specific)

Execute top-to-bottom; later items assume earlier ones. Each line carries a
priority and a one-line rationale.

- [ ] **Add `heavy.yml` triggered on `push: tags: v*` + `workflow_dispatch`,
  consolidating the release-blocking suite** — full OS matrix, MSRV (gate),
  feature-powerset, fuzz, mutation, coverage, and the conformance / sandbox /
  runtime e2e currently inline in `ci.yml:350-548`. **High** — gives "heavy
  lifting on feature release" an explicit, auditable trigger so a `v*` tag means
  "this exact commit passed the full suite" (closes G1, G5).
- [ ] **SHA-pin every third-party action and convert
  `anthropics/claude-code-action@beta` off the moving branch** — Dependabot is
  already wired (`.github/dependabot.yml:10-13`) and updates SHA pins. **High** —
  removes mutable-supply-chain risk on actions, including one with `contents:
  write` (closes G2). `@beta` first.
- [ ] **Add SBOM generation (CycloneDX) to `heavy.yml` and attach to the GitHub
  Release** — e.g. `cargo cyclonedx` or `anchore/sbom-action`. **Medium** —
  canonical T2 marks SBOM CORE; pairs naturally with the new release build
  (closes G3).
- [ ] **Add a release build + `GitHub Release` step in `heavy.yml`** —
  **Medium** — there is currently no release artifact at all; required before
  SBOM/cosign have anything to attach to.
- [ ] **Add a root `justfile` wrapping xtask + existing verbs
  (`fmt/lint/test/deny/ci/heavy/fix`)** and refactor `lefthook.yml` run-strings
  and `ci.yml` steps to call `just <verb>`. **Medium** — single source of truth
  for each command so local == CI; aligns tau with the cross-repo verb contract
  (closes G4).
- [ ] **Move lefthook pre-push `deep-gate` (podman) out of git hooks into a CI
  job; keep pre-push fast/absent. Fix local `core.hooksPath` override (`lefthook
  install --reset-hooks-path`).** **Medium** — heavy/container checks belong in
  the T2 heavy tier, not on `git push`; the gate hard-fails without a podman
  socket and is routinely bypassed, and the hooksPath override desyncs installed
  hooks from `lefthook.yml` (closes G11).
- [ ] **Pin tool versions for `cargo install cargo-mutants` / `cargo-fuzz`**
  (`mutants-scheduled.yml:105`, `fuzz-nightly.yml:94`) via
  `taiki-e/install-action` or an explicit `--version`. **Medium** — stops a
  breaking tool release from silently reddening the scheduled jobs (closes G6).
- [ ] **Add `timeout-minutes` to every job in `ci.yml` (and `docs-deploy.yml`)**
  — **Low** — completes the "timeout on every job" hardening rule; a hung job
  currently rides GitHub's 6-hour default (closes G7).
- [ ] **Add a `concurrency` group to `claude.yml`** — **Low** — prevents two
  @claude mentions racing on the same PR branch with `contents: write` (closes
  G8).
- [ ] **Extract tau's `.github/workflows/*` + `.github/actions/*` as the canonical
  sync template and stand up the sync bot** (Renovate / repo-file-sync /
  multi-gitter) opening PRs into cairn / cairn-ui / tau-ui. **Medium** —
  operationalizes "tau = source of truth"; drift becomes a visible PR.
- [ ] **(Phase-2, optional) Add cosign signing + SLSA provenance attestation to
  the release artifacts produced by `heavy.yml`** — **Low** — canonical-optional;
  only actionable once a release build exists (closes G10).
- [ ] **(Phase-2, optional) Add a projen-style generator + `synth`-diff CI check**
  — **Low** — fails CI if committed workflows drift from the generated source.

---

## Picking up from here

- Worktree: `/Users/titouanlebocq/code/tau-worktrees/audit`, branch
  `audit/design-security`. This section added only `audit/devops.md` and one line
  in `audit/README.md`; no source or other audit file was modified.
- Start from the checklist in §4. The two High items (`v*`-tag `heavy.yml`,
  SHA-pinning) are the highest-leverage and unblock the supply-chain items.
- All `path:line` references are to the worktree's `.github/` tree as read for
  this audit; verify against current HEAD before editing.
