# Workflow IR design (Phase α.1 — Framing D resolution)

**Status:** Design spec. Locks D-1 through D-7b enumerated in
[`2026-05-29-framing-d-workflow-ir.md`](2026-05-29-framing-d-workflow-ir.md).
Closes Phase α.1. Gates Phase β.2 (workflow-IR implementation per
[ROADMAP.md](../../../ROADMAP.md) §β.2).

**Date:** 2026-05-31.

**Audience:** β.2 implementers and downstream surface authors (TS sugar
in β.8 / δ.2; AOT lowering in β.7 / γ.x).

**Relates to:**
[`tau-philosophy.md`](../../explanation/tau-philosophy.md) (the
compiler thesis the IR realizes),
[`2026-05-29-framing-d-workflow-ir.md`](2026-05-29-framing-d-workflow-ir.md)
(the scoping doc this spec resolves),
[`2026-05-30-tau-runtime-core-design.md`](2026-05-30-tau-runtime-core-design.md)
(β.1 kernel that the v0 interpreter lives in).

---

## In one breath

The tau workflow IR is a typed, content-hashed intermediate representation
emitted by `tau build` from a `tau.toml` source. v0 (β.2) ships a
partial-interpret lowering — the IR travels as data inside a wasm
component whose code is the `tau-runtime-core` interpreter. v1 (β.7 / γ.x)
swaps the lowering for AOT: the same IR is lowered to per-workflow wasm
code with no interpretation step. The IR shape is designed AOT-ready from
day one. Capabilities lower as WASI imports for what WASI models and a
tau-owned custom section for what it doesn't (exec, hardware, per-host
allowlist). `tau build --target T` refuses to build when T can't enforce
the workflow's declared capabilities — no override flag, matching
Rust-class build-time discipline. The conformance contract asserts
multiset side-effect equivalence between dev-mode and bundle-mode for
the same workflow.

---

## Decisions

The framing doc enumerates seven decisions plus one sub-decision the
brainstorming surfaced. Each is locked below with its rationale.

### D-1 — Node taxonomy: typed full (Agent + Tool + Deterministic + Subflow)

The IR has four typed node variants:

```rust
pub enum Node {
    Agent        { … },     // LLM loop with tools and optional context block
    Tool         { … },     // native (Rust impl) or MCP (external server)
    Deterministic { … },    // pure function reference; no LLM, no I/O
    Subflow      { … },     // compose flows: spawn / call into another agent or workflow
}
```

**Rationale.** The framing doc's risk section recommended starting with
two variants (Agent + Tool) on minimal-IR grounds. We instead commit to
four. Reason: tau's philosophy doc explicitly scopes both "AI agents"
and "workflow automation"; the n8n / Temporal class needs `Deterministic`
and `Subflow` from day one. Adding them later would force an IR-version
bump for a use case we already declared in-scope.

**Cost.** Four lowering paths instead of two. The conformance suite
(D-7b) must cover all four from v0 — they ship under conformance, not
behind future work.

### D-2 — Message wire: new `tau_ir::Message` + adapter

The IR's inter-node wire format is a new type owned by the IR crate,
with bidirectional conversion to `tau_domain::Message`:

```rust
// crates/tau-ir/src/message.rs (new)
pub struct Message {
    pub id: MessageId,
    pub sender: Address,
    pub recipient: Address,
    pub parent_id: Option<MessageId>,
    pub created_at_ms: i64,                   // normalized: SystemTime → i64-ms
    pub headers: BTreeMap<String, String>,
    pub payload: MessagePayload,
}
```

**Conservative migration rule.** `tau_ir::Message` MUST include every
semantic field from `tau_domain::Message`. The only permitted change is
type normalization (`SystemTime → i64-ms`, matching the β.1 Clock port
pattern). Any field omission requires an inline rationale in the spec
and an entry in the IR change-log.

**Rationale.** A clean separation between the agent-runtime envelope
type (`tau_domain::Message`) and the IR wire type lets the IR own its
own evolution (versioning via `ir_format`, see D-6) without touching
domain types other crates consume. The β.1.5 vocabulary drift test
shipped a precedent: cross-crate "this mirror MUST track that source"
testing is cheap to write and catches every drift.

