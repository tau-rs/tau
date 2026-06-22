# EPIC 2.1 — Contract Versioning Stability Surface (ADR-0056) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land one ADR (`0056`) that makes tau's two-contract semver stability surface normative and precise — each published contract versioned by its own native mechanism, ports riding Rust crate semver.

**Architecture:** Docs-only story. The deliverable is `docs/decisions/0056-contract-versioning-stability-surface.md` plus its two index wirings (`docs/SUMMARY.md`, `docs/decisions/README.md`). No Rust code, no tests beyond the mdbook build + linkcheck gate. The decision (Approach D) is fully specified in the spec; this plan transcribes it into ADR form and proves the book builds clean.

**Tech Stack:** Markdown; mdbook 0.4 + mdbook-linkcheck 0.7 + mdbook-mermaid ^0.14 (binaries at `~/.cargo/bin`, not on PATH).

## Global Constraints

- Spec of record: `docs/superpowers/specs/2026-06-22-epic-2-1-contract-versioning-design.md`. Every requirement below traces to it.
- ADR number is **0056** (0055 is the current highest). Date **2026-06-22**. Status **Accepted**.
- ADR-0055 is the **premise, NOT superseded** — do not change 0055's status or add a "Superseded by" line to it.
- This is **docs-only** — no Rust files, no cargo invocation. The only build gate is mdbook + linkcheck.
- Every new doc page MUST be listed in `docs/SUMMARY.md` or mdbook silently skips it (DOCS RULES).
- mdbook binaries are not on PATH: build with `cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build`. Remove `docs/book/` after verifying (`rm -rf docs/book`).
- Commit identity override per CLAUDE.md: `git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit --no-verify`. `--no-verify` is acceptable for docs-only changes.
- Branch: `epic-2-lock-abi-contracts` (already checked out). Do not rename it.

---

### Task 1: Author ADR-0056 and wire it into the book

**Files:**
- Create: `docs/decisions/0056-contract-versioning-stability-surface.md`
- Modify: `docs/SUMMARY.md` (insert one line after the ADR-0055 line, currently line 128)
- Modify: `docs/decisions/README.md` (insert one row in the Index table after the ADR-0055 row)

**Interfaces:**
- Consumes: the spec (`docs/superpowers/specs/2026-06-22-epic-2-1-contract-versioning-design.md`) — the Decision/Consequences/Alternatives sections transcribe directly.
- Produces: a merged ADR satisfying issue #383 acceptance ("ADR merged"). No code symbols.

- [ ] **Step 1: Create the ADR file**

Create `docs/decisions/0056-contract-versioning-stability-surface.md` with exactly this content:

```markdown
# ADR-0056: the two contracts are the semver stability surface

**Status:** Accepted
**Date:** 2026-06-22
**Deciders:** tau core

## Context

[ADR-0055](0055-tau-identity-two-contracts.md) fixed tau's *identity*: the
product is `tau-runtime-core` (the engine) plus two versioned public contracts —
the authoring / IR schema and the WIT host world — with the `tau` CLI as the
reference host. But 0055 fixed the *identity*, not the *stability mechanics*, and
its decision text carries a latent ambiguity. It says the public surface is "the
two contracts **+ the no_std ports API**" (which reads like three surfaces),
while two lines later it says "versioning + conformance attach to **the two
contracts**" and its alternatives section explicitly rejects a three-contract
model ("two contracts, not three").

This ADR makes the versioning scheme **normative and precise** so that the rest
of EPIC 2 (schema publish, WIT generation + drift test, operator policy) and the
in-flight parallel work (durability #373, β.7.5) cannot diverge from one agreed
contract shape. It is constitutional: it lands the *decision*; the operator
*policy* (deprecation windows, migration how-tos, the `cargo-semver-checks`
workflow) lands in Story 2.4.

Three facts constrain the decision:

- The IR already carries a version: `tau-ir::module` has an `ir_format` semver
  field (currently v2.2.0), separate from `tau_version`; both feed the IR content
  hash.
- `tau-ports` is a broad no_std Rust trait surface; the WIT host world is only
  the minimal 3-function subset, **generated from** ports (Story 2.3) — never
  hand-maintained.
- The three surfaces have **different consumers** and a **containment**
  relationship — `WIT ⊊ ports`, WIT generated from ports — so every WIT-breaking
  change implies a ports-breaking change, but not vice-versa (ports-internal
  churn in `storage`/`orchestration` need not touch the 3 host functions):

  | surface | consumer | churn tolerance |
  |---|---|---|
  | IR JSON schema | frontend / SDK authors | track `ir_format` |
  | WIT host world | wasm-guest embedders, any language | want zero false-alarm bumps |
  | ports API | no_std Rust embedders | see the full surface, correctly |

## Decision

**The public ABI is exactly two published, conformance-kitted contracts, each
versioned by its own native mechanism** — the "one source → no drift" principle
applied to versioning itself. No bespoke version registry, no hand-maintained
version constant:

1. **Authoring contract** — the IR JSON schema, including the root `[allow]`
   governance section — versioned by the existing `ir_format` field.
2. **Embedding contract** — versioned by the **WIT package version**,
   `package tau:host@x.y.z;`.

**The no_std ports API is the Rust-native binding of the embedding contract**,
versioned by `tau-ports` **crate semver** and enforced by `cargo-semver-checks`
in CI. It is *not* a third published contract. This is the precise meaning of
0055's "+ the no_std ports API": ports' stability is delivered by Rust's own
semver mechanism, so it is covered without being a third language-neutral ABI.

**Containment invariant:** `WIT ⊊ ports`; WIT is generated from ports (Story
2.3), so every WIT-breaking change implies a ports-breaking change. The Story 2.3
drift test enforces this.

**Breaking-change definition per surface** (the normative semver semantics):

- **IR `ir_format`** — *major:* remove/rename/retype a field, or change the
  canonical encoding such that an older runtime cannot load the IR or a
  previously-valid IR becomes invalid; *minor:* additive (new optional field /
  forward-compatible variant); *patch:* non-semantic / doc only.
- **WIT `tau:host`** — *major:* remove, re-signature, or re-semantic any of the
  3 host functions; *minor:* additive host function or additive optional
  parameter; *patch:* doc only.
- **`tau-ports` crate** — standard Rust semver, enforced by `cargo-semver-checks`.

**CLI verbs are explicitly OUT of this surface.** They evolve under a looser
compatibility policy documented with the CLI, not governed by the contract
semver (restates 0055).

### Relationship to ADR-0055

0055 is the **premise** of this ADR, not superseded. This ADR resolves 0055's
"+ the no_std ports API" phrasing (the ports API is the Rust binding of the
embedding contract, versioned by crate semver — point 2 above) and makes 0055's
"versioning attaches to the two contracts" operational. 0055's status is
unchanged.

## Consequences

- **New CI obligation:** a `cargo-semver-checks` lane for `tau-ports` (does not
  exist today). Named here; wired by Stories 2.3/2.4.
- **Wires the rest of EPIC 2:** Story 2.2 surfaces the IR JSON Schema `$id` /
  version from `ir_format`; Story 2.3 emits WIT carrying `@x.y.z` plus the
  containment drift test; Story 2.4 owns the operator policy.
- **`tau-ports` versioning wrinkle:** `tau-ports` likely shares the workspace
  version today (`version.workspace = true`). To give it an independent semver
  signal it may need to break out of the workspace version, or we accept a coarse
  workspace version and rely on `cargo-semver-checks` to flag inadequate bumps.
  Story 2.4 resolves this; this ADR only names the constraint.
- **Neutral:** no code in this ADR — the acceptance gate is "ADR merged" with a
  clean mdbook build + linkcheck.

## Alternatives considered

- **One bespoke `host-abi` version covering WIT ⊕ ports together.** Rejected: a
  ports-internal break (e.g. in `storage`) that never touches the 3 host
  functions would still bump the exact number a wasm-guest author tracks —
  false-alarm major bumps land on tau's flagship "embed in any language" persona.
  A hand-maintained `HOST_ABI_VERSION` constant also violates one-source-no-drift.
- **Three independent published version lines** (`ir_format` + a WIT-world
  version + a ports-crate version, each published). Rejected: reintroduces the
  three-surface ceremony 0055 explicitly killed, and creates WIT↔ports version
  drift risk — two hand-coordinated numbers for one generated relationship.
- **One unified `tau-abi` semver covering IR + WIT + ports together.** Rejected:
  couples authoring and embedding — an IR-only additive change forces an
  embedding-version bump and churns embedders who depend on only one contract,
  and vice-versa.
```

