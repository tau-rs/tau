# EPIC 8 — Identity + positioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Write the session's constitutional identity decisions into committed docs (1 new ADR + 2 doc reframes) so in-flight parallel work (durability #373, β.7.5 #372/#369) stays aligned to the vision.

**Architecture:** Pure-docs epic. The "tests" are `mdbook build` (clean, `[INFO]`-only) + `mdbook-linkcheck` (clean) — there is no Rust to compile. Story 8.1 adds ADR-0055 and wires it into the two ADR indices the repo maintains. Stories 8.2/8.3 reframe `philosophy.md` and `ROADMAP.md` to a component-first identity.

**Tech Stack:** mdBook 0.4 + mdbook-linkcheck + mdbook-mermaid (`^0.14`), Markdown. Binaries at `~/.cargo/bin/{mdbook,mdbook-linkcheck}` (not on PATH — prepend `$HOME/.cargo/bin`).

## Global Constraints

- **Branch:** `feat/epic-8-identity` off `main`.
- **ADR number:** `0055` (latest existing is `0054`).
- **ADRs ARE book pages in this repo** (handoff's generic "ADRs aren't book pages" note is WRONG here): every ADR `0001`–`0054` except `0039` is listed in BOTH `docs/SUMMARY.md` AND the index table in `docs/decisions/README.md`. ADR-0055 MUST be added to both, or the book/index drifts.
- **mdBook build (the "test"):** `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build` → only `[INFO]` lines, then `rm -rf docs/book`.
- **Mermaid label gotcha:** use `(1)`/`Step 1`, never `1.`; use `[…]` (Unicode ellipsis) or ```` ```text ```` fences inside/after mermaid blocks. (No new mermaid added here, but applies if any is introduced.)
- **Commit identity (lefthook can corrupt it):**
  `git -c user.name="LEBOCQ Titouan" -c user.email="75916953+LEBOCQTitouan@users.noreply.github.com" commit --no-verify -m "..."`
- **Commits:** Conventional, imperative, scoped. End body with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Push/merge:** docs-only → no deep gate; plain `git push`. Repo has a MERGE QUEUE: enroll with `gh pr merge <#> --auto` (no `--delete-branch`, no strategy flag — the queue owns it).
- **Repo:** `tau-rs/tau` (transferred from `LEBOCQTitouan/tau`). Live docs: `lebocqtitouan.github.io/tau/latest/`.
- **PR base:** `main`. PR shape: ONE PR for the epic (three commits, one per story) — see "Execution Handoff".

---

## Task 1: Branch setup

**Files:** none (git only).

- [ ] **Step 1: Create the feature branch off main**

```bash
cd /Users/titouanlebocq/conductor/workspaces/tau/salvador
git fetch origin main
git switch -c feat/epic-8-identity origin/main
```

Expected: `Switched to a new branch 'feat/epic-8-identity'`. (Working tree clean; this is a docs-only line of work.)

- [ ] **Step 2: Confirm baseline book builds BEFORE any edits**

```bash
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd ..
rm -rf docs/book
```

Expected: only `[INFO]` lines, no `[WARN]`/`[ERROR]`. Establishes that any later failure is ours. If it fails on `main`, STOP and report — do not build on a broken baseline.

---

## Task 2: Story 8.1 — ADR-0055 (identity: two contracts; CLI = reference host)

**Files:**
- Create: `docs/decisions/0055-tau-identity-two-contracts.md`
- Modify: `docs/SUMMARY.md` (append ADR-0055 line after the ADR-0054 line)
- Modify: `docs/decisions/README.md` (append ADR-0055 row to the Index table)

**Interfaces:**
- Produces: ADR-0055 — referenced by stories 8.2 (philosophy.md) and 8.3 (ROADMAP.md) as the identity authority; status **Accepted**.

- [ ] **Step 1: Write the ADR file**

Create `docs/decisions/0055-tau-identity-two-contracts.md` with EXACTLY:

```markdown
# ADR-0055: tau identity — a compiler+engine between two contracts; the CLI is the reference host

**Status:** Accepted
**Date:** 2026-06-22
**Deciders:** tau core

## Context

The 2026-06-20 roadmap-challenge / vision-reframe session (verified deep
research) concluded that tau is a **combination play**: every individual axis
(IR/durability, edge-wasm, MCP-hosting, wasm-sandboxing, prompt-compilation,
per-tool sandboxing, on-MCU agents) already has a more-focused rival ahead.
The only unoccupied square is the **intersection + conformance +
vendor-independence + root-governed capability safety**.

To defend that square, the project must be precise about *what tau's product
is*. Historically the docs lead with "the `tau` CLI" and frame edge / browser
/ embedded as downstream build targets of a CLI-centric application. That
framing privileges one host, makes the other hosts look like afterthoughts,
and — most damagingly — risks tying tau's public stability surface to CLI
verbs that should be free to churn. Parallel work is in flight (durability
#373, β.7.5 #369/#372); without a locked identity it will drift.

[`docs/explanation/tau-philosophy.md`](../explanation/tau-philosophy.md)
already establishes Conviction 1 ("tau is a *compiler*, not a framework").
This ADR locks the precise product boundary that conviction implies.

## Decision

**tau's product is `tau-runtime-core` (the engine) plus TWO versioned public
contracts:**

1. **The authoring / IR schema** (JSON). This INCLUDES the root `[allow]`
   governance section — the capability ceiling and resource registry of the
   `tau.toml` constitution. `[allow]` is the *governance section of the
   authoring contract*, not a separate ABI (there are two contracts, not
   three).
2. **The WIT host world** (the embedding interface). The WIT world is
   **generated from the no_std ports** — it is never hand-maintained, so it
   cannot drift from the engine it describes.

**The `tau` CLI is the REFERENCE HOST, not the product.** The analogy is
LLVM: the product is the LLVM core + the IR; `clang` is one reference
frontend/driver built on it. For tau, the product is `tau-runtime-core` + the
two contracts; the `tau` CLI is one reference host/embedder that exercises
them.

**The public stability / semver surface is the two contracts + the no_std
ports API.** CLI verbs get a separate, looser compatibility policy (documented
with the CLI, not governed by the contract semver).

**The CLI is held to the highest quality bar.** It is the on-ramp and the
example that edge / browser / embedded embedders copy. This decision demotes
the CLI's *architectural privilege* (it is one host among peers), not its
*importance* (it remains the reference standard).

## Consequences

- **Docs lead with component + contracts.** Features are framed engine-first;
  `philosophy.md` (Story 8.2) and `ROADMAP.md` (Story 8.3) are reframed to
  match. Future ADRs frame features as engine + contract changes, then note
  the CLI surface.
- **Edge / browser / embedded hosts are PEERS of the CLI** — each is a host of
  one component, not a downstream target of a CLI-centric product.
- **Versioning + conformance attach to the two contracts**; CLI verbs evolve
  under the looser policy. This is what lets the CLI stay the
  highest-quality, fastest-moving reference surface without destabilising
  embedders.
- **New obligation:** the WIT host world must be *generated* from the ports
  (no hand-maintained drift). Locking and codegen of both contracts is EPIC 2
  ("Lock the two contracts"); this ADR is the identity premise EPIC 2
  implements.
- **Neutral:** no code changes in this ADR. It is constitutional framing that
  constrains how subsequent engine, durability, and β.7.5 work is described
  and versioned.

## Alternatives considered

- **CLI-as-product (status-quo framing).** Rejected: it privileges one host,
  makes edge / browser / embedded read as afterthoughts, and ties the public
  stability surface to CLI verbs that must be free to churn. The trade-off it
  imposes — a frozen CLI or unstable embedders — is exactly the failure this
  ADR prevents.
- **Three contracts (authoring schema, `[allow]` governance, WIT world).**
  Rejected: `[allow]` is the governance *section* of the authoring contract,
  not an independent ABI with its own version line (reconciliation R1 of the
  vision audit). Counting it separately would create a third versioned surface
  that always moves in lockstep with the first — ceremony without value.
- **Engine-only product (contracts are implementation detail, not product).**
  Rejected: an engine without versioned, conformance-checked contracts is not
  embeddable or provable-identical-across-targets. The contracts *are* the
  product surface; hiding them would forfeit the moat (conformance +
  vendor-independence).
```

- [ ] **Step 2: Add ADR-0055 to `docs/SUMMARY.md`**

Find the last ADR line:

```
- [ADR-0054 — In-wasm MCP facilitator (β.7.5)](decisions/0054-in-wasm-mcp-facilitator.md)
```

Insert immediately AFTER it:

```
- [ADR-0055 — tau identity: two contracts; CLI = reference host](decisions/0055-tau-identity-two-contracts.md)
```

- [ ] **Step 3: Add ADR-0055 row to the index table in `docs/decisions/README.md`**

The Index table rows are `| [NNNN](file.md) | Title | Status |`. Find the
`0054` row (last row of the table) and insert immediately after it:

```
| [0055](0055-tau-identity-two-contracts.md) | tau identity — compiler+engine between two contracts; CLI = reference host | Accepted |
```

Verify the 0054 row text first with `grep -n '0054' docs/decisions/README.md`; insert after whatever line that is. If 0054 is somehow absent from the table, insert after the highest-numbered row present and note it in the commit body.

- [ ] **Step 4: Run the "test" — book builds clean with the new page wired in**

```bash
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd ..
rm -rf docs/book
```

Expected: only `[INFO]` lines. A `[WARN] ... not found in SUMMARY` or a linkcheck `[ERROR]` means Step 2/3 wiring is wrong — fix before committing. Confirm no broken-link errors mentioning `0055`.

- [ ] **Step 5: Commit Story 8.1**

```bash
git add docs/decisions/0055-tau-identity-two-contracts.md docs/SUMMARY.md docs/decisions/README.md
git -c user.name="LEBOCQ Titouan" -c user.email="75916953+LEBOCQTitouan@users.noreply.github.com" \
  commit --no-verify -m "docs(adr): ADR-0055 tau identity — two contracts; CLI = reference host

Closes #375. tau's product = tau-runtime-core + two versioned contracts
(authoring/IR schema incl. root [allow]; generated WIT host world). The CLI
is the reference host, not the product. Public semver surface = the two
contracts + no_std ports; CLI verbs get a looser policy. Wired into
SUMMARY.md and the decisions README index.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Story 8.2 — Rewrite philosophy.md (per-target capability lowering + locked wedge)

**Files:**
- Modify: `docs/explanation/tau-philosophy.md` — Conviction 3 body (currently lines ~96–113) and the `## The wedge` section (currently lines ~370–390).

**Interfaces:**
- Consumes: ADR-0055 identity (Task 2) — the engine-first / per-target framing.

- [ ] **Step 1: Rewrite Conviction 3**

Replace the entire Conviction-3 block. OLD (exact current text):

```markdown
### 3. tau is *capability-safe by construction*; portability is the dividend

Every tool — native or contracted — declares its capabilities. The
**capability gate** is the single, uniform enforcement point: at the OS
boundary (landlock / seccomp / sandbox-exec / AppContainer) for native tools,
and at the contract boundary for MCP servers. `tau check` refuses to build a
workflow that requires enforcement a target can't provide. A workflow that
demands `strict` cannot ship as `passthrough` without explicit declaration.

This is the gap that MCP itself leaves open: MCP's "capabilities" are
protocol-feature negotiation; its authorization is OAuth-scoped remote access.
**Neither sandboxes a tool's filesystem, network, or exec at runtime.** tau
fills exactly that gap, and does so uniformly across native and contracted
tools.

Portability falls out of capability-correctness: if every tool's capability
shape is honored by the target, the artifact runs there. The target triple is
the contract.
```

NEW:

```markdown
### 3. tau is *capability-safe by construction*; portability is the dividend

Every tool — native or contracted — declares its capabilities **once**, in
the root `tau.toml` constitution. Capabilities are uniform in *declaration*,
not in *mechanism*: `tau build` **lowers the same declaration per target** to
whatever that target enforces with:

- **wasm (the primary target):** generated **WIT imports** + host config. A
  capability the workflow never declared produces no import; an un-imported
  host function is **unreachable by construction** — there is nothing to
  sandbox at runtime because the capability was never wired in. Enforcement is
  structural, not a runtime check.
- **host / native:** an OS sandbox — landlock / seccomp (Linux),
  sandbox-exec (macOS), AppContainer (Windows) — gates the declared
  capabilities at the process boundary.
- **bare-metal firmware:** **passthrough** — advisory only, and **honestly
  labeled** as such. The declaration is recorded and surfaced, but the chip
  provides no enforcement boundary; tau does not pretend otherwise.

`tau check` refuses to build a workflow that requires enforcement a target
can't provide. A workflow that demands `strict` cannot ship as `passthrough`
without an explicit declaration.

This is the gap that MCP itself leaves open: MCP's "capabilities" are
protocol-feature negotiation; its authorization is OAuth-scoped remote access.
**Neither sandboxes a tool's filesystem, network, or exec at runtime.** tau
fills exactly that gap, lowering one declaration to each target's native
mechanism.

Portability falls out of capability-correctness: if every tool's declared
capability shape can be lowered onto the target, the artifact runs there. The
target triple is the contract.
```

- [ ] **Step 2: Replace the `## The wedge` section with the locked text**

OLD (exact current text — the whole section body from the heading through the closing paragraph):

```markdown
## The wedge

The only unoccupied position in the landscape is:

> **A portable, capability-enforced agent + workflow artifact that nobody
> else produces.**

Three differentiators, ranked by defensibility:

1. **Per-tool capability enforcement** — fills the gap MCP explicitly leaves
   to the host. Concrete, demonstrable, immediately useful.
2. **A portable agent harness (MCP host / facilitator) on wasm and edge** —
   greenfield. Wasm MCP *servers* exist; portable hosts essentially don't.
3. **Compiling the harness all the way to embedded / firmware** — novel,
   highest-upside, riskiest. Acknowledged research bet.

Everything else (harness/inference split, delegated inference, BFF
credentials, server/edge/browser portability) is table stakes already
delivered by Vercel AI SDK, Cloudflare Agents, the OpenAI/Claude Agent SDKs,
or the Dapr stack. tau **integrates** those patterns; it does not claim
novelty on them.
```

NEW:

```markdown
## The wedge

The 2026 field has filled in around tau — Cloudflare a portable MCP host
(vendor-locked), wasmCloud wasm sandboxing (a mesh you operate), LangGraph a
durable agent graph (Python, no artifact), BAML a prompt compiler
(host-language glue), Temporal durable execution (your infra), esp-claw an
on-MCU agent (no sandbox). Each owns one slice. Nobody ships tau's
combination:

> declare what agents are *allowed* in a root constitution, author workflows
> beautifully in any language (generated, typed), and compile to one
> hardware-agnostic, capability-bounded component proven identical across
> local / edge / browser / embedded — with build-time enforcement and no
> runtime surprises.

The moat is the **combination + conformance + vendor-independence +
root-governed capability safety** — not novelty of any one piece.
```

- [ ] **Step 3: Run the "test" — book builds clean**

```bash
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd ..
rm -rf docs/book
```

Expected: only `[INFO]` lines; linkcheck clean. (philosophy.md's outbound links are unchanged by these edits, so any linkcheck error is unrelated — investigate, don't ignore.)

- [ ] **Step 4: Commit Story 8.2**

```bash
git add docs/explanation/tau-philosophy.md
git -c user.name="LEBOCQ Titouan" -c user.email="75916953+LEBOCQTitouan@users.noreply.github.com" \
  commit --no-verify -m "docs(philosophy): per-target capability lowering + locked wedge

Closes #376. Conviction 3 reframed: capabilities declared once, LOWERED PER
TARGET (wasm = generated WIT imports, unreachable by construction; host =
OS sandbox; firmware = advisory passthrough, honestly labeled). Drops the
inaccurate 'single uniform OS-boundary gate' framing. Wedge replaced with
the locked combination-play text.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Story 8.3 — Reframe ROADMAP.md (component-first; firmware as niche)

**Files:**
- Modify: `ROADMAP.md` — intro framing (top), Phase γ.5 firmware framing (~lines 619–653), NG5 durability note (~lines 808–817).

**Interfaces:**
- Consumes: ADR-0055 (Task 2) artifact framing.

- [ ] **Step 1: Add the component-first artifact framing to the intro**

OLD (exact, top of file):

```markdown
# Tau roadmap

This document tracks current direction, prior shipped work, and the
forward phasing under the canonical philosophy
[`docs/explanation/tau-philosophy.md`](docs/explanation/tau-philosophy.md).
```

NEW:

```markdown
# Tau roadmap

This document tracks current direction, prior shipped work, and the
forward phasing under the canonical philosophy
[`docs/explanation/tau-philosophy.md`](docs/explanation/tau-philosophy.md).

**What tau ships is a component, not an application.** The product is
`tau-runtime-core` (the engine) plus two versioned public contracts — the
authoring/IR schema (including the root `[allow]` governance section) and the
generated WIT host world — per
[ADR-0055](docs/decisions/0055-tau-identity-two-contracts.md). The `tau` CLI
is the **reference host** that exercises those contracts, held to the highest
quality bar but architecturally one host among peers (edge / browser /
embedded are its equals). The phases below are framed engine-first on that
basis.
```

- [ ] **Step 2: Reframe Phase γ.5 — firmware as a gated Reserved niche, wasm-on-MCU as the on-chip spine**

OLD (exact — the paragraph after the γ table):

```markdown
**γ.5 is two sub-projects, not one.** The embassy shell (γ.5a) is the
expensive part — async runtime swap, `no_std` dependency tree audit,
plumbing for `reqwless`/`embedded-tls`. It must land before any actual
firmware target (γ.5b) can produce a real artifact. Sized realistically:
γ.5a ~6–8 weeks (one implementer with embassy/HAL familiarity);
γ.5b ~3–4 weeks per CPU triple after γ.5a.

The `wasi-p2-component` target on MCU (the "wasm component on
microcontroller") is **deferred** to a future phase until the runtime
ecosystem catches up. Tracked via Framing C″.
```

NEW:

```markdown
**Embedded = tau-as-component is the goal; tau-as-firmware is a gated
niche.** The product on a microcontroller is tau shipped as a **component**
inside someone's product firmware — a wasm guest (γ.4) or a `no_std`
library — delegating inference to a gateway like every other target.
**wasm-on-MCU is the on-chip spine** (γ.4 today on WAMR Preview-1; the
`wasi-p2-component` target graduates when the runtime ecosystem ships the
Component Model on MCU — tracked via Framing C″).

**γ.5 (tau-as-firmware — embassy + WAMR *owning the whole chip*) is demoted
to a Reserved NICHE**, gated on BOTH: (1) a named gateway-less buyer who
genuinely cannot delegate inference, and (2) WAMR shipping the Component
Model on MCU. Absent both, γ.5 is not scheduled. When it is unblocked it is
still two sub-projects: the embassy shell (γ.5a) is the expensive part —
async runtime swap, `no_std` dependency-tree audit, `reqwless`/`embedded-tls`
plumbing — and must land before any firmware target (γ.5b) produces a real
artifact (γ.5a ~6–8 weeks; γ.5b ~3–4 weeks per CPU triple).
```

- [ ] **Step 3: Add the durability note to NG5**

OLD (exact — the NG5 bullet):

```markdown
- **NG5.** Tau is not a general-purpose workflow engine. *(Clarification:
  tau executes workflow IR with capability-safe portability as its
  defining property; it does not compete with general orchestrators
  like Temporal/n8n on their breadth. Durability — when and whether to
  re-run — is delegated to the host orchestrator; tau guarantees the
  compiled bundle is a safe-to-retry reentrant unit. See
  [Run tau under a durable orchestrator](docs/how-to/run-tau-under-a-durable-orchestrator.md).)*
```

NEW:

```markdown
- **NG5.** Tau is not a general-purpose workflow engine. *(Clarification:
  tau executes workflow IR with capability-safe portability as its
  defining property; it does not compete with general orchestrators
  like Temporal/n8n on their breadth. Durability is **delegated-canonical**:
  when and whether to re-run is the host orchestrator's job, and tau
  guarantees the compiled bundle is a safe-to-retry reentrant unit.
  **Opt-in, host-sized durability tiers** layer on top of that reentrancy —
  A-minimal turn-level checkpoint/resume shipped 2026-06-10 (PR #373;
  [ADR-0053](docs/decisions/0053-turn-level-checkpoint-resume.md)), with a
  gated A-full tier above it — none of which changes the delegated-canonical
  model. See
  [Run tau under a durable orchestrator](docs/how-to/run-tau-under-a-durable-orchestrator.md).)*
```

- [ ] **Step 4: Run the "test" — book builds clean, no broken internal links**

```bash
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd ..
rm -rf docs/book
```

Expected: only `[INFO]` lines. The new intro link to `0055-...` and the NG5 link to `0053-...` are both real files — linkcheck must pass. A `0055` or `0053` link error means a typo in Step 1/3.

NOTE: `ROADMAP.md` lives at the repo root; in the book it renders via the `- [Roadmap](../ROADMAP.md)` entry in SUMMARY.md. Its links are written relative to repo root (`docs/decisions/...`), which is how the existing `docs/explanation/tau-philosophy.md` link in the same file is written — match that style exactly (the existing intro link is the template).

- [ ] **Step 5: Commit Story 8.3**

```bash
git add ROADMAP.md
git -c user.name="LEBOCQ Titouan" -c user.email="75916953+LEBOCQTitouan@users.noreply.github.com" \
  commit --no-verify -m "docs(roadmap): component-first frame; firmware as gated niche

Closes #377. Artifact framing = a component + two contracts (per ADR-0055),
not an application/CLI. Embedded = tau-as-component (wasm guest or no_std
lib); tau-as-firmware (embassy+WAMR owning the chip) demoted to a Reserved
niche gated on a named gateway-less buyer AND WAMR shipping the Component
Model. wasm-on-MCU is the on-chip spine. NG5 notes durability is
delegated-canonical + opt-in host-sized tiers (A-minimal shipped #373).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Open the PR and enroll in the merge queue

**Files:** none (git/gh only).

- [ ] **Step 1: Final clean-build gate across all three stories**

```bash
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd ..
rm -rf docs/book
git status   # confirm docs/book is gone and tree is clean
```

Expected: `[INFO]`-only build; `git status` shows no untracked `docs/book/`.

- [ ] **Step 2: Push**

```bash
git push -u origin feat/epic-8-identity
```

Docs-only → no deep gate; plain push.

- [ ] **Step 3: Open the PR against main**

```bash
gh pr create --base main --title "docs(epic-8): identity + positioning — two contracts, per-target lowering, component-first" --body "$(cat <<'EOF'
EPIC 8 — Identity + positioning (constitutional). Writes the 2026-06-20
vision-reframe decisions into committed docs so in-flight parallel work
(durability #373, β.7.5 #369/#372) stays aligned.

- **8.1 (#375)** ADR-0055: tau = compiler+engine between two contracts
  (authoring/IR schema incl. root `[allow]`; generated WIT host world);
  the CLI is the reference host, not the product. Public semver surface =
  the two contracts + no_std ports; CLI verbs get a looser policy. Wired
  into SUMMARY.md + decisions README index.
- **8.2 (#376)** philosophy.md: Conviction 3 → per-target capability
  lowering (wasm = generated WIT imports, unreachable by construction;
  host = OS sandbox; firmware = advisory passthrough). Wedge → locked
  combination-play text.
- **8.3 (#377)** ROADMAP.md: component-first artifact framing; embedded =
  tau-as-component; tau-as-firmware demoted to a gated Reserved niche;
  durability noted as delegated-canonical + opt-in host-sized tiers
  (A-minimal shipped #373).

Docs-only. mdbook build + linkcheck clean locally. Closes #375, #376, #377.
EOF
)"
```

- [ ] **Step 4: Enroll in the merge queue**

```bash
PR=$(gh pr view --json number -q .number)
gh pr merge "$PR" --auto
```

(No `--delete-branch`, no strategy flag — the merge queue owns the strategy.)

- [ ] **Step 5: Report PR number + URL to the user.**

---

## Self-Review

**1. Spec coverage** (against the EPIC 8 handoff + vision-roadmap.md):
- 8.1 ADR with the exact DECISION bullets (engine + two contracts; CLI=reference host; semver surface; highest-bar CLI) → Task 2, ADR body. ✓
- 8.2 (a) Conviction 3 per-target lowering with all three targets + "remove uniform OS-boundary gate" → Task 3 Step 1. ✓
- 8.2 (b) wedge replaced with the verbatim locked text → Task 3 Step 2 (text copied verbatim from the handoff lines 44–52). ✓
- 8.3 component+two-contracts artifact framing → Task 4 Step 1; firmware→Reserved niche gated on two conditions → Step 2; durability delegated-canonical + tiers (A-minimal #373) → Step 3. ✓
- Epic DoD: ADR Accepted; philosophy + ROADMAP match vision; mdbook+linkcheck green → Tasks 2–5. ✓

**2. Placeholder scan:** none — all ADR text, all OLD/NEW edit blocks, and all commands are concrete and verbatim.

**3. Type/anchor consistency:** ADR filename `0055-tau-identity-two-contracts.md` is identical in the file create, the SUMMARY line, the README row, and the ROADMAP intro link. ADR-0053 link in NG5 matches the real filename `0053-turn-level-checkpoint-resume.md`. Wedge NEW text matches the locked text exactly.

**Deviation from handoff (intentional, justified):** the handoff's generic note says "ADRs are typically NOT book pages — don't add them to SUMMARY". This repo contradicts that: ADR-0001…0054 (except 0039) are ALL in SUMMARY.md and the README index. Following the repo's actual convention, ADR-0055 is added to both (Task 2 Steps 2–3). Not doing so would make 0055 the lone post-0008 ADR missing from the index.
