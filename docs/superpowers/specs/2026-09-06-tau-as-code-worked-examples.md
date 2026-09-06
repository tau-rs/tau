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

**Review log:** 2026-09-06 maintainer walkthrough of §2 (app-star), three
amendments folded in: **(a)** collection = dedicated `tau/` folder by
convention (`tau/pipelines/` scanned, the E-2.2 lane), `[synth] entry` as
the opt-in composition escape hatch — the same-file "dual projection"
resolver is dropped; **(b)** the generated client (`tau/gen`) is the only
runtime surface, and the **typed-reference rule** is adopted (B8);
**(c)** `tau/gen` is **gitignored + freshness-checked**, not committed —
this amends the design doc §1 sentence "tau.gen.ts … committed" (body
prose, not a §10 numbered decision) and the corresponding ADR-wave line
should be touched up when that PR is reviewed. §3 (harness-star) review
pending.

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
├── package.json                # theirs; + tau dep, postinstall runs `tau gen`
├── src/                        # their app — tau never touches it
│   └── routes/tickets.ts       # ← the caller (+2 lines)
├── tau/                        # everything tau AUTHORS, one folder
│   ├── tau.toml                # constitution; no [synth] entry needed
│   ├── agents/triage.md        # vocabulary: markdown + frontmatter (dirs lane)
│   ├── models/default.toml     # model identity → provider binding
│   ├── pipelines/triage.ts     # scanned by convention (E-2.2); id = file path
│   └── gen/                    # GENERATED typed client — gitignored
├── .tau/                       # everything tau MANAGES
│   └── envs/local.state.toml   # the pin — committed, secret-free (E-4.1)
└── crates/ticketflow-tools/    # optional muscle: #[tau::tool] project Rust
```

The app repo **is** a tau project: `tau/` = what you author, `.tau/` =
what tau manages, `tau/gen` = the bridge out. The **default is pure
convention** — `tau/pipelines/` is scanned exactly like posture A's
`pipelines/` dir, zero new authoring machinery. The **escape hatch** for
teams that want to own collection (true co-location included) is
declaring `[synth] entry` and composing their own import graph — the
React model: convention by default, composition when you outgrow it.

### 2.2 The constitution stays TOML — even here

```toml
# tau/tau.toml  (illustrative shape)
[project]
id = "ticketflow"

# no [synth] entry: tau/pipelines/ is scanned by convention. Escape hatch:
#   [synth]
#   entry = "../src/tau.entry.ts"   # own your collection (co-location etc.)

[allow]                          # never emittable by code — decision 5, unmoved
net = ["api.anthropic.com", "billing.corp.internal:443"]

[allow.fs]
read = ["../kb"]
```

```markdown
<!-- tau/agents/triage.md -->
---
id: triage
model: default
tools: [kb.search, billing.lookup]
---
You triage inbound support tickets. Categorize, set urgency, and flag
accounts that need a billing check…
```

### 2.3 The definition and the call site — split by the toolchain

**The definition** runs *only* at build time, inside tau's synth sandbox
— the way a Terraform file is only ever read by `terraform`. It is not
part of the app bundle; nothing in `src/` imports it. (Design §4 API
rules apply: typed non-coercible handles, predicate methods — never
lambdas.)

```ts
// tau/pipelines/triage.ts — build-time only; id = file path
import { pipeline } from "tau";
import { agents, tools } from "../gen";

