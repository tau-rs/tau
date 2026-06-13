# Goals and deliverables

tau pipelines can run a sequence of agents and tools, but nothing
stops them from "running clean and delivering nothing" — a failure
mode discovered by a human reading the output, not by the system.
Goals and deliverables are tau's first-class postcondition primitives:
structured assertions that the pipeline checks at both build time and
runtime so the *system* catches the gap, not you.

This page explains the model, the two primitives, failure handling,
and what the build-time guarantees cover.

## The 2x2: what and how

The design separates two axes that are easy to conflate:

- **What is asserted** — a *goal* (a condition must hold) vs a
  *deliverable* (an artifact must be produced and be any good).
- **How it is verified** — *deterministic* (a pure predicate, no LLM)
  vs *LLM-judged* (a semantic verdict).

```
                    VERIFIED BY
                deterministic          LLM-judged
              ┌────────────────────┬────────────────────┐
  a condition │      goal          │       ——            │
  must hold   │  "cites >=2 srcs"  │  (a goal done      │
              │  predicate, no LLM │   wrong)            │
              ├────────────────────┼────────────────────┤
  an artifact │  existence floor   │  deliverable        │
  must exist  │  (the cheap gate)  │  "is report.md any  │
  & be good   │                    │   good?" content    │
              └────────────────────┴────────────────────┘
```

These pair each kind with the verification it is actually good at:
predicates answer "did we hit the number?"; an LLM answers "is this
any good?", which a predicate cannot.

## Where checks live

Goals and deliverables are authored in `tau.toml` (and `.ts` via the
TS authoring surface), lowered into the IR, and consumed by `tau
check`, `tau build`, and `tau run`. This placement is not optional —
it is what makes build-time enforcement possible. The legacy
standalone `tau-workflow` runner (`workflows/*.toml`) does not lower
to IR and does not gain this feature.

A check is *defined* in a `[goals.*]` or `[deliverables.*]` table and
*positioned* in the pipeline via a `[[pipeline.steps]]` entry:

```toml
[[pipeline.steps]]
id  = "verify_report"
run = "check:report"        # references [deliverables.report]
```

Positioning determines what has already run when the check fires, and
what is in scope for a retry span. Multiple checks at different points
in a pipeline are allowed; the same `run = "check:<id>"` grammar
handles both.

## The `goal` primitive

A goal asserts a *measurable* condition deterministically — no LLM,
no model cost.

```toml
# easy path — fixed predicate menu
[goals.has_sources]
evaluates = "/workspace/report.md"     # a filesystem path
check     = "matches"
pattern   = "(?m)^## Sources"

# named-output locus
[goals.summary_present]
evaluates = "steps.writer.output"      # a named pipeline output
check     = "non_empty"

# power path — registered native Rust fn (escape hatch)
[goals.link_health]
evaluates = "/workspace/report.md"
fn        = "research_checks::all_links_resolve"
```

**Predicate menu (v1).** The `check` field accepts:

| Predicate | What it tests |
|---|---|
| `exists` | file/output is present |
| `non_empty` | file/output is non-empty |
| `equals` | byte-equal to a literal `value` |
| `matches` | regex `pattern` matches |
| `min_count` | byte/line count `>=` threshold |
| `schema_valid` | JSON value validates against a schema |

**Native-fn escape hatch.** `fn = "<crate>::<path>"` references a
function registered in the `DeterministicRegistry`. Arbitrary
predicate power; requires shipping and registering Rust code.