**The risk we accept.** "Thin too thin." If we drop a field from
`tau_ir::Message` that turns out to matter for IR semantics, we'd need
an `ir_format` bump to add it back. The conservative migration rule
above mitigates by forbidding silent drops.

### D-3 — Capability lowering: WASI imports + tau custom-section metadata

The wasm artifact a workflow lowers to declares its capability needs in
two complementary places:

| Where | What lives there |
|---|---|
| **WASI imports** on the component interface | filesystem handles (`wasi:filesystem`), sockets (`wasi:sockets`), and other capabilities WASI models directly |
| **`tau.caps` custom section** in the wasm bytes | per-tool capability metadata for what WASI doesn't model: process exec (`exec.allowed_binaries`), hardware ports (`hardware = ["i2c:0x48", "gpio:21"]`), per-host network allowlists (`net.host_allowlist = ["api.weather.com"]`) |

**Runtime enforcement (summary).** A tau host reads the custom section at
component load, builds an `AmbientOpsGate` keyed by `(tool_id, op)`, and
gates ambient operations against the metadata. A standard WASI runtime
(no tau awareness) skips the custom section gracefully — but components
that import `tau:host/exec` or `tau:host/hardware` fail to instantiate
in non-tau hosts because the imports cannot be satisfied. This is loud
and honest, not silent and degraded.

**What "runs in standard runtimes" actually means.** Only workflows
whose tools fit within WASI's vocabulary (filesystem + sockets) are
genuinely portable to non-tau WASI hosts. Workflows reaching for
`tau:host/exec` or `tau:host/hardware` are tau-only at instantiation.
The per-workflow tradeoff is the author's, expressed via tool choice in
`tau.toml`.

### D-3b — Strict build-time refusal, no override flag

`tau build --target T` validates the workflow's `CapabilityTable`
against `T`'s declared `supported_shapes` (from the target-triple
registry shipped in Phase 2 §B). If any required shape is not
supported, the build refuses with a diagnostic naming the conflict.
**There is no `--allow-loose-enforcement` flag, ever.** The author
changes tool, changes target, or waits for a future tier that admits
the capability.

**Rationale.** Per `feedback_tau_rust_like_build_enforcement` — tau is
to enforce at build time the way Rust does, leaving no holes that
appear at runtime. The wasm component's instantiation failure (option
1's loud runtime error) is not a sufficient substitute for a build-time
refusal: the build artifact would have already been published, the CI
would have already passed, and the failure would manifest only at
deploy. The Rust analog is `#![no_std]` — there is no
`--allow-libc-on-no-std` flag; the user rewrites or picks a different
target.

**Diagnostic shape:**

```
$ tau build --target wasm-wasi-only
  error: workflow `fan-monitor` declares tool `read_temp` requiring
         capability shape `Hardware`; target `wasm-wasi-only` does
         not support this shape.
  help:  change `[tools.read_temp]` to use an MCP server, or build
         for a target that supports `Hardware` (e.g. wasm-wasi-tier1,
         bare-metal-esp32-c3-passthrough).
  note:  tau does not provide an override flag for capability fit;
         see docs/decisions/0037-workflow-ir.md (planned).
```

### D-4 — Composition: one monolithic component per workflow

Each tau workflow lowers to a single wasm component. Tools are linked
into the same component (statically for native tools; an MCP-proxy
stub for MCP tools). Per-tool capability isolation is enforced by the
host's `AmbientOpsGate` against the custom-section metadata — not by
component-graph boundaries.

**Rationale.** tau owns all in-process tool code (philosophy doc:
"tau does not package MCP server code"). The defense-in-depth value of
component-graph isolation only applies to *adversarial in-process tool
code*, which tau explicitly does not host. The dividends of monolithic
composition — smallest payload, best whole-program tree-shake, simplest
hash, cheapest embedded fit — are large; the cost of giving up
component-graph isolation is nil for tau's actual threat model.