- [ ] **Step 2: Add the SUMMARY.md entry**

In `docs/SUMMARY.md`, immediately after the ADR-0055 line (currently line 128):

```
- [ADR-0055 — tau identity: two contracts; CLI = reference host](decisions/0055-tau-identity-two-contracts.md)
```

insert:

```
- [ADR-0056 — The two contracts are the semver stability surface](decisions/0056-contract-versioning-stability-surface.md)
```

- [ ] **Step 3: Add the README.md index row**

In `docs/decisions/README.md`, find the Index-table row for ADR-0055 and insert immediately after it:

```
| [0056](0056-contract-versioning-stability-surface.md) | The two contracts are the semver stability surface | Accepted |
```

(If no 0055 row exists in the table — the index is not always complete — append the 0056 row as the last data row of the `| ADR | Title | Status |` table.)

- [ ] **Step 4: Build the book and confirm it is clean**

Run:

```bash
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build
```

Expected: only `[INFO]` lines, no `[WARN]`/`[ERROR]`, exit 0. Linkcheck (`warning-policy = "error"`) passes — confirming the new page is reachable from SUMMARY.md and has no broken intra-doc links (e.g. the `0055-tau-identity-two-contracts.md` relative link resolves).

If the mdbook binaries are missing, STOP and ask the user to install them (`cargo install` of agent-chosen packages is denied per CLAUDE.md):
`cargo install mdbook --locked --version ^0.4 && cargo install mdbook-linkcheck --locked --version ^0.7`

- [ ] **Step 5: Clean the build output**

Run:

```bash
rm -rf docs/book
```

(`docs/book/` is gitignored but should not linger.)

- [ ] **Step 6: Commit**

```bash
git add docs/decisions/0056-contract-versioning-stability-surface.md docs/SUMMARY.md docs/decisions/README.md
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "docs(epic-2.1): ADR-0056 — two contracts are the semver stability surface

Each published contract versioned by its own native mechanism (IR ir_format,
WIT package version); ports API rides tau-ports crate semver via
cargo-semver-checks. Premise = ADR-0055 (not superseded). Closes #383."
```

Expected: one commit, 3 files changed.

---

## Self-Review

**Spec coverage:** every spec section maps to Task 1 — Decision points 1–6 → ADR §Decision + §Relationship; Consequences → ADR §Consequences; Alternatives A/B/C → ADR §Alternatives; "no code in 2.1" → docs-only plan; SUMMARY/README wiring → Steps 2–3; mdbook+linkcheck gate → Step 4. No gaps.

**Placeholder scan:** the full ADR body is inlined verbatim in Step 1 — no TBD/TODO. Step 3 has a conditional fallback (table may lack a 0055 row) with an explicit instruction, not a placeholder.

**Type consistency:** docs-only — no code symbols to reconcile. Cross-references (`0055-tau-identity-two-contracts.md`, `ir_format`, `tau:host`, `cargo-semver-checks`) match the spec exactly.
