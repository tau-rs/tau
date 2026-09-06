# harness-star — vision fixture for posture C (tau as the substrate)

> **STATUS: VISION FIXTURE — target state, not yet runnable.** Nothing
> here builds today; machinery lands per the wave table in the
> worked-examples spec §6 (serve v2 gates the transport, typed-client
> codegen gates the declension). This tree exists so the vision can be
> **verified file-by-file before implementation**, at the path it will
> live at forever.
> Spec: [`worked-examples §3`](../../docs/superpowers/specs/2026-09-06-tau-as-code-worked-examples.md).
> One deliberate refinement over the spec sketch: **three** host-tool
> obligations (the runbook fix executes through the host too), which lets
> the fixture show a dangerous host tool reachable only through an
> approved check.

**The story:** *Fieldbook* — a platform team builds an on-call copilot in
Python. An engineer asks "why is checkout latency up?"; an agent
investigates through the company's own tooling and executes a runbook fix
**only after a human approves**. The team refuses to rewrite their Python
integrations, and security refuses powers that live in prompt text. They
don't build a harness — they **declare** one, and tau declines it into
Python.

## Walkthrough — verify each checkpoint

**VC-1 · The harness is declared, not coded.** Open
[`tau.toml`](tau.toml) `[harness]`: the exposed set, three host-tool
obligations (id + schema + claims), the session ceiling, the approval
route. All vocabulary/governance → all TOML. *(C1; invariants 3, 7)*
— Verify: this is what "defining a harness with tau" should mean — a
declaration compiled into the artifact.

**VC-2 · Pipelines stay ordinary.** Open
[`pipelines/runbook-exec.ts`](pipelines/runbook-exec.ts) and
[`agents/investigator.md`](agents/investigator.md). Host tools are used
like any tool; the approval is a normal check the TOML routes outward.
Posture C changes what surrounds a pipeline, never how one is written.
*(invariant 12)* — Verify: no "harness-flavored" authoring dialect.

**VC-3 · The card is the security review.** Read
[`transcripts/01-inspect-harness.txt`](transcripts/01-inspect-harness.txt).
Obligations, claims, the `HOST-ENFORCED ⚠` honesty label, the session
ceiling, the approval route — one screen. *(C2; invariants 7, 9)*
— Verify: you'd hand this rendering to a security reviewer as-is.

**VC-4 · Refusal up front.** Read
[`transcripts/02-refusal.txt`](transcripts/02-refusal.txt). Unmet
obligations refuse at connect, gap named. *(C3; invariant 8)* — Verify:
connect-time is where you want this to fail.

**VC-5 · The declension.** Open
[`host-py/fieldbook_harness/`](host-py/fieldbook_harness/__init__.py)
(generated, stamped) then [`host-py/app.py`](host-py/app.py) (the team's
~40 lines: three typed tool methods + one approval callback). No tau
runtime in the Python process. *(C5; invariant 6)* — Verify: this is what
"declining tau into another language" should feel like — implement the
obligations, nothing else.

**VC-6 · The approval round-trip.** Read
[`transcripts/03-approval-roundtrip.txt`](transcripts/03-approval-roundtrip.txt).
Suspend → typed elicitation → human → typed resume; every crossing
journaled and replayable, decision included. *(C4; invariant 11)*
— Verify: the incident-review story (replay with the human decision in
the record) is the bar.

**VC-7 · Transport parity.** Open
[`host-wasm/embed.mjs`](host-wasm/embed.mjs). Same bundle, same card,
satisfied over the WIT world instead of the socket — proof the harness
surface is the artifact contract seen from the host side, not a third
contract. *(C6)* — Verify: one card, two transports is the right
accounting.

**VC-8 · Generated means regenerable.** Read
[`transcripts/04-export-harness.txt`](transcripts/04-export-harness.txt),
including the stale-stamp refusal. *(C5; invariants 4, 6)* — Verify:
delete-and-regenerate as the only maintenance mode for declensions.

**VC-9 · Env narrowing + sandbox profile + model ceiling.** Open
[`.tau/envs/prod.state.toml`](.tau/envs/prod.state.toml): a named
versioned sandbox profile and an endpoint late-bound under
[`models/default.toml`](models/default.toml)'s ceiling — narrowing only,
run-or-refuse preserved. *(X2, X3; invariant 10)* — Verify: this covers
"custom sandboxing and models per project".

## Recording your verdict

Same protocol as [`app-star`](../app-star/README.md): per checkpoint,
**OK** / **AMEND** (right beat, wrong shape) / **WRONG** (the beat itself
— recuts the spec's invariants or requirements before ratification).
