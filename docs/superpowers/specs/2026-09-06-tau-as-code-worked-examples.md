# tau as code — the worked examples (setting the vision in stone)

**Status:** VISION-NORMATIVE companion to
[`2026-09-06-tau-as-code-vision.md`](2026-09-06-tau-as-code-vision.md).
This document exists to *set the vision in stone* the only honest way a
vision can be: as concrete scenarios a developer can recognize themselves
in, distilled into invariants and requirements an implementation tree is
built from and checked against.

**Normativity ladder — read this first.** Three levels, weakest binds last:

1. **The invariants (§5)** are the stone. Once ratified (merged), a future
   design that breaks one is wrong by definition; changing an invariant
   takes the same ceremony as a §10 decision (argued ADR, not drift).
2. **The scenario capabilities** (each example's "what must be true" list)
   are the acceptance bar: the vision is *delivered* when the fixtures in
   §7 exercise them green in CI.
3. **The surface shapes** (function names, TOML keys, CLI flags shown in
   snippets) are **illustrative only** — they show one coherent possibility
   consistent with the locked design; the ADRs for each slice fix the real
   names. Nobody may cite a snippet against an ADR.

**Date:** 2026-09-06.

**Relates to:** design §10 (all locked decisions honored; none re-opened);
vision doc §1–§6; [`vision-roadmap.md`](../plans/vision-roadmap.md);
the north-star fixture lineage (arc acceptance = *north-star-v2*; this doc
adds two siblings, §7).

---

## 1. The cast

| Example | Posture | Developer | Language(s) | What it proves |
|---|---|---|---|---|
| **north-star** (exists) | A — tau as the project | a maker automating their own repo | TS + TOML + Rust | v1 golden path; baseline, not re-argued here |
| **Ticketflow** (§2) | B — tau in the project | a SaaS team adding agentic triage to an existing Fastify backend | TS app | co-located definition, typed invocation, plan-gated CI, one-step deploy |
| **Fieldbook** (§3) | C — tau as the substrate | a platform team building an on-call copilot for their org, in their stack | Python host (+ wasm variant) | harness declaration, obligations card, declension, reverse dispatch, refusal-up-front |
| **The augmentation trio** (§4) | cross-cutting | both teams above | Rust · TOML · env pins | per-project tools, sandbox profiles, model bindings — all carded |

Posture A is deliberately not re-worked: E-0..E-4 own it, and *north-star-v2*
is already its acceptance witness. Everything below stands on it.

---

## 2. Ticketflow — tau in the project

**The team:** four engineers run a support SaaS on Fastify/TypeScript.
They want inbound tickets triaged by an agent — category, urgency, a
billing lookup when the customer is flagged — *without* adopting a second
repo, a second deploy pipeline, or a workflow console. Their bar: it lives
next to the route that calls it, it type-checks against their code, and no
agent gets a capability their reviewer didn't see.

### 2.1 The repo, after adopting tau

```text
ticketflow/
├── package.json
├── tau.toml                    # the root: project id + [synth] entry + [allow]
├── agents/
│   └── triage.md               # vocabulary: markdown + frontmatter (dirs lane)
├── models/
│   └── default.toml            # model identity → provider binding
├── tau.gen.ts                  # generated, committed, hash-stamped (E-1.4)
├── .tau/envs/local.state.toml  # the pin — committed, secret-free (E-4.1)
└── src/
    ├── tau.entry.ts            # [synth] entry: explicitly imports every *.tau.ts
    ├── triage/triage.tau.ts    # ← the co-located pipeline (choreography only)
    └── routes/tickets.ts       # ← the caller (ordinary app code)
```

The app repo **is** a tau project. Nothing about dirs-scanning, `[allow]`,
or the pin assumed a dedicated repo; posture B is a packaging stance, not a
new mode.

### 2.2 The constitution stays TOML — even here

```toml
# tau.toml  (illustrative shape)
[project]
id = "ticketflow"

[synth]
entry = "src/tau.entry.ts"      # runs sandboxed at BUILD time; never at runtime

[allow]                          # never emittable by code — decision 5, unmoved
net = ["api.anthropic.com", "billing.corp.internal:443"]

[allow.fs]
read = ["./kb"]
```

```markdown
<!-- agents/triage.md -->
---
id: triage
model: default
tools: [kb.search, billing.lookup]
---
You triage inbound support tickets. Categorize, set urgency, and flag
accounts that need a billing check…
```

### 2.3 The co-located pipeline — one symbol, two projections

```ts
// src/triage/triage.tau.ts  (illustrative shape; design §4 API rules apply:
// typed non-coercible handles, tagged-template interpolation, predicate
// methods — never lambdas)
import { pipeline, tau } from "tau";
import { agents, tools } from "../../tau.gen";

export const triage = pipeline("triage", (p) => {
  const t = p.agent("classify", agents.triage, { input: p.input });

  p.branch("flagged?", t.output.field("billing_flag").isTrue(), (b) => {
    b.tool("billing", tools.billing.lookup, {
      customer: tau`${t.output.field("customer_id")}`,
    });
  });

  p.check("categorized", t.output.field("category").isNonEmpty());
  return { verdict: t.output };
});
```

```ts
// src/routes/tickets.ts — ordinary app code, no tau concepts beyond the handle
import { triage } from "../triage/triage.tau";

app.post("/tickets", async (req, reply) => {
  const { verdict } = await triage.run({ ticket: req.body }); // typed I/O
  reply.send(verdict);
});
```

What the toolchain does with that one module:

- **At build (synth):** the sandboxed subprocess evaluates
  `src/tau.entry.ts`, which imports `triage.tau.ts`; `pipeline()` registers
  choreography into the emitted ProjectConfig JSON — the *same* lane, id
  grammar, collision rules, and single `validate()` as `pipelines/*.ts`
  (E-2). In-code definition is sugar over the flow lane, literally.
- **At app runtime:** the same `triage` symbol resolves through the
  generated typed client (v2.5 machinery) to an invocation handle bound to
  the **pinned** bundle — warm `tau serve` socket when present, CLI NDJSON
  contract otherwise. The definition body never executes in the app
  process. How the same import resolves differently (conditional export,
  gen-side splitting) is an ADR question; the visible contract above is
  the fixed point.

### 2.4 The four DX beats that ARE the vision

**(1) Widening is loud.** A dev adds a refund tool that needs
`stripe.com` egress. The PR touches `[allow]`; CI runs `tau plan --check`;
exit code 3 (capability widening) fails the gate and the plan renders
capability changes first:

```text
$ tau plan --check
~ pipeline triage
    + tool refund (stripe.refund)
! capabilities WIDEN:
    + net: api.stripe.com          ← requires review
exit 3
```

The reviewer reads a permission sheet, not a diff of YAML soup.

**(2) Drift is impossible-silently.** The dev edits `triage.tau.ts` but
forgets to re-pin. The gen hash-stamp fails the app build ("stale
tau.gen.ts"), and a definition ahead of the pin fails `tau plan --check`.
Green CI *means* the code, the artifact, and the pin agree.

**(3) Deploy is one added step.** Their existing pipeline gains
`tau apply` (atomic per repo) after the app deploy. A `[trigger]` for the
nightly backlog sweep compiles to a systemd-user timer; no scheduler
service appears in their architecture diagram.

**(4) The 3 a.m. story.** A production triage run misfires. The on-call
engineer copies `.tau/runs/<id>/journal.jsonl` from the box, runs
`tau replay` locally against the same pinned IR, and steps the exact run —
deterministically, LLM calls and all. Divergence from a since-changed
definition is a *named* `ReplayDivergence`, not a silent pass.

### 2.5 What must be true (scenario capabilities → tree)

- **B1** — app-root project: golden-path docs + `tau init --app`
  scaffold; nothing else new (rides E-2/E-4).
- **B2** — explicit-import collection: `[synth] entry` reaches co-located
  `*.tau.ts` modules; sandbox read-set derives from the import graph.
- **B3** — dual projection: one exported symbol usable as definition
  (synth) and typed run-handle (app runtime); ADR decides the mechanism.
- **B4** — typed handle transport selection: serve socket when warm, CLI
  NDJSON fallback, identical semantics.
- **B5** — plan-gated CI recipe: documented one-stanza `tau plan --check`
  gate with exit-3 semantics (rides E-3.3).
- **B6** — stale-pin detection: definition/pin disagreement is a named
  `plan --check` failure, distinct from widening (rides E-1.4 + E-4.1).
- **B7** — journal portability: a journal captured in env `local` on one
  machine replays on another against the same IR hash (rides E-3.1/3.2).

---

## 3. Fieldbook — tau as the substrate (the harness, declined)

**The team:** the platform group at a mid-size company builds *Fieldbook*,
an on-call copilot: a Python CLI + Slack surface where an engineer types
"why is checkout latency up?" and an agent investigates — reading
dashboards through the company's own tooling, proposing a runbook fix, and
executing it **only after a human approves**. They will not rewrite their
Python integrations, and security will not accept an agent whose powers
live in prompt text.

They do not build a harness. They **declare** one, and tau declines it
into Python.

### 3.1 The harness declaration (vocabulary → TOML, per the split)

```toml
# tau.toml, harness section  (illustrative shape)
[harness]
expose = ["oncall-investigate", "runbook-exec"]   # the product surface;
                                                   # every other pipeline stays internal

[[harness.host_tool]]                # obligations: the HOST must provide these
id = "metrics.query"
schema = "schemas/host/metrics-query.json"
claims = ["net: prometheus.corp.internal:9090"]   # claimed, host-enforced

[[harness.host_tool]]
id = "pager.ack"
schema = "schemas/host/pager-ack.json"
claims = ["net: api.pagerduty.com"]

[harness.session]
late_binding_ceiling = { fs_read = ["./workdir"] }  # session MCP/tools may
                                                     # attach only under this

[harness.approvals]
elicit = ["runbook-exec/apply-fix"]  # this check routes to the host as a
                                     # typed elicitation (resume_schema)
```

### 3.2 The harness card — obligations are compiled, inspectable, refusable

```text
$ tau inspect --harness
fieldbook  ir:9f3ac2…  (harness card)

  EXPOSES
    oncall-investigate   in: AlertRef        out: Findings
    runbook-exec         in: RunbookRef      out: Applied | Refused

  HOST MUST PROVIDE                              enforcement
    metrics.query   (schemas/host/metrics-query) HOST-ENFORCED ⚠
    pager.ack       (schemas/host/pager-ack)     HOST-ENFORCED ⚠

  SESSION CEILING     fs.read ./workdir — nothing else may attach
  APPROVALS → HOST    runbook-exec/apply-fix (typed, resume_schema)
```

`HOST-ENFORCED ⚠` is the honesty label from decision 12: tau schema-checks
these calls, but their *effects* are the host's word — and the card says so
instead of implying engine-grade enforcement.

### 3.3 The declension — Python implements the obligations, nothing else

```text
$ tau export --harness py --out fieldbook_harness/
generated: fieldbook_harness/  (from harness card 9f3ac2… — regenerate on pin change)
```

```python
# fieldbook/app.py — the team's code, against the generated typed scaffold
from fieldbook_harness import Harness, HostTools, types

class Tools(HostTools):                       # exactly the declared obligations
    def metrics_query(self, req: types.MetricsQueryIn) -> types.MetricsQueryOut:
        return promql(req.query, req.range)   # their existing Python, untouched

    def pager_ack(self, req: types.PagerAckIn) -> types.PagerAckOut:
        return pagerduty.ack(req.incident_id)

def on_approval(elicit: types.ApplyFixElicitation) -> types.ApplyFixDecision:
    return slack_prompt(elicit)               # human decides; typed both ways

h = Harness.connect(tools=Tools(), on_approval=on_approval)  # serve v2 socket
session = h.session("oncall-investigate")
for event in session.run(types.AlertRef(id="PD-4412")):      # typed run events
    render(event)                              # loop/journal/budgets are tau's
```

Reverse dispatch (serve v2 `session.*`) calls `Tools.metrics_query` when a
step needs it; the `apply-fix` check suspends the run, elicits through
`on_approval`, and resumes with the typed decision. The team wrote ~60
lines: their tools, their approval UI. The loop, journal, replay, budgets,
capability enforcement, and card are tau's.

### 3.4 The two beats that ARE the vision

**(1) Refusal up front.** Ship a host build missing `pager.ack`? Session
start is refused with a named error listing unmet obligations — at
connect, not at step 7 of an incident:

```text
E-HARNESS-OBLIGATIONS: host provides 1/2 declared tools; missing: pager.ack
```

**(2) Same artifact, second transport.** The internal web console embeds
the *same* bundle as a wasm component; the WIT world (artifact contract
#2) carries the identical obligations. One harness card, two transports —
process (serve) and component (wasm) — because the harness surface is the
artifact contract seen from the host side, not a third contract.

### 3.5 What must be true (scenario capabilities → tree)

- **C1** — harness declaration: exposed set, host-tool obligations
  (id + schema + claims), session ceiling, approval routes; validated by
  `tau check` like everything else.
- **C2** — harness card: `tau inspect` rendering with obligations and the
  `host-enforced` label (extends E-3.4).
- **C3** — refusal-up-front: obligations verified at session start; unmet
  = named error naming what's missing.
- **C4** — reverse dispatch: serve v2 `session.*` host-tool calls +
  typed elicitation/resume (rides scheduled serve v2, made concrete here).
- **C5** — declension codegen: `tau export --harness <lang>` generating
  the typed host scaffold from the card (Python + TS first); generated,
  stamped, never hand-maintained.
- **C6** — transport parity: the same harness card honored over serve and
  the WIT world; a conformance fixture proves parity.
- **C7** — Rust embed prelude as reference declension: every C1–C6
  behavior expressible through the prelude first (dogfood: `tau dev`).

---

## 4. The augmentation trio — custom tools, sandbox, models, per project

Cross-cutting, shown in the two projects above; this is the "primordial"
requirement made concrete. The one rule everywhere: **declared + bounded +
carded, or it does not run.**

**(a) A project-local Rust tool (Ticketflow).**

```rust
// crates/ticketflow-tools/src/lib.rs  (E-1 lane; illustrative attribute shape)
#[tau::tool(caps(net = "billing.corp.internal:443"))]
/// Look up a customer's billing standing.
fn billing_lookup(customer_id: CustomerId) -> Result<BillingStanding, ToolError> {
    …
}
tau::export![billing_lookup];
```

Name, schema, description, capabilities derive from the signature +
attribute (one declaration per fact). If `[allow]` doesn't grant the
claimed host, **`tau check` fails at build** — the tool cannot exist
un-grantable. The tool appears on the card like any other power.

**(b) A sandbox profile (Fieldbook, org-wide).** Security publishes a
named, versioned profile — `nadir-locked@1`: no ambient network beyond
`[allow]` resolution, fs scoped to the workdir, exec denied — composed
from the capability vocabulary (ADR-0036 pattern). Projects select it;
environments may **narrow** it, never widen (decision 6; run-or-refuse
preserved):

```toml
# .tau/envs/prod.state.toml (v2 environments; illustrative)
sandbox.profile = "nadir-locked@1"
```

A custom containment strategy (their own jail wrapper) is a host-tier
adapter behind the sandbox port — same rule: it can only narrow, and the
active profile is on the card.

**(c) Model bindings per project, endpoints per environment.**

```toml
# models/default.toml — identity is project vocabulary
id = "default"
provider = "anthropic"          # provider = plugin; a project can ship its own
model = "claude-sonnet-5"

[ceiling]
allow = ["claude-sonnet-5", "claude-haiku-4-5"]   # env late-binding bounded here
```

Dev laptops bind `default` to a local Ollama plugin; prod binds the
corporate gateway endpoint — both are environment-tier late binding **under
the declared ceiling**, the same discipline as session tooling. Model
routing is config, never code.

**What must be true:**

- **X1** — un-grantable tools fail at `tau check`, with the missing grant
  named (rides E-1).
- **X2** — sandbox profiles: named/versioned vocabulary, project
  selection, env narrowing-only, card visibility; custom sandboxes as
  adapters behind a port.
- **X3** — model ceiling: env endpoint/model late-binding validated
  against a declared allowlist; violation = refusal with the ceiling
  cited.
- **X4** — card completeness: every rung of the tool ladder, the active
  sandbox profile, and the effective model binding all render on
  `tau inspect`.

---

## 5. The stone tablet — twelve invariants (proposed for ratification)

Distilled from the beats above. Merging this document ratifies them;
thereafter they bind like §10 decisions (change = argued ADR).

1. **One lowering.** Every authoring surface, sugar, and frontend — in any
   posture, any language — lowers through the synth contract into the one
   validator and the one frozen IR. There is never a second path.
2. **Build-time definition.** "In code" never means "at runtime". Runtime
   sees sealed artifacts only; runtime graph construction stays refused.
3. **The constitution is TOML wherever the project roots.** `[allow]`,
   agents, models, harness obligations: dirs/TOML, never emitted by code.
4. **Co-location without runtime authoring.** The same source file may
   carry a definition (harvested at synth) and its invocation (typed
   handle at runtime); the toolchain makes the split, and drift between
   them is a loud, named failure — never silent.
5. **Widening is always loud.** Any capability increase — tool grant, net
   host, sandbox loosening, model outside ceiling — is a first-class,
   diffable, CI-blockable event (plan exit 3 semantics), in every posture.
6. **No consumer needs a tau library.** Every language-facing surface is a
   generated, hash-stamped projection (declension) or a frozen wire
   contract; nothing language-facing is hand-maintained.
7. **The harness is a declared artifact.** What a host must provide, may
   attach, and will be asked to approve is compiled into the bundle and
   rendered on a card — not documented in a README.
8. **Refusal up front.** Unmet host obligations refuse at session start
   with the gap named; a run never discovers its harness mid-flight.
9. **Delegated enforcement is labeled.** Host-enforced power is visibly
   marked as such on the card; tau never implies engine-grade enforcement
   it doesn't perform.
10. **Late binding only under declared ceilings; narrowing only at host
    tier.** Sessions, environments, sandboxes, and models bind late only
    beneath something the artifact declared; nothing narrows-then-widens.
11. **One journal everywhere.** Every posture's runs record to the same
    journal substrate and replay with the same verbs; a declined harness
    is exactly as replayable as the golden-path repo.
12. **Rungs never tax.** A posture-A user never meets harness concepts; a
    posture-B user never meets serve internals; adopting a higher rung
    adds files, never rewrites lower-rung ones.

---

## 6. From examples to the implementation tree

The scenario capabilities are the tree's seed nodes. Requirement IDs above
(B1–B7, C1–C7, X1–X4) are stable — the future
`implementation-trees/tau-as-code.md` cites them, and each node's DoD is
"its beat from §2–§4 demonstrably works". Sequencing constraints the
examples impose:

| Wave | Nodes | Gate before it |
|---|---|---|
| 0 (rides v1 as-is) | B1 B5 B6 X1 | E-1..E-4 merged (nothing new — docs/scaffold/wiring) |
| 1 (first new machinery) | B2 B3 C1 C2 X4 | ADRs: collection convention, dual projection, harness schema |
| 2 (transport) | B4 C3 C4 C7 | serve v2 lands (v2 backlog) |
| 3 (codegen) | C5, then B3's runtime half | typed-client codegen lands (v2.5) |
| 4 (parity + ops) | C6 B7 X2 X3 | environments/promote (v2) |

Tree-building rule: no node may weaken a §5 invariant to ship sooner; a
node that seems to require it goes back to ADR.

---

## 7. Acceptance fixtures — the living witnesses

Vision **in stone** = this document merged (invariants ratified). Vision
**delivered** = two fixtures exist beside north-star and stay green:

- **`examples/app-star/`** — a minimal but real Fastify-style app built as
  Ticketflow (§2): co-located pipeline, typed handle, plan-gated CI, pin,
  replayed journal. Green = every B-requirement demonstrated.
- **`examples/harness-star/`** — a minimal Fieldbook (§3): harness
  declaration, Python declension over serve, wasm parity check, refusal
  test, approval round-trip. Green = every C-requirement demonstrated.
  (Fixture refinement: three host-tool obligations — `runbook.apply`
  joins the two sketched in §3.1, so a dangerous host tool reachable only
  through an approved check is demonstrated.)

Both fixtures exist **today as walkable skeletons** at those paths —
every file the target state, banner-marked not-yet-runnable, with
`transcripts/` showing expected CLI output and a README walkthrough of
verification checkpoints (VB-1..10, VC-1..9) mapped to the requirement
ids and invariants. The skeletons are the vision's review surface; they
graduate in place to the living witnesses as the waves in §6 land.

Both follow the north-star pattern: committed, CI-exercised, cited by docs
— the fixture *is* the spec's proof, and a regression in one is a
regression in the vision.

---

## 8. What the examples deliberately do not show

Boundaries, so nobody reads a missing scene as a missing rejection:
no runtime graph mutation anywhere (Fieldbook's agent investigates inside
Dynamic/Explore boxes; it never rewires the pipeline); no in-process tau
runtime in Python (the declension talks to a socket or a component); no
hand-written Python SDK (delete the generator's output and regenerate); no
tau registry (Fieldbook ships its harness as a plain OCI/npm artifact); no
prompt-text governance (every power shown lives in TOML and on the card).
