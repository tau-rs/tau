# Assert pipeline postconditions with goals and deliverables

You want your pipeline to *prove* it produced what it was supposed to,
not just exit zero. This recipe walks through `goal` (a deterministic
predicate) and `deliverable` (a produced artifact + LLM-judged content)
and shows how to add the self-correcting retry loop.

If you want the *why* behind the design, read
[Workflows](../explanation/workflows.md) and
[ADR-0044](../decisions/0044-deliverables-and-goals.md).

---

## Background: two primitives, two verification methods

```
                    VERIFIED BY
                deterministic          LLM-judged
              ┌────────────────────┬────────────────────┐
  a condition │      goal          │       (n/a)         │
  must hold   │  "cites >=2 srcs"  │                     │
              │  predicate, no LLM │                     │
              ├────────────────────┼────────────────────┤
  an artifact │  existence floor   │   deliverable       │
  must exist  │  (the cheap gate)  │  "is report.md any  │
  & be good   │                    │   good?" content    │
              └────────────────────┴────────────────────┘
```

- **`goal`** — crisp measurable condition, verified without spending any
  LLM tokens. Uses a fixed predicate menu or a registered native Rust fn.
- **`deliverable`** — artifact the build proves is *producible* and the
  runtime proves is *good*, via a swappable LLM judge returning
  `{ met, rationale }`.

Both live in `tau.toml` → IR → `tau build` / `tau check` / `tau run`.
The legacy standalone `tau-workflow` runner is not covered.

---

## Step 1 — declare a goal

A `[goals.<id>]` table asserts a condition over a *read locus* (a
filesystem path or `steps.<id>.output`):

```toml
[goals.has_sources]
evaluates = "/workspace/report.md"   # what to inspect
check     = "matches"                # predicate from the menu
pattern   = "(?m)^## Sources"       # required for "matches"
```

### Predicate menu

| `check` value | What it tests | Extra field required |
|---|---|---|
| `exists` | The locus resolves (file present, step output set) | — |
| `non_empty` | Resolves and contains at least one non-whitespace byte | — |
| `equals` | Content equals a literal string | `equals = "..."` |
| `matches` | Content matches a regex | `pattern = "..."` |
| `min_count` | Number of non-empty lines is at least N | `min_count = N` |
| `schema_valid` | Content parses as JSON and validates against a schema | `schema = { ... }` |

`tau check` verifies the regex compiles at build time — a bad regex is a
build error, not a surprise at runtime.

### Native-fn escape hatch

When the predicate menu is not expressive enough, use a registered native
Rust fn:

```toml
[goals.link_health]
evaluates = "steps.writer.output"     # a named step output, not a file
fn        = "research_checks::all_links_resolve"
```

`fn` and `check` are mutually exclusive. The fn name must be registered
in the host `DeterministicRegistry` under that exact key.

### Locus variants

- **Filesystem path** — any absolute string not starting with `steps.`
  (e.g. `"/workspace/report.md"`).
- **Named step output** — `steps.<id>.output` where `<id>` is a pipeline
  step id (e.g. `steps.writer.output`). The value is the step's text
  output captured in the output store.

---

## Step 2 — declare a deliverable

A `[deliverables.<id>]` table asserts an artifact is both produced and
acceptable:

```toml
[deliverables.report]
path         = "/workspace/report.md"        # the artifact locus
must_satisfy = "A coherent summary that accurately reflects the sources."
```

`must_satisfy` is always required — it is the acceptance criterion fed
to the judge.

### Locus

Exactly one of:

- `path = "<absolute-path>"` — a filesystem file.
- `output = "steps.<id>.output"` — a named step output.

Setting both, or neither, is a build error.

### Judge resolution

The judge evaluates the artifact's *content* against `must_satisfy` and
returns `{ "met": bool, "rationale": string }`. Three effort levels:

| Level | What you write | Who judges |
|---|---|---|
| Easy | just `must_satisfy` | tau's built-in minimalist judge |
| Tune | `must_satisfy` + `judge_model = "..."` | built-in judge, chosen model |
| Power | `must_satisfy` + `judge = "my_agent"` | a `[agents.*]` you defined |

**Honest limit:** `judge_model` is parsed, validated, and stored, but is
a **runtime no-op in v1** — the runtime runs all judges on the ambient
backend. Per-judge model selection requires multi-backend resolution that
does not exist yet. See [ADR-0044](../decisions/0044-deliverables-and-goals.md).

`judge` and `judge_model` are mutually exclusive — a custom agent brings
its own model, and setting both is a build error. If you use
`judge = "my_agent"`, that agent must be defined in `[agents.*]`.

The judge reads the **actual produced artifact** (the file contents or
named output value) at runtime — it judges the real deliverable, not an
in-memory stub.

---

## Step 3 — declare a producer

A deliverable binds to a **producer**: an agent that explicitly declares
it writes the deliverable's locus. The `produces` field on an agent makes
this declaration:

```toml
[agents.writer]
model         = "claude-haiku-4-5"
prompt.system = "Write /workspace/report.md from the notes."
tool_refs     = ["write_file"]
produces      = ["/workspace/report.md"]   # explicit intent declaration

[tools.write_file]
native       = "WriteFile"
capabilities = [{ kind = "fs.write", paths = ["/workspace/**"] }]
```

`tau check` enforces two build-time guarantees:

1. **No producer** — the deliverable's locus must appear in exactly one
   agent's `produces` list. Zero producers is a build error; more than
   one is ambiguous and also a build error.
2. **Capability coverage** — the producer must hold an `fs.write`
   capability whose glob covers the declared path. A mismatch is a build
   error before any token is spent.

