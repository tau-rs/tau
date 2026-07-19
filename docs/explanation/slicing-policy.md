# Slicing policy — how we cut roadmap work

tau's backlog is cut into **vertical slices**: each roadmap item, when it merges,
lets a user newly *do* something. This page is the standing rule for what counts
as a slice, why, and the worked examples that show the difference. It exists
because we have repeatedly shipped horizontal layers (a data model, an
interpreter) that no user could reach — value on paper, not in hand.

## The rules

1. **Every slice states its user-observable delta before work starts.** In one
   sentence: *"when this merges, a user can newly ___."* No delta sentence → it
   is not a slice, it is a task inside one. Put the sentence in the roadmap story
   and in the GitHub issue body.

2. **Producer-without-consumer merges must name their consumer.** If a PR ships a
   producer (a data-model variant, an interpreter path, an ABI) with no authoring
   surface that reaches it, the PR body must name, own, and date the consumer
   story that redeems it. A producer merge is allowed — but never anonymous.

3. **Enabler slices are legal but time-boxed.** EPIC-0-style enablers (no direct
   user delta — e.g. de-std the run loop) must name the first *vertical* slice
   that redeems them, and that slice must start within **4 weeks**. Otherwise the
   enabler is re-justified in the roadmap or reverted from it.

4. **Conformance/parity is part of each slice's DoD, never a trailing phase.** A
   "conformance for all constructs" story is an anti-pattern — it lets the
   constructs merge unverified and pushes the proof to a phase that may never
   start. Bake it into every slice's Definition of Done instead.

5. **Cross-epic north star.** There is ONE executable demo fixture, extended as
   epics land: an `[allow]`-governed workflow using Branch + Loop, built to both
   the dev and wasm targets and run in CI (extends the β fan-monitor pattern). It
   is tracked as its own issue and wired into CI when EPIC 4.2a merges. Each new
   construct/target extends this fixture rather than adding a throwaway one.

## Worked examples

The contrast is the whole point — future readers need to see a mis-cut next to a
clean one.

### Producer-without-consumer (what to avoid, or at least to name)

- **EPIC 4.1 (#444).** Added `StepRun::Branch/Parallel/Loop/Suspend` to the IR
  data model + typecheck. Byte-stable when unused — clean engineering — but **no
  `tau.toml` syntax produced any of them.** A user gained nothing on merge.
- **EPIC 4.2-interpreter (#454).** Made the interpreter *execute* those blocks
  (recursive `run_steps`, flat-global nested scope, bounded fork-join). The
  engine could now run a Branch — but the authoring surface still could not
  express one, so **a user still gained nothing on merge.** Two horizontal layers
  in a row, each real work, neither a slice. This is exactly what rule 2 now
  forces us to name up front. The redeeming consumers are the vertical
  4.2a/4.2b/4.2c slices (Branch/Parallel/Loop *end-to-end*) in
  [`../superpowers/plans/vision-roadmap.md`](../superpowers/plans/vision-roadmap.md).

### Correct cuts (what to copy)

- **EPIC 1.4 (#436).** "`tau check` fails if any cap ⊄ `[allow]`." Delta on merge:
  a user can newly catch an over-reaching workflow at build time. Syntax,
  enforcement, error, and test all in one PR.
- **EPIC 6.1 (#425).** Intent-knob `durable="survive-restarts"` with
  `tau check --target X` printing the resolved durability. Delta on merge: a user
  can newly declare a durability intent and *see* how the host resolves it — no
  hidden behavior. A whole vertical: knob → resolution → observable output.

## Why this is written down

The re-cut of EPIC 4 (D13-C, 2026-07-19) turned 4.2–4.6 from layer-ordered phases
into per-construct vertical slices precisely because 4.1 and 4.2-interpreter had
shipped as producer-without-consumer merges. This page is the rule that makes the
next re-cut unnecessary.