**Loci.** `evaluates` is a *read* locus — either a filesystem path
(`"/workspace/report.md"`) or a named output reference
(`"steps.<id>.output"`). It inspects an existing value or file; it
does not bind a producing step (that is a deliverable's job).

## The `deliverable` primitive

A deliverable asserts an artifact is produced *and* its content is
acceptable. It has three layers — two deterministic, one LLM:

```
 deliverable "report.md is a coherent summary"
   build time : EXISTS a producer wired to write report.md, permitted to do so   (deterministic)
   runtime gate: report.md exists & non-empty                                     (deterministic, cheap)
   runtime judge: its CONTENT satisfies must_satisfy                              (LLM)  <- headline
```

```toml
[deliverables.report]
path         = "/workspace/report.md"
must_satisfy = "A coherent summary that accurately reflects the sources."
on_fail      = "retry"          # default is "abort"
max_attempts = 3
retry_from   = "writer"         # the gate; default = the bound producer
```

### Loci (v1)

- **Filesystem path** — `path = "/workspace/report.md"`. Producer
  bound via `produces` (see next section). Runtime: existence check
  then judge reads the file.
- **Named data output** — `output = "steps.writer.output"`. Producer
  is the emitting step. Runtime: value is non-empty then judge reads
  the value.

Deferred for a future release: external resources (HTTP/MCP
side-effects), provenance evidence ("tool X was invoked"), and
process outcomes ("step exited 0" — that belongs to `goal`).

### Producer binding (build-time contract)

A deliverable must bind to a producing step at build time. The
mechanism is an explicit `produces` declaration on the agent or step —
a statement of intent, analogous to a function's return type:

```toml
[agents.writer]
model         = "claude-haiku-4-5"
prompt.system = "Write /workspace/report.md from the notes."
tool_refs     = ["write_file"]
produces      = ["/workspace/report.md"]

[tools.write_file]
native       = "WriteFile"
capabilities = ["fs-write:/workspace/**"]
```

Build-time checks catch two mismatches:

```
error: deliverable 'report' has no producer
       no step declares  produces = ["/workspace/report.md"]

error: step 'writer' declares it produces '/workspace/report.md'
       but holds no fs-write capability covering that path
```

**Honest limits.** The build promises *"the pipeline is wired to
produce X"* — not that the LLM actually writes good content. For an
`Agent` producer the guarantee is "capable of"; for a `Deterministic`
producer it is "guaranteed". A step that declares `produces` but whose
LLM does not actually write the file is caught only at the runtime
existence gate, never at build time. This is inherent to LLM-produced
content and is stated plainly rather than papered over.

### Judge resolution

`must_satisfy` is always required (it is the criterion). The judge
selects *who* evaluates it:

| Effort | Author writes | Who judges |
|---|---|---|
| Easy | just `must_satisfy` | tau's built-in minimalist judge (fixed canonical prompt, default model) |
| Tune | `must_satisfy` + `judge_model` | same minimalist judge, chosen model |
| Power | `must_satisfy` + `judge = "my_agent"` | a user `[agents.*]` — e.g. the `critic` reference package |

Build-time checks catch configuration errors:

```
error: deliverable 'report' sets judge = "house_critic"
       but no [agents.house_critic] is defined

error: deliverable 'report' sets both judge_model and judge
       — a custom judge brings its own model
```

**Verdict contract.** Every judge — built-in or custom — must return:

```jsonc
{ "met": bool, "rationale": string }
```

`rationale` is load-bearing: it is the "why" fed back into the retry
loop. The contract is prompt-enforced (the canonical judge prompt
instructs the JSON shape). An unparseable verdict is treated as
`met = false` with a diagnostic rationale so a flaky judge re-prompts
on retry rather than causing a hard kernel error.

The judge reads the **actual produced artifact** via the engine's
trusted `read_artifact` dispatcher — it judges the real file or output
value, not an in-memory reference.

## Failure handling: abort or rewind-to-gate retry

A failed check is either a gate or a feedback loop.

- **Default — `abort`.** The run exits non-zero; the
  rationale/diagnostic is the message. Pure assertion, zero engine
  cost.
- **Opt-in — `retry`.** On failure, execution **rewinds to the gate**
  (`retry_from`) and re-runs forward through the producer back to the
  check, with the failure rationale injected, bounded by
  `max_attempts`.

```
         ┌───────── rewind to gate, inject "why" ──────────┐
         │                                                   │
┌────────▼──┐     ┌──────────┐     ┌─────────────┐  fail & attempt<max
│   gather  │ ──> │  writer  │ ──> │   check     │ ─────────────────────┘
└───────────┘     └──────────┘     └──────┬──────┘
  gate can be          ^             pass │  │ fail & attempt==max
  moved here           │                 ▼  ▼
  (default gate ───────┘            continue  abort non-zero
   = producer)
```

### The gate (`retry_from`)

The gate is the rewind point. It defaults to the bound producer and
may be moved to any *earlier* step — but never after the producer. A
producer produces; steps after it only consume, so rewinding past it
could not change the artifact. This is a build-time check:

> **Guarantee 1 — gate position.**
> `error: deliverable 'report' has retry_from = "polish"`
> `but 'polish' runs after producer 'writer'`
> `— the gate must be at or before the producer.`

A second guarantee follows from determinism:

> **Guarantee 2 — retry must be able to change something.**
> `error: deliverable 'report' sets on_fail = "retry"`
> `but the retry span (gather -> writer) contains no non-deterministic step;`
> `retrying cannot change the result.`

A purely deterministic span produces identical output on every
attempt. Retrying it is a guaranteed no-op loop and is rejected at
build time rather than wasted at runtime.

A third guarantee prevents ambiguous attempt counting when multiple
checks have retry spans:

> **Guarantee 3 — retry spans must not overlap.**
> Two checks whose `[gate..check]` intervals share any step are
> rejected at `tau check` — overlapping retry semantics are deferred.

### The feedback loop (the "why")

On each retry, the verdict's `rationale` is injected as an extra turn
into every **agent** step inside the retry span. Deterministic steps
ignore it — they are pure functions. This is what makes the loop
*converge* rather than reroll blindly: attempt 2 of `writer` literally
sees *"previous attempt rejected: only 1 source cited, need >=2."*

After `max_attempts`, the run aborts non-zero with the final rationale.

### Budget bounding

The retry loop is bounded by `max_attempts`. Each attempt's agents are
independently capped by the existing per-agent `AgentBudget`
(`max_turns` / `max_tokens`). The real bound is therefore
`max_attempts × per-attempt AgentBudget`: a stubborn judge or producer
cannot burn unbounded tokens. The per-agent budget resets each attempt
(today's `run_agent` is constructed fresh per call); a cumulative
cross-attempt token cap is a clean follow-up, not v1.

## Trace events

Every check evaluation emits a structured event — pass *or* fail —
via the existing tracing layer, landing in the JSONL run log and any
OTLP export:

```jsonc
{ "event": "check.evaluated", "id": "has_sources", "kind": "goal",
  "verdict": "pass", "attempt": 1 }
{ "event": "check.evaluated", "id": "report", "kind": "deliverable",
  "verdict": "fail", "attempt": 1, "locus": "/workspace/report.md",
  "rationale": "only 1 source cited; need >=2" }
{ "event": "check.retry", "id": "report", "rewind_to": "writer", "next_attempt": 2 }
{ "event": "check.evaluated", "id": "report", "kind": "deliverable",
  "verdict": "pass", "attempt": 2 }
```

The full retry history — with the *why* at each step — is durably in
the trace.

## Worked example (end to end)

A research pipeline: gather sources, write a report, check both a
structural goal and content quality.

```toml
[project]
name = "research"

[agents.gather]
model         = "claude-haiku-4-5"
prompt.system = "Research the question; collect sources."

[agents.writer]
model         = "claude-haiku-4-5"
prompt.system = "Write /workspace/report.md from the notes."
tool_refs     = ["write_file"]
produces      = ["/workspace/report.md"]

[tools.write_file]
native       = "WriteFile"
capabilities = ["fs-write:/workspace/**"]

[goals.has_sources]                      # deterministic, crisp, no LLM
evaluates = "/workspace/report.md"
check     = "matches"
pattern   = "(?m)^## Sources"

[deliverables.report]                    # LLM judges the artifact's content
path         = "/workspace/report.md"
must_satisfy = "A coherent summary that accurately reflects the sources."
on_fail      = "retry"
max_attempts = 3
retry_from   = "writer"

[[pipeline.steps]]
id  = "gather"
run = "agent:gather"

[[pipeline.steps]]
id  = "writer"
run = "agent:writer"

[[pipeline.steps]]
id  = "check_sources"
run = "check:has_sources"

[[pipeline.steps]]
id  = "check_report"
run = "check:report"
```

What each tau command does with this file:

- **`tau check`** — proves statically: `report` has a declared
  producer (`writer`) permitted to write the path; `retry_from =
  "writer"` is `<=` the producer; the retry span `[writer..check_report]`
  contains an LLM step; `has_sources` regex compiles. No tokens spent,
  no model invoked.
- **`tau build`** — lowers to IR, encodes, archives. Same structural
  checks run as part of lowering.
- **`tau run`** attempt 1 — `writer` writes a report citing 1 source
  → `check_sources` predicate fails → run exits non-zero.
  (Alternatively, with `on_fail = "retry"` on `has_sources`: rewind to
  `writer` with *"only 1 source cited; need >=2"*.) Then `check_report`
  judge: fails → rewinds to `writer` → attempt 2 passes → run succeeds.
  Every verdict in the trace.

## v1 scope note

Runtime check *evaluation* (`check.evaluated` events, the retry loop)
runs on `tau run` (local) only in v1 — `tau dev` and `tau run
--bundle` do not yet dispatch pipelines, so check steps are not
reached there. However:

- The IR and bundle format carry `Check` nodes fully today — a bundle
  built now will evaluate when bundle pipeline dispatch ships.
- The headline build-time guarantees (`tau check` / `tau build`) work
  on every surface: they are pure lowering-time analysis with no
  runtime dependency.

## What this is not

Two common misreadings:

- **Not a test framework.** Goals and deliverables assert
  *postconditions of a run*, not expected outputs of deterministic
  functions. For unit-testing tool plugins or deterministic steps, use
  the standard Rust test infrastructure and the `ir-conformance`
  fixture suite.
- **Not a score threshold.** `met` is boolean in v1; score-with-threshold
  verdicts are a planned follow-up. If you need "at least 80% of
  criteria met", write a custom judge agent that performs that
  aggregation internally and returns a binary `met`.

## See also

- [Workflows](workflows.md) — the broader pipeline model; goals and
  deliverables are positioned inside `[[pipeline.steps]]`.
- [Capabilities and consent](capabilities-and-consent.md) — the
  `fs-write` capability that backs the producer binding.
- [Multi-agent orchestration](multi-agent-orchestration.md) — how
  agents spawn sub-agents; the same agent machinery runs the judge.
- [tau-philosophy.md](tau-philosophy.md) — the build-time enforcement
  principle that motivated this feature.
- [Escape hatches](escape-hatches.md) — where native-fn goal predicates
  are registered.