For `output` loci (named step outputs), no `fs.write` capability is
required — there is no filesystem path to cover.

---

## Step 4 — failure handling: abort or retry

By default a failed check aborts with the rationale. To add the
self-correcting retry loop:

```toml
[deliverables.report]
path         = "/workspace/report.md"
must_satisfy = "A coherent summary that accurately reflects the sources."
on_fail      = "retry"     # default is "abort"
max_attempts = 3           # default 3 when on_fail = "retry"
retry_from   = "writer"    # the gate; default = the bound producer
```

### How the retry loop works

```
          rewind to gate, inject "why"
          |
  [gather] --> [writer] --> [check]
     ^              |
     gate can be    producer = default gate
     moved here
```

On failure, execution rewinds to `retry_from` and re-runs forward
through the producer to the check again. The failure `rationale` from
the previous verdict is injected as an extra turn into every **agent**
step inside the retry span — so attempt 2 of `writer` literally sees:
*"previous attempt rejected: only 1 source cited, need >=2."*

Deterministic steps inside the span receive no injection — they are pure
functions and see the same result regardless.

### Gate position rules (build-time checks)

`tau check` enforces two guarantees at build time:

- **Guarantee 1 — gate is at or before the producer.** A gate after the
  producer cannot change the artifact (the producer already ran); it is
  a build error:
  ```
  error: deliverable 'report' has retry_from = "polish"
         but 'polish' runs after producer 'writer'
         — the gate must be at or before the producer
  ```
- **Guarantee 2 — the span has a non-deterministic step.** If every step
  between the gate and the check is deterministic, retrying will produce
  identical output indefinitely — tau rejects this at build time:
  ```
  error: deliverable 'report' sets on_fail = "retry" but the retry span
         contains no non-deterministic step; retrying cannot change the result
  ```

### Budget bounding

The retry loop is bounded by `max_attempts` **and** by the existing
`AgentBudget` on each agent in the span. The budget cap is authoritative
even below `max_attempts` — a stubborn judge or producer cannot burn
unbounded tokens.

---

## Step 5 — the trace

Every check evaluation emits a structured event in the JSONL run log and
any OTLP export:

```jsonc
{ "event": "check.evaluated", "id": "has_sources", "kind": "goal",
  "verdict": "pass", "attempt": 1 }
{ "event": "check.evaluated", "id": "report", "kind": "deliverable",
  "verdict": "fail", "attempt": 1, "locus": "/workspace/report.md",
  "rationale": "only 1 source cited; need >=2" }
{ "event": "check.retry",     "id": "report", "rewind_to": "writer",
  "next_attempt": 2 }
{ "event": "check.evaluated", "id": "report", "kind": "deliverable",
  "verdict": "pass", "attempt": 2 }
```

The full retry history — with the rationale at each attempt — is
durably in the trace, not only the final outcome.

---

## Worked example

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
capabilities = [{ kind = "fs.write", paths = ["/workspace/**"] }]

[[pipeline.steps]]
id    = "gather"
run   = "agent:gather"
input = "${input}"

[[pipeline.steps]]
id    = "writer"
run   = "agent:writer"
input = "${steps.gather.output}"

[goals.has_sources]                      # deterministic — no LLM
evaluates = "/workspace/report.md"
check     = "matches"
pattern   = "(?m)^## Sources"

[deliverables.report]                    # LLM judges the content
path         = "/workspace/report.md"
must_satisfy = "A coherent summary that accurately reflects the sources."
on_fail      = "retry"
max_attempts = 3
retry_from   = "writer"
```

What happens:

- `tau check` proves: `report` has a declared producer (`writer`)
  permitted to write the path; `retry_from = "writer"` is at the
  producer (valid gate); the span `gather → writer` contains an LLM
  step; the `has_sources` regex compiles. Zero tokens spent.
- `tau run` attempt 1: `writer` writes a report citing one source →
  judge fails → rewinds to `writer` with *"only 1 source cited; need
  >=2"* → attempt 2 cites two sources → judge passes → `has_sources`
  predicate passes → run succeeds. Every verdict in the trace.

### Placement

Checks are appended to the pipeline tail by default (goals first, then
deliverables, both in alphabetical order by id). You can place a check
at an earlier checkpoint by inserting an explicit step:

```toml
[[pipeline.steps]]
id  = "mid_check"
run = "check:has_sources"
input = "${steps.gather.output}"    # unused by checks; required by the schema
```

An explicitly-placed check is not appended again.

---

## What `tau check` validates

| Check | Error |
|---|---|
| Deliverable has no producer | `DeliverableNoProducer` |
| Multiple agents claim the same locus | `DeliverableAmbiguousProducer` |
| Producer lacks `fs.write` capability covering the path | `DeliverableProducerLacksCapability` |
| `retry_from` names a step that does not exist | `UnknownRetryFrom` |
| Gate runs after the producer | `GateAfterProducer` |
| Retry span has no non-deterministic step | `RetrySpanNoLlm` |
| `goal` regex does not compile | `BadGoalRegex` |
| `judge` and `judge_model` are both set | `JudgeAndModelConflict` |
| `judge` agent is not defined | `UnknownJudgeAgent` |

These are all build-time errors caught before any run.

---

## See also

- [ADR-0044](../decisions/0044-deliverables-and-goals.md) — architecture
  decisions behind this feature (Check-not-Node, check-as-pipeline-step,
  honest limits).
- [Workflows](../explanation/workflows.md) — the IR pipeline model.
- [Project manifest schema](../reference/project-manifest-schema.md) —
  full field reference for `goals`, `deliverables`, and `produces`.
- [Escape hatches](../explanation/escape-hatches.md) — how native-fn
  registration works.