### D-5 — Lowering strategy: phased (interpret v0 in β.2, AOT v1 at β.7 / γ.x)

> *Throughout this section "v0" and "v1" refer to the **lowering strategy
> phase**, not to the `ir_format` field (D-6). The IR format version stays
> `v1.0.0` across both lowering phases — the IR shape is unchanged; only
> the way `tau build` emits the artifact differs.*

**v0 (β.2):** Partial-interpret. `tau build` emits a wasm component
whose code is the `tau-runtime-core` interpreter and whose data is the
canonicalized IR bytes (carried as a wasm data segment or
custom-section payload). One interpreter runs in both dev mode
(callbacks-for-tools) and bundle mode (interpreter-in-wasm). Behavior
is identical across modes because there's only one interpreter
implementation.

**v1 (β.7 / γ.x):** AOT. The same IR lowers to per-workflow wasm code
with no interpretation step. Each IR node type has a codegen path. The
IR is designed AOT-ready from v0 (typed nodes, statically resolvable
references, no dynamic codegen tricks in the v0 interpreter that
would resist static lowering).

**Why phased.** The framing doc's mitigation ("ship a minimal IR first
… extend deliberately") assumed two node variants. With D-1's four
variants, an AOT v0 implementation is a multi-month commitment whose
correctness is hard to validate without a working interpreter to
compare against. Phased delivery ships semantics first, codegen second
— the path `rustc` itself took (OCaml interpreter before self-hosting).

**Migration commitment.** β.7 SHALL deliver the AOT lowering. v0
artifact bytes are not stable across the v0→v1 transition (different
wasm bytes for the same source); the `tau_version` bump at v1 captures
the change.

### D-6 — Determinism contract: per-target hashing; `ir_format` separate from `tau_version`

`tau verify --bundle` (shipped 2026-05-28) requires byte-identical
rebuilds for the same source on the same target with the same compiler.
The IR upholds this with the following inputs:

**In the hash:**
- Canonical-form `tau.toml` content (semantic, not whitespace/key-order)
- Agent definitions: prompt text, model, tools list, context block
- Per-tool capability declarations
- Native tool impl reference + impl content hash
- MCP contract: url + declared capability subset + (if pinned) protocol version
- Skill version pins (content hash, per Skills-5; not version string)
- Target triple (from the target-triple registry, Phase 2 §B)
- `ir_format` (the IR language version — new field, see below)
- `tau_version` (the compiler binary version — inherited from current bundle)

**Not in the hash (cosmetic / environmental):**
- TOML formatting, comments, key ordering within the same table
- Source-code comments in native tool impls (content hash covers source bytes; comment-only changes are picked up there honestly)
- Build timestamps, build host, build user, env vars not affecting output
- Build flags that don't affect lowered output (sccache wrapper, etc.)

**Canonical-form rules.**

1. Deserialize `tau.toml` to typed structs; re-serialize in canonical
   field order (alphabetical within each table) before hashing. No
   whitespace or comment leakage.
2. Inner maps use `BTreeMap` (alphabetical iteration; workspace
   convention).
3. Timestamps are `i64`-ms (D-2 decision); no `SystemTime` participates
   in the hash.
4. Optional fields with default values normalize to "absent" before
   hashing (omitting a field equals setting it to the default).

**`ir_format` vs `tau_version`.**

The IR carries TWO version signals:

- `ir_format` — the IR *language* version, semver-shaped (currently
  `"v1.0.0"`). Bumps follow semver rules: MAJOR for breaking shape
  changes (removed node type, removed required field, changed lowering
  contract); MINOR for additive changes (new optional field, new
  variant of a `#[non_exhaustive]` enum); PATCH for spec-only edits
  with no IR-shape effect.
- `tau_version` — the IR *compiler* version. Bumps with every tau
  release.