export default pipeline((p) => {
  const t = p.agent("classify", agents.triage, { input: p.input });

  p.branch("flagged?", t.output.billing_flag.isTrue(), (b) => {
    b.tool("billing", tools.billing.lookup, { customer: t.output.customer_id });
  });

  p.check("categorized", t.output.category.isNonEmpty());
  return { verdict: t.output };
});
```

**The typed-reference rule** (B8, adopted at review): *you write a string
only when declaring a new name; every reference to declared vocabulary is
a generated typed symbol.* `"classify"` / `"flagged?"` / `"categorized"`
declare step ids (explicit ids, collision = synth error — locked); but
`agents.triage`, `tools.billing.lookup`, and field access
`t.output.category` come typed from `tau/gen` — a typo is a compile
error, autocomplete lists exactly the project vocabulary. Field proxies
compile to the locked JSON-pointer reads
(`${steps.classify.output.category}`): typed surface, unchanged
semantics. The rule holds in every declension (Python:
`h.pipelines.oncall_investigate`, never `h.session("…")`).

**The call site** is ordinary app code importing the *generated client*
— the only runtime surface:

```ts
// src/routes/tickets.ts — runtime only
import { pipelines } from "../../tau/gen";

app.post("/tickets", async (req, reply) => {
  const { verdict } = await pipelines.triage.run({ ticket: req.body }); // typed I/O
  reply.send(verdict);
});
```

Definitions go *into* the toolchain; `tau/gen` comes *out*; the app only
touches what comes out. `pipelines.triage.run()` executes the **pinned**
bundle out-of-process — warm `tau serve` socket when present, CLI NDJSON
contract otherwise; the definition body never executes in the app
process. (The earlier same-symbol "dual projection" resolver was dropped
at review: unnecessary once the dedicated folder is the default;
escape-hatch users get the same effect with a userland re-export.)

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

**(2) Drift is impossible-silently.** `tau/gen` is **gitignored** —
generated code reproducible from committed sources is a build artifact —
and regenerated by `tau dev` (watch), npm postinstall, and CI. The
guarantee moves from "committed + stamped" to **freshness, verified
everywhere it matters**: `tau check` fails on a stale registry stamp, the
double-synth byte-identity gate (already locked) keeps generation
deterministic, and a definition ahead of the pin fails `tau plan
--check`. Green CI *means* the code, the artifact, and the pin agree. (A
repo that wants committed bindings may commit them; the stamp check works
either way.)

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
  scaffold laying down the `tau/` folder; `tau/` = authored, `.tau/` =
  managed (rides E-2/E-4).
- **B2** — collection: `tau/pipelines/` scanned by convention (the E-2.2
  lane, zero new machinery); `[synth] entry` as the declared escape hatch
  that owns collection — sandbox read-set derives from folder or import
  graph respectively.
- **B3** — the generated client is the only runtime surface: `tau/gen`
  exposes `pipelines.<id>.run()` typed handles bound to the pinned
  bundle; definition modules are never imported by app code.
- **B4** — typed handle transport selection: serve socket when warm, CLI
  NDJSON fallback, identical semantics.
- **B5** — plan-gated CI recipe: documented one-stanza `tau plan --check`
  gate with exit-3 semantics (rides E-3.3).
- **B6** — gen freshness: `tau/gen` gitignored + regenerated (dev watch /
  postinstall / CI); `tau check` fails stale stamps; stale pin is a named
  `plan --check` failure distinct from widening (rides E-1.4 + E-4.1).
- **B7** — journal portability: a journal captured in env `local` on one
  machine replays on another against the same IR hash (rides E-3.1/3.2).
- **B8** — the typed-reference rule: strings only declare new ids; all
  references to vocabulary (agents, tools, pipelines, models, output
  fields) are generated typed symbols; field proxies compile to
  JSON-pointer reads.

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
4. **The toolchain makes the split.** Definitions are harvested at build
   time; the generated client is the only runtime surface app code
   touches; and drift anywhere along the chain — vocabulary → generated
   code → artifact → pin — is a loud, named failure, never silent.
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
| 0 (rides v1 as-is) | B1 B2 B5 X1 | E-1..E-4 merged (folder convention = the E-2.2 lane + docs/scaffold) |
| 1 (first new machinery) | B6 B8 C1 C2 X4 | ADRs: gen surface + typed-reference rule, harness schema |
| 2 (transport) | B4 C3 C4 C7 | serve v2 lands (v2 backlog) |
| 3 (codegen) | B3 C5 | typed-client codegen lands (v2.5) |
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
