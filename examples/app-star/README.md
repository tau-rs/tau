# app-star — vision fixture for posture B (tau in the project)

> **STATUS: VISION FIXTURE — target state, not yet runnable.** Nothing
> here builds today; the machinery lands per the wave table in the
> worked-examples spec §6. This tree exists so the vision can be
> **verified file-by-file before a line of it is implemented**, and so the
> path doesn't move when it becomes the real, CI-green witness.
> Spec: [`worked-examples §2`](../../docs/superpowers/specs/2026-09-06-tau-as-code-worked-examples.md).
> Surface names are illustrative until the slice ADRs; the *beats* below
> are what you are verifying.

**The story:** *Ticketflow* — four engineers, an existing Fastify SaaS,
one wish: agentic ticket triage that lives next to the route that calls
it, type-checks against their code, deploys with their app, and can never
grow a capability their reviewer didn't see.

## Walkthrough — verify each checkpoint

Walk these in order; each names what to open, the moment it embodies, and
the question to answer. A "no" on any checkpoint is a vision correction —
record it (see the end) and the stone gets recut before ratification.

**VB-1 · The app repo IS the tau project.** Open [`tau.toml`](tau.toml),
[`agents/triage.md`](agents/triage.md), [`models/default.toml`](models/default.toml).
The constitution and vocabulary sit beside `package.json`, still pure
TOML/dirs — posture B is packaging, not a new mode. *(B1; invariant 3)*
— Verify: you accept that even in-app, `[allow]`/agents/models never move
into TS.

**VB-2 · Explicit collection.** Open [`src/tau.entry.ts`](src/tau.entry.ts).
The synth sandbox evaluates only this import graph; an un-imported
`*.tau.ts` doesn't exist. *(B2; invariant 1)* — Verify: explicit imports
over magic suffix-scanning is the convention you want.

**VB-3 · One symbol, two projections.** Open
[`src/triage/triage.tau.ts`](src/triage/triage.tau.ts) then
[`src/routes/tickets.ts`](src/routes/tickets.ts). The definition is
harvested at build time; the same import is a typed run-handle at app
runtime; the body never executes in the app process. *(B3, B4;
invariants 2, 4, 6)* — Verify: this is the co-location DX you meant by
"in-code pipeline definitions", and you accept it is sugar over the flow
lane — build-time, never runtime assembly.

**VB-4 · The typed bridge.** Open [`tau.gen.ts`](tau.gen.ts). Generated,
committed, hash-stamped; the app type-checks against the vocabulary
through it. *(B6; invariant 4)* — Verify: "generated + committed + stale
= loud error" is the drift discipline you want.

**VB-5 · Widening is loud.** Read
[`transcripts/01-plan-widen.txt`](transcripts/01-plan-widen.txt) and
[`ci.example.yml`](ci.example.yml). One CI stanza; exit 3 blocks; the
reviewer reads a permission sheet with capability changes first. *(B5;
invariant 5)* — Verify: this is the review moment you want a teammate to
have.

**VB-6 · Drift transcripts.** Read
[`transcripts/02-stale-gen.txt`](transcripts/02-stale-gen.txt). *(B6)*
— Verify: the error's shape (named code, cause, one-line fix) is the bar.

**VB-7 · Deploy + ops.** Read
[`transcripts/03-apply.txt`](transcripts/03-apply.txt) and
[`.tau/envs/local.state.toml`](.tau/envs/local.state.toml). One added
deploy step; the trigger becomes a systemd-user timer; the pin is
committed and secret-free. *(B1; rides E-4)* — Verify: "no scheduler
service in the architecture diagram" is the promise you want kept.

**VB-8 · The 3 a.m. story.** Read
[`transcripts/04-replay.txt`](transcripts/04-replay.txt). Copy the
journal, replay the exact run, divergence is named. *(B7; invariant 11)*
— Verify: this is the debugging story, including the divergence error.

**VB-9 · Augmentation, tools rung 2.** Open
[`crates/app-star-tools/src/lib.rs`](crates/app-star-tools/src/lib.rs).
Capabilities derive from the attribute; an un-grantable tool fails
`tau check` at build. *(X1; invariant 5)* — Verify: signature-derived +
`[allow]`-bounded is how project tools should feel.

**VB-10 · Model ceiling.** Re-open
[`models/default.toml`](models/default.toml): the `[ceiling]` table is
what environments may late-bind within. *(X3; invariant 10)* — Verify:
routing-as-config-under-ceiling covers your "models per project".

## Recording your verdict

For each VB checkpoint: **OK** (matches the vision), **AMEND** (right
beat, wrong shape — say what instead), or **WRONG** (the beat itself is
off — this recuts an invariant or requirement in the spec). Amendments to
snippet shapes are cheap forever; WRONGs are cheap only until the
worked-examples doc is ratified — that is the point of this walkthrough.