Both are in the bundle hash. Rationale: the Rust precedent —
edition (the language version) is separate from rustc version (the
compiler). Decoupling them lets tau patches (security, perf, bug fixes
that don't touch the IR semantic) avoid invalidating every existing
bundle's hash, and gives future multi-format readers a dispatch key.
The cost is one field + one drift test, modeled on β.1.5's vocabulary
drift test.

### D-7a — Conformance level: multiset side-effect equivalence

The cross-mode conformance gate (C3 invariant) asserts equivalence
between dev-mode and bundle-mode for the same workflow, defined as:

```
dev-mode RunOutcome ≡ bundle-mode RunOutcome    iff

  1. Same final AgentStatus and same final assistant text
  2. Same MULTISET of tool calls — each tool was called N times with
     the SAME (name, args, result) tuples. Order does NOT matter;
     concurrency does NOT matter.
  3. Same MULTISET of message-added events — same message bodies.
     Order does NOT matter.
  4. Same total token usage (when LLM is deterministically mocked).
```

**What this catches.** Argument drift (dev called fs.read("/tmp/a"),
bundle called fs.read("/tmp/b")). Multiplicity drift (dev called tool
twice, bundle once). Result drift (same call, different result).
Outcome drift.

**What this tolerates.** Internal-trace divergence (interpreter and
AOT emit different spans by design). Reordering when the workflow's
tool dispatch is legitimately concurrent. Streaming-vs-batch internal
mechanism differences.

**Industry precedent.** This pattern matches the wasm test suite
(wasmtime/WAMR), ECMAScript test262, and the WASI conformance runners:
*conformance is on observable side effects at the host boundary, not
on internal execution order.* Temporal's "replay determinism" is a
different problem (single-runtime persistence-driven replay) and does
not apply to cross-mode equivalence.

### D-7b — Conformance suite scope: ~6 fixtures (one per node-type × capability-shape)

The v0 suite ships six fixtures, exercising each D-1 node variant
under at least one capability shape:

| # | Fixture | Node types | Capabilities |
|---|---|---|---|
| 1 | `01_agent_native_tool` | Agent + Tool(native) | Filesystem (read) |
| 2 | `02_agent_mcp_tool` | Agent + Tool(mcp) | Network (host-allowlisted) |
| 3 | `03_agent_denied_capability` | Agent + Tool | Refused at build time (verifies D-3b) |
| 4 | `04_subflow_spawn_child` | Agent + Subflow → Agent | Subset-of-parent caps |
| 5 | `05_deterministic_step` | Deterministic | (no I/O capability) |
| 6 | `06_multi_turn_history` | Agent × 3 turns | History accumulation; messages multiset |

Each fixture:
- ~50 LOC of `tau.toml` + a deterministic mock LLM script + an expected
  outcome JSON (`{ run_outcome, tool_call_multiset, message_multiset }`).
- Runs in dev-mode and bundle-mode; conformance runner compares.
- CI cost: a few seconds total (mocked LLM; native tools; no external
  network).

**Future fixtures (not in v0).** Multi-turn workflows with denied tools;
subflows of subflows; deterministic chains; concurrency / parallel tool
dispatch. Each added as the corresponding β.X work lands.

---

## The IR shape

A new `tau-ir` crate (`no_std` + `alloc`, like `tau-runtime-core`) owns
the IR types. `tau-runtime-core` depends on `tau-ir` for the
interpreter loop; future AOT codegen (β.7) and TS sugar (β.8 / δ.2)
also depend on `tau-ir`.

```rust
// crates/tau-ir/src/lib.rs
#![no_std]
extern crate alloc;

pub struct IrModule {
    pub ir_format:   IrFormatVersion,             // semver-shaped "v1.0.0"
    pub tau_version: SemVer,
    pub target:      TargetTriple,                // tau_ports::target
    pub workflow:    Workflow,
}

pub struct Workflow {
    pub agents:            BTreeMap<AgentId, Agent>,
    pub tools:             BTreeMap<ToolId, Tool>,
    pub steps:             BTreeMap<StepId, Deterministic>,
    pub edges:             Vec<Subflow>,
    pub capability_table:  BTreeMap<ToolId, CapabilityRequirements>,
}

pub enum Node {
    Agent {
        id:         AgentId,
        prompt:     String,
        model:      String,                       // e.g. "claude-haiku-4-5"
        tool_refs:  Vec<ToolId>,
        context:    Option<ContextConfig>,        // β.4 surface; optional
        budget:     AgentBudget,
    },
    Tool {
        id:           ToolId,
        impl_:        ToolImpl,
        capabilities: CapabilityRequirements,     // shapes + scopes
        spec:         ToolSpec,                   // name, description, input schema
    },
    Deterministic {
        id:            StepId,
        fn_ref:        NativeFnRef,               // resolved at build; statically linked
        input_schema:  Value,
        output_schema: Value,
    },
    Subflow {
        id:    SubflowId,
        kind:  SubflowKind,
    },
}

pub enum ToolImpl {
    Native { name: String, content_hash: Hash256 },
    Mcp    { url: Url, contract_hash: Hash256, capability_subset: CapabilityRequirements },
}

pub enum SubflowKind {
    Spawn   { target_agent: AgentId, cap_subset: CapabilityRequirements },
    Compose { target_workflow: Box<IrModule> },
}

// inter-node wire (D-2)
pub struct Message {
    pub id:            MessageId,
    pub sender:        Address,
    pub recipient:     Address,
    pub parent_id:     Option<MessageId>,
    pub created_at_ms: i64,
    pub headers:       BTreeMap<String, String>,
    pub payload:       MessagePayload,
}

// adapter (also in tau-ir; not in tau-domain to keep tau-domain free
// of IR-specific code)
impl From<tau_domain::Message> for Message { /* ... */ }
impl From<Message> for tau_domain::Message { /* ... */ }
```

### A minimal IR example

A single workflow covering all four node types:

```toml
# tau.toml (the source)

[agent.monitor]
prompt = "Read temperature; run fan if >30°C; alert if >50°C."
model  = "claude-haiku-4-5"
tools  = ["read_temp", "set_fan", "alert"]

[tools.read_temp]
native       = "ReadTemp"
capabilities = { hardware = ["i2c:0x48"] }

[tools.set_fan]
native       = "SetFan"
capabilities = { hardware = ["gpio:21"] }

[tools.alert]
subflow = "alerter"

[steps.normalize]
deterministic = "parse_celsius"
input         = "tool_result(read_temp)"

[agent.alerter]
prompt = "Send a critical-temperature alert."
model  = "claude-haiku-4-5"
tools  = ["page"]

[tools.page]
mcp          = "pagerduty"
capabilities = { network = ["api.pagerduty.com"] }
```

The lowered IR (canonical form, abbreviated):

```text
IrModule {
  ir_format:   "v1.0.0",
  tau_version: "0.X.Y",
  target:      TargetTriple { os: "wasi", arch: "wasm32", tier: 1 },
  workflow: Workflow {
    agents: {
      "alerter": Agent { prompt: "...", model: "claude-haiku-4-5",
                         tool_refs: ["page"], context: None, budget: {...} },
      "monitor": Agent { prompt: "...", model: "claude-haiku-4-5",
                         tool_refs: ["read_temp", "set_fan", "alert"],
                         context: None, budget: {...} },
    },
    tools: {
      "page":      Tool { impl_: Mcp { url: "...", contract_hash: 0xAA…,
                                       capability_subset: { network: [api.pagerduty.com] } },
                          capabilities: { network: [api.pagerduty.com] }, spec: {...} },
      "read_temp": Tool { impl_: Native { name: "ReadTemp", content_hash: 0xBB… },
                          capabilities: { hardware: [i2c:0x48] }, spec: {...} },
      "set_fan":   Tool { impl_: Native { name: "SetFan", content_hash: 0xCC… },
                          capabilities: { hardware: [gpio:21] }, spec: {...} },
    },
    steps: {
      "normalize": Deterministic { fn_ref: "parse_celsius",
                                   input_schema: {...}, output_schema: {...} },
    },
    edges: [
      Subflow { kind: Spawn { target_agent: "alerter",
                              cap_subset: { network: [api.pagerduty.com] } } },
    ],
    capability_table: { … },                  // derived; consumed by AmbientOpsGate
  },
}
```

Two build invocations show D-3b in action:

```text
$ tau build --target wasm-wasi-tier1
  ✓ all capability shapes supported (Network, Hardware)
  ✓ ir_format=v1.0.0; tau_version=0.X.Y
  ✓ canonical-form IR bytes: 0xDEAD…
  ✓ wasm artifact bytes:     0xBEEF…
  ✓ bundle written; verify-friendly

$ tau build --target wasm-wasi-only
  error: workflow `monitor` declares tool `read_temp` requiring
         capability shape `Hardware`; target `wasm-wasi-only` does
         not support this shape.
  …
```

---

## Build pipeline

```
tau.toml + native tool sources + skill pins + capability_override
         │
         ▼  [1] parse → typecheck → resolve external refs (skills, MCP)
   IrModule (typed, in-memory)
         │
         ▼  [2] capability fit check vs target  (D-3b — refuse on miss)
         │
         ▼  [3] determinism canonicalization     (D-6)
   IrModule + canonical bytes
         │
         ▼  [4] v0 STRATEGY: package as data
             {
               wasm  = interpreter binary (compiled from tau-runtime-core
                                            + tau-ir, statically linked),
               data  = canonical IR bytes,
               custom_section = "tau.caps" capability metadata,
               imports = WASI handles + tau:host/{exec,hardware} if used,
             }
         │
         ▼  [5] hash inputs:
             { canonical IR + tau_version + ir_format + target }
         │
         ▼
   workflow.taubundle (Phase 2 §C.2 format, extended with `ir_payload`)
```

The dev mode (`tau dev`) takes the same `IrModule` and runs it through
the same interpreter, but with tools dispatched as in-process callbacks
instead of through the wasm boundary. The interpreter is one
implementation; modes differ only in how tool calls are routed.

---

## Determinism contract details

Beyond the inputs enumerated in D-6:

**Canonical-form invariants** (re-asserted by a dedicated test crate):
1. Re-parsing canonical bytes and re-serializing produces the same
   canonical bytes (idempotence).
2. Permuting the source `tau.toml` (whitespace, key order, comments)
   produces the same canonical bytes (insensitivity to cosmetics).
3. Re-building the same source on the same target with the same
   tau version produces the same artifact bytes
   (reproducibility — this is what `tau verify --bundle` checks).

**Future-compat note.** Adding a new field to `Node::Agent` (e.g.
`max_tool_calls`) bumps `ir_format` (v1.0.0 → v1.1.0 if the field is
optional with a default; v2.0.0 if required). The drift test (modeled
on β.1.5's vocabulary drift) detects accidental additions: a field
appears in `IrModule` but `ir_format` was not bumped.

---

## Conformance contract details

A new `tau-ir-conformance` test crate holds the fixtures and the
runner. The runner is generic over execution mode (`dev` vs `bundle`):

```rust
trait ExecutionMode {
    async fn run(&self, ir: &IrModule, mock_llm: &MockLlmScript)
        -> ConformanceReport;
}

struct ConformanceReport {
    run_outcome:        RunOutcome,
    tool_calls:         BTreeMap<(String, Value), u32>,    // multiset
    message_added:      BTreeMap<MessageBody, u32>,        // multiset
    token_usage:        TokenUsage,
}

fn assert_conform(dev: &ConformanceReport, bundle: &ConformanceReport) {
    assert_eq!(dev.run_outcome, bundle.run_outcome);
    assert_eq!(dev.tool_calls,    bundle.tool_calls);
    assert_eq!(dev.message_added, bundle.message_added);
    assert_eq!(dev.token_usage,   bundle.token_usage);
}
```

Each fixture is a directory:

```text
crates/tau-ir-conformance/fixtures/
  01_agent_native_tool/
    workflow.toml
    mock_llm.jsonl              # deterministic LLM responses
    expected_outcome.json       # the report multisets, pre-baked
  02_agent_mcp_tool/
    ...
```

CI runs the conformance suite on every PR.

---

## Out of scope

Carried forward from the framing doc, unchanged:

- Runtime semantics of `agent.<kind>.spawn` beyond the IR shape
  (multi-agent v2 work — separate spec).
- Workflow durability / persistence beyond replay (Temporal-class
  semantics — separate spec).
- TS sugar surface (β.8 / δ.2; lowers to this IR, designed later).
- MCP facilitator wire-level adapter choices (β.3).
- AOT codegen design (β.7; this spec only commits the IR will be
  AOT-ready, not how the codegen is structured).

Added by this spec:

- Per-target wasm component-model evolution (γ.x territory; if the
  component model lands on MCU during γ, we revisit D-4 for the
  embedded tier).
- Forward-compat reader policy (whether tau N can read bundles from
  tau N-1 / N-2). The `ir_format` field enables this; the policy
  itself is deferred to a v1.1 spec.

---

## Risks acknowledged

**The minimal-IR mitigation no longer fully applies.** The framing
doc's risk acknowledgement recommended starting with two node variants
(Agent + Tool). By committing to four, we accept a larger initial
lowering surface and a larger conformance suite. Mitigation: the
phased lowering (D-5) keeps the v0 implementation cost contained —
interpreting four typed variants is straightforward; the AOT codegen
cost lands at v1 when we have a working semantic kernel to validate
against.

**`tau_ir::Message` may be too thin.** A field we drop from the
`tau_domain::Message` envelope may turn out to matter for IR
semantics. Mitigation: the conservative migration rule (D-2) forbids
silent drops; a drift test asserts the field correspondence on every
build.

**The `tau.caps` custom section is a tau-owned vocabulary.** If WASI
later models exec / hardware in standard ways, we'd want to migrate
those gates from the custom section to standard WASI imports.
Mitigation: the custom section is *data*, not a WIT contract; the
migration is an `ir_format` bump that re-emits artifacts with new
import shapes, no source change required.

**Phased lowering means v0 and v1 bytes differ.** Bundles built under
v0 won't byte-match v1 rebuilds. Mitigation: the `tau_version` bump at
v1 captures this; `tau verify --bundle` for a v0 bundle correctly
identifies it as built under a specific tau version + ir_format. v1
tau can choose to support reading v0 bundles for execution (via the
v0 interpreter, kept around as a code path) — that compat policy
lands in a v1.1 spec, per the out-of-scope list.

---

## Deliverable shape

This spec is one of three artifacts that close Phase α.1:

| # | Artifact | Status |
|---|---|---|
| 1 | This design spec | landing with this PR |
| 2 | ADR-0037 (or next) — Workflow IR commitment | follow-up PR; one-page record of the locked decisions, linking back to this spec |
| 3 | β.2 implementation plan | next session via `superpowers:writing-plans`; references this spec for the design |

The minimal IR example in §"A minimal IR example" is committed
in-tree (this PR) as the concrete shape reference for β.2 implementers.

Until artifacts 2 and 3 land, β.2 implementation has not started.

---

## Implementation hand-off (preview of β.2 plan structure)

The β.2 plan, written next, will decompose the IR implementation into
PR-sized phases. A preview of the likely shape:

- **β.2.1** — `tau-ir` crate scaffolding (Cargo.toml, lib.rs, the type
  definitions enumerated in §"The IR shape").
- **β.2.2** — `tau.toml` → `IrModule` lowering (parse, typecheck,
  resolve, capability-fit check).
- **β.2.3** — Canonicalization + hashing (D-6 inputs; canonical-form
  test crate).
- **β.2.4** — v0 interpreter: extend `tau-runtime-core` to drive an
  `IrModule` instead of (only) the existing `Runtime` graph.
- **β.2.5** — Bundle integration: extend the Phase 2 §C.2 bundle
  format with an `ir_payload` section; wire `tau build` to emit and
  `tau run --bundle` to load.
- **β.2.6** — Conformance suite (`tau-ir-conformance`): the six
  fixtures from D-7b.
- **β.2.7** — Docs + ADR-0037: spec footnote, ADR commit, mdBook
  build.

Sequencing is mostly linear (each phase depends on the previous); the
conformance suite (β.2.6) can develop fixtures in parallel with the
interpreter (β.2.4) once the IR types (β.2.1) stabilize.
