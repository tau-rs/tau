# D3: Sound Capability Subset + `meet` — Design

**Date:** 2026-07-19
**Status:** Approved (brainstorm) — pending implementation plan
**Scope:** Prerequisite for D1-C (runtime capability attenuation). Not D1-C itself.

## Problem

`crates/tau-pkg/src/capability_override/glob_subset.rs` decides glob subset by
**bounded sampling** (`MAX_SAMPLES = 64`, fixed seed expansion for `*`/`**`).
This is provably unsound. Two witnesses:

1. `is_glob_subset("/proj/*", "/proj/seed*") == true` — `*` expands to the fixed
   seed `"seed"`, and `/proj/seed` matches `/proj/seed*`; but `/proj/xyz`
   (admitted by the child) is **not** admitted by the parent.
2. No path normalization: `/proj/../etc/**` passes the prefix check against
   `/proj/**` (it starts with `/proj/`) yet escapes to `/etc`.

The Story 1.3 "reusable subset primitive" (`subset.rs`,
`capability_set_subset`) is layered directly on top of this sampler
(`use super::glob_subset::is_glob_subset_set`), so it inherits the
unsoundness. Stories 1.4 / 1.5 (`tau check` governance) consume that
predicate.

A runtime clamp built on an unsound `meet` would launder unsound grants into
"enforced" ones — worse than no clamp. D3 replaces the sampler with a sound,
`no_std`, structurally-decidable subset + `meet`, as the single source of
truth for both `tau check` (host, std) and the future runtime clamp (kernel,
no_std).

## Decisions (from brainstorm)

- **D3-1 Grammar = G2.** Decide subset/meet analytically over a restricted glob
  grammar; everything outside it fails closed. G2 covers 292/293 of the
  star-bearing patterns in the current tree plus brace alternations.
- **D3-2 Single source of truth.** The sound primitive lands in `tau-domain`
  (`no_std`). `tau-pkg`'s `glob_subset.rs` sampler is **deleted**; `subset.rs`
  delegates to the domain primitive. Some sampling-era test admissions flip to
  fail-closed — that is the fix, not a regression.
- **D3-3 Free functions on `&[Capability]`.** No `CapabilitySet` newtype; slices
  are already the lingua franca at every call site. The handoff's
  `impl CapabilitySet { fn meet }` sketch was illustrative.

## The reduction

Only two capability fields are true globs: **fs paths**
(`FsCapability::{Read,Write,Exec}.paths`) and **net hosts**
(`NetCapability::Http.hosts`). The rest — `commands`, `allowed_kinds`,
`allowed_skills`, `methods`, `mode` — are literal token sets: subset = exact
membership, `meet` = set intersection, both trivially sound. So D3's soundness
work is entirely the path/host glob analyzer + lexical path normalization.

## 1. Location & API

New module `crates/tau-domain/src/package/capability/lattice.rs` (`no_std` +
`alloc`):

```rust
/// `child ⊆ parent` over full capability sets, matched by kind
/// (`Custom` by name). Returns the first violation. Sound: a kind or pattern
/// outside the decidable grammar is treated as *not* a subset (deny-by-default).
pub fn capability_subset(
    child: &[Capability],
    parent: &[Capability],
) -> Result<(), CeilingViolation>;

/// Greatest lower bound of two capability sets. Total on the G2 grammar.
/// Guarantees: `meet(a,b) ⊆ a` and `meet(a,b) ⊆ b`; idempotent, commutative;
/// and the lattice law `capability_subset(a,b).is_ok() ⟺ meet(a,b) == canon(a)`.
pub fn meet(a: &[Capability], b: &[Capability]) -> Vec<Capability>;
```

`CeilingViolation` moves from `tau-pkg` into `tau-domain` (String fields,
alloc-only, `#[non_exhaustive]`). `tau-pkg`'s `capability_override/subset.rs`
becomes a thin re-export (`capability_set_subset` kept as an alias delegating to
`tau_domain::…::capability_subset`) so Stories 1.3/1.4/1.5 call sites do not
churn. `glob_subset.rs` is deleted.

## 2. G2 pattern grammar (fs paths)

A pattern normalizes to a `/`-split segment list over the alphabet
**{ `literal`, `*`, `**` }**:

- `literal` — an exact path component.
- `*` — **exactly one** component (non-recursive; does not cross `/`).
- `**` — **trailing segment only**; matches any suffix including the empty
  suffix.
- `{a,b,c}` — brace alternation; expanded to arms before analysis (each arm is
  itself a G2 fragment).

Outside G2 → **not in grammar → fail-closed** (never a subset; contributes ⊥ to
`meet`). Outside-G2 forms: intra-segment wildcard (`foo*`, `*.log`), `?`,
character classes `[...]`, **middle** `**` (`/foo/**/bar.txt`), non-absolute
paths, nested braces that don't reduce.

### Normalization (lexical; no filesystem access — required for no_std)

1. Require absolute (leading `/`); relative → invalid → fail-closed.
2. Split on `/`; drop empty segments (collapse `//`) and `.` segments.
3. `..` folds by popping the preceding **literal** segment. `..` that escapes
   the root, or follows a `*` / `**` (unresolvable statically), → invalid →
   fail-closed.

Both witnesses die here:
- `is_glob_subset("/proj/*","/proj/seed*")` — parent `seed*` is intra-segment →
  outside G2 → **false**.
- `/proj/../etc/**` ⊆ `/proj/**` — child normalizes to `/etc/**`; head
  `etc ≠ proj` → **false**.

## 3. Subset (structural, exact)

`subset(C, P)` on normalized, brace-expanded segment lists:

| P head        | C head            | result                    |
|---------------|-------------------|---------------------------|
| `**` (last)   | anything          | **true** (⊤ suffix)       |
| `*`           | `literal` or `*`  | consume both, recurse     |
| `*`           | `**`              | false                     |
| `literal L`   | `literal L` (eq)  | consume both, recurse     |
| `literal`     | anything else     | false                     |
| P empty       | C empty           | true                      |
| P empty       | C non-empty       | false                     |
| C empty       | P == `[**]`       | true                      |
| C empty       | P non-empty (≠`**`)| false                    |

For a child set vs parent set: every child pattern must be a subset of **some**
parent pattern (matching the existing `is_glob_subset_set` contract). A child
capability kind with no matching parent kind is a violation (ceiling ∅,
deny-by-default). `Custom` matches by `name`. Unknown future variants →
fail-closed.

Worked checks:
- `/proj` ⊆ `/proj/**` → recurse to `C=[], P=[**]` → true.
- `/proj/**` ⊄ `/proj/src/**` → at `src` (literal) vs `**` (C head) → false.
- `/etc/**` ⊄ `/proj/src/**` → head mismatch → false.

## 4. `meet` (exact language intersection, then canonicalize)

Group by kind/verb (`fs.read`, `fs.write`, `fs.exec`, `net.http`,
`process.spawn`, `agent.spawn`, `skill.spawn`, `tasklist`, `plan`,
`custom`-by-name). A kind present in only one operand is dropped (∅).

- **Literal-token fields** (`commands`, `allowed_kinds`, `allowed_skills`,
  `methods`, `mode`): `meet` = set intersection.
- **Glob fields:** `meet(PA, PB) = ⋃_{pa∈PA, pb∈PB} intersect(pa, pb)`, where
  `intersect` on segment lists is:

| heads                       | result                                  |
|-----------------------------|-----------------------------------------|
| `**` on either side         | the *other* side's remaining tail (⊤)   |
| `*` , `*`                   | `*`, recurse                            |
| `*` , `literal L`           | `L`, recurse                            |
| `literal L` , `literal L`   | `L`, recurse                            |
| `literal` ≠ `literal`       | ∅                                       |
| one side exhausted, other head `literal` | ∅                          |
| one side exhausted, other head `**`      | exact path so far (non-∅)  |

This computes the **exact** intersection language — e.g.
`meet(/a/**, /*/b/**) = /a/b/**`, `meet(/a/*, /a/**) = /a/*` — not merely "the
smaller operand", which is why a real algorithm is required.

**Canonical form** (`canon`) makes the lattice law hold as structural equality:
normalize each pattern, dedup, and **absorb** (drop any pattern that is a subset
of another pattern in the same set). Set equality is multiset equality of
canonical patterns.

**Lattice law** `subset(A,B).is_ok() ⟺ meet(A,B) == canon(A)`:
- (⇒) If `A ⊆ B`, then `L(A) ∩ L(B) = L(A)`, so `canon(meet) = canon(A)`.
- (⇐) If `meet(A,B) == canon(A)`, then `L(A) ⊆ L(B)`.

This is the subtlest claim in the design; property tests (§6) guard it, and it
relies on absorb-based `canon` being a true normal form on G2.

## 5. Net hosts (small separate sub-grammar)

Deliberately conservative: a host is `exact` or `*.suffix`. `subset`/`intersect`
by suffix containment (`a.example.com ⊆ *.example.com`;
`*.a.example.com ⊆ *.example.com`). Anything else (embedded `*`, multiple `*`) →
fail-closed. This is a smaller grammar than paths and is documented as such.

## 6. Tests

- **Property tests** (proptest; runs under the std test harness even though the
  crate is `no_std`):
  - `meet(a,b) ⊆ a` and `meet(a,b) ⊆ b`.
  - idempotent: `meet(a,a) == canon(a)`.
  - commutative: `meet(a,b) == meet(b,a)`.
  - lattice law: `subset(a,b).is_ok() ⟺ meet(a,b) == canon(a)`.
- **Witness regressions:** both audit witnesses (§Problem) fail-closed /
  normalize correctly.
- **Migrated units:** sound cases from `glob_subset` tests survive; sampling-era
  admissions (`question_mark_sample_fallback_admits`,
  `character_class_sample_fallback_admits`, nested-brace fall-through) **flip to
  fail-closed** and are re-documented as the intended behavior.
- **no_std gate:** `cargo check -p tau-domain --no-default-features` includes the
  new module.
- **Delegation:** existing `tau-pkg` / `tau-cli` (1.3/1.4/1.5) tests still pass
  against the re-exported predicate; any that asserted a sampling-era admission
  are updated to the sound verdict, called out individually in the plan.

## 7. Scope boundary

**In:** the sound subset + `meet` primitive in `tau-domain`, deletion of the
sampler, `tau-pkg` delegation.

**Out:**
- **D1-C runtime clamp** — separate handoff; consumes `meet`.
- **D5 subflow `cap_subset`** — composes as `meet(parent_effective, cap_subset)`;
  the API is shaped so it reuses this directly.
- **EPIC 3.4 wasm in-guest gate removal** — a D1-C / ADR concern. D3 is a pure,
  `no_std` primitive consumed identically by native and wasm, so it does not
  touch that fork.
- **Grammar broadening to G3** (middle `**`) or richer host grammars — additive
  later; outside-grammar patterns already fail-closed soundly.

## Risk flagged to the user

Making `tau check` ride the sound predicate may turn a currently-green check on
some existing config into a ceiling violation, where the pattern is genuinely
outside the sound grammar. That is D3 doing its job, but it is a behavior change
on merged EPIC 1 code. The plan enumerates each flipped test.
