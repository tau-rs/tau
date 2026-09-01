# tau vision roadmap — epics & stories

Derived from the 2026-06-20 roadmap-challenge session. Source of record:
`.context/SESSION-SYNTHESIS.md` (working notes) — this doc is the committed,
implementable backlog. This is the **active backlog**; for the historical phase
record (Phase 0/1/α/β/γ/δ) see [`../../../ROADMAP.md`](../../../ROADMAP.md).

**Vision (one line, re-pointed 2026-09-01 per the redesign):** tau compiles agent
definitions into sealed artifacts you can prove things about — declare what agents are
*allowed* in a root `tau.toml` constitution, author in three surfaces (TOML declares the
vocabulary, TypeScript choreographs the flow, Rust implements the muscle — one validator,
one frozen content-hashed IR underneath), and tau proves behavior ⊆ constitution at build
time and emits one hardware-agnostic, capability-bounded, conformance-proven component you
can test deterministically, review in CI, and run from any language. (Previous framing
"build workflows beautifully in any language (generated typed SDKs or the tau-native
DSL)" superseded by the three-surface split — ADR-0071, design
[`2026-09-01-tau-authoring-ops-and-primitives-design.md`](../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md) §1.)

**How to use this doc:** epics = milestones; stories = one PR each (acceptance criteria +
crates touched); when a story is picked up, generate its TDD plan via the `writing-plans`
skill into `docs/superpowers/plans/<story-id>.md`, execute via `executing-plans`.

---

## Sequencing (the DAG)

```
EPIC 0  Core no_std readiness (de-std run loop)        ◀── BLOCKS 3,4,5,7
EPIC 2  Lock the two contracts (public ABIs+codegen)   ◀── BLOCKS 3,5   [constitutional]
EPIC 1  Root [allow] governance + build enforcement    ◀── BLOCKS 3; informs 4 [constitutional]
EPIC 8  Identity + positioning (ADRs/wedge/philosophy)  ── do FIRST, parallel [constitutional]
   │
   ├─ EPIC 3  Capability lowering + WIT-world gen   (needs 0,1,2)
   ├─ EPIC 4  IR control flow (blocks + dyn regions) (needs 2)
   ├─ EPIC 5  Polyglot sourcing + DX               (needs 0,2)
   ├─ EPIC 6  Durability tiers                      (A-minimal shipped; mostly independent)
   └─ EPIC 7  Embedded as component                 (needs 0,5)
```
Critical path: **0 → (2,1) → 3/4/5 → 7.** Start EPIC 8 immediately (protects the vision
against in-flight parallel work). EPIC 6 is mostly independent.

---

## EPIC 8 — Identity + positioning  [constitutional · do FIRST]
**Goal:** lock the identity, wedge, and per-target capability framing so the parallel work
(durability #373, β.7.5 #372/#369) aligns with the vision.
- **8.1** ADR: *tau = compiler+engine between two contracts; CLI = reference host.* Public
  stability surface = the two contracts; CLI verbs get a looser compat policy.
  *Accept:* ADR merged in `docs/decisions/`.
- **8.2** Rewrite `philosophy.md`: conviction 3 → per-target capability lowering (WIT on
  wasm = by construction; OS sandbox host/native; advisory bare-metal); §wedge → the locked
  text (`SESSION-SYNTHESIS.md §8.1`). *Accept:* mdbook builds; linkcheck clean.
- **8.3** Reframe `ROADMAP.md`: artifact = component + two contracts; demote tau-as-firmware
  to a Reserved niche. *Accept:* ROADMAP reflects the component-first frame.
**Epic DoD:** identity ADR merged; philosophy + ROADMAP match the vision.

## EPIC 0 — Core no_std readiness (de-std the run loop)  [BLOCKS 3,4,5,7]  ✅ SHIPPED 2026-06-22
**Goal:** `run`/`stream`/`interpreter` compile *and run* no_std (was gated behind
`tool-validation` → pulled std via `jsonschema` + `tau-domain/std`).
Spec: `docs/superpowers/specs/2026-06-22-epic-0-destd-run-loop-design.md`.
Shipped via **PR-0b (#424, merged)** + **PR-0c**.
- **0.1** ✅ Inventory every std pull in the run path (recorded in the spec §1/§4).
- **0.2** ✅ Replaced std-only `jsonschema` with a no_std JSON-Schema-subset validator
  (`tau-runtime-core::schema`, fail-closed on out-of-subset keywords; behaviour proven
  equivalent to jsonschema via a 20-pair differential before removal). PR-0b.
- **0.3** ✅ `tau-domain/std` dropped from `tool-validation`; remaining host std attached to
  the host-only `host-fs` feature (so the no_std guest build stays std-free). PR-0b.
- **0.4** ✅ `serde_json` was already alloc-only in the core (verified).
- **0.5** ✅ CI lane added: `runtime-core no-std build` job now also *runs* the loop under
  `--no-default-features --features wasm-interpreter,tool-validation` (proves validation
  executes no_std, not just compiles). PR-0c.
**Epic DoD:** ✅ the agent loop compiles + runs no_std; `tool-validation` pulls no std;
new CI step green. (Regression note: dropping the std pulls unmasked two latent feature-graph
holes — `tau-ports` `process` not pulling `serde?/std`, and `host-fs` not pulling
`tau-domain/std` — both fixed in PR-0b; the pre-existing isolated `cargo check -p
tau-runtime-core` CI guard catches this class.)

## EPIC 2 — Lock the two contracts as public ABIs (+ codegen source)  [constitutional · BLOCKS 3,5]
**Goal:** IR/authoring schema (incl. root `[allow]`) + WIT host world are minimal, versioned
public ABIs, and the source for generated SDKs.
- **2.1** ADR: the two contracts are the semver stability surface.
- **2.2** Publish the IR JSON Schema (from `tau-ir` serde) + a conformance test kit for
  frontend authors. *Accept:* schema published; a sample IR validates.
- **2.3** Generate the WIT host world from the ports (one source → no drift); freeze the
  minimal 3-function surface. *Accept:* ports↔WIT drift test green.
- **2.4** Compat/versioning policy doc for both contracts.
**Epic DoD:** both contracts published + versioned + drift-tested.

## EPIC 1 — Root `[allow]` governance + build-time enforcement  [constitutional · BLOCKS 3]
**Goal:** root `tau.toml [allow]` = the constitution; `tau check` proves caps ⊆ root.
- **1.1** ADR: root governance + build-time enforcement.
- **1.2** Config: `[allow]` capability ceiling + `[allow.mcp]`/`[allow.tools]`/`[allow.models]`
  registry. *Accept:* parses; round-trips.
- **1.3** Elevate `capability_override` + `glob_subset` from per-package to repo-root ceiling.
- **1.4** `tau check` fails if any agent/tool/region cap ⊄ `[allow]` or references an
  unregistered resource. *Accept:* over-reaching fixture fails with a clear error.
- **1.5** Enforce the lattice: root ⊇ agent ⊇ dynamic-region ⊇ spawn ⊇ tool.
- **1.6** Lint warning for coarse ceilings (`network=["*"]`).
**Epic DoD:** an over-reaching workflow fails `tau check`; lattice enforced at build.

## EPIC 3 — Capability lowering + WIT-world generation  [needs 0,1,2]
**Goal:** caps lowered per target; on wasm the WIT world is generated from allow-bounded caps.
- **3.1** Capability→WASI/WIT mapping table (network→`wasi:sockets`+allowed-hosts;
  fs→preopens; hardware→host-mediated/out-of-scope). *Accept:* table + tests.
- **3.2** Generate the WIT world from used+bounded caps at `tau build wasm`.
- **3.3** Configure host `WasiCtx` from the same caps (allowed-hosts, preopens).
- **3.4** Drop the in-guest gate on wasm; OS gate stays for host/native.
  Drops the in-guest runtime check **only for `Disposition::Wasi` caps** (the
  ABI + host `WasiCtx` own them); the in-guest gate for `Disposition::InGuest`
  caps (agent/skill.spawn, tasklist, plan) stays. See
  `docs/superpowers/specs/2026-08-09-epic-3-4-drop-in-guest-wasm-gate-design.md`.
- **3.5** `verify --bundle`: generated WIT reproducible from declared caps.
- **3.6** ✅ *(net + fs shipped)* **Guest effect ABI** — route the guest's net/fs effects through
  `wasi:http`/`wasi:filesystem` so granted imports survive wasm-ld DCE and the
  host `WasiCtx` (3.3) becomes the *live* runtime enforcement path. Closes the
  epic's **binary-observable** DoD (until effects route through WASI, DCE strips
  all WASI imports — 3.2/3.4 meet the DoD at the world-text + host-boundary layer
  only). **Prereq: 3.4** (its `Wasi`-cap gate-drop is what lets rerouted effects
  reach the host instead of being denied by the guest's empty-stub grant).
**Epic DoD:** an ungranted cap is un-importable at the ABI; wasm caps == `[allow]`-bounded set.
(*Binary*-observable half of "un-importable at the ABI" lands with **3.6**;
3.2/3.4 establish it at the world-text + host-`WasiCtx` layer.)

## EPIC 4 — IR control flow: structured blocks + dynamic regions  [needs 2]
**Goal:** Branch/Parallel/Loop/Suspend + capability-bounded dynamic regions; IR ≥ v2.3.0.

**Re-cut per-construct (D13-C, 2026-07-19).** The original 4.2–4.6 were layer-ordered
(one PR = the interpreter for *all* constructs, another = conformance for all, …). Two of
those horizontal layers shipped as **producer-without-consumer** merges — value that a user
cannot yet reach:
- **4.1 (#444, merged)** added Branch/Parallel/Loop/Suspend to the IR data model + typecheck
  only. No `tau.toml` syntax produces them.
- **4.2-interpreter (#454, merged)** made the interpreter *execute* Branch/Parallel/Loop
  (recursive `run_steps` walk, flat-global nested scope, bounded fork-join). Still no syntax
  produces them — `PipelineRunRef` (`tau-pkg` `project.rs`) is only Agent/Tool/Deterministic/Check.

So the engine can run these blocks but **no author can write one.** The remaining work is the
*authoring→lowering* consumer, cut vertically per construct. Each slice below builds the
syntax→lowering→typecheck-reachability→wasm-parity→conformance→docs on top of the merged
interpreter; see [slicing-policy.md](../../explanation/slicing-policy.md) (which cites 4.1 and
4.2-interpreter as its worked producer-without-consumer examples).

**Slice DoD template (identical in shape for 4.2a/4.2b/4.2c):**
`tau.toml` syntax → lowering → typecheck (incl. user-reachability) → interpreter execution
(already merged, #454) → the construct's `IrFeature` set flipped (D8's feature-set honesty
test forces this) → wasm: parity OR explicit feature-reject at load → conformance fixture
runs in CI → one docs example. **Merge = "a user can author and run `<construct>` end-to-end
today."**

- **4.2a** **Branch end-to-end.** `tau.toml` branch syntax → `PipelineRunRef::Branch` lowering
  → user-reachable → docs example. *Delta:* a user can newly write a conditional branch in
  `tau.toml` and run it.
- **4.2b** **Parallel end-to-end.** *Delta:* a user can newly write a parallel fan-out/join in
  `tau.toml` and run it (bounded fork-join already executes).
- **4.2c** **Loop end-to-end.** Absorbs the nested-scope items that stop being deferrable once
  Loop is user-reachable: `Loop.until` referencing its own body's output (rejected today,
  ADR-0058:92), nested `PipelineStepId` uniqueness, nested `${steps.<id>.output}` template
  visibility, **and the nested-input template-validation gap** (audit H5: typecheck never
  parses nested step `.input` templates — `typecheck.rs:219-244` vs `:263-369`). *Delta:* a
  user can newly write a bounded loop with feedback in `tau.toml` and run it.
- **4.3** **Suspend end-to-end + checkpoint/resume round-trip.** Its natural pair; the existing
  4.3 scope (reuse the shipping `per_tool_call` checkpoint — HITL = checkpoint + wait for signal
  + seed-and-skip resume) merges in. Interpreter currently aborts Suspend with a named error
  (deferred from #454). *Delta:* a user can newly suspend a run for a human/external signal and
  resume it.
- **4.4** ✅ SHIPPED 2026-08-21 **Dynamic regions** (`StepRun::Dynamic` + ceiling + bounds;
  build-time envelope verify, tier 2) — scope unchanged, **and** now the tracking home for the
  **agent⊇spawn lattice check** (deferred from 1.5 "to EPIC 4" but absent from every story).
  Named sub-story landed with its prerequisite **per-kind agent definitions**
  (`[agent.kinds.*]`, origin ADR-0024). *Delta:* a user can newly declare a bounded dynamic
  region whose spawns are lattice-checked at build (`spawn_exceeds_agent`,
  `unknown_spawn_kind`, `region_exceeds_ceiling`, `spawn_exceeds_region`) — see
  [Dynamic regions](../../explanation/dynamic-regions.md). Runtime execution deferred to 4.5.
- **4.5** ✅ SHIPPED 2026-08-27 **Runtime gate for dynamic regions** (membership +
  attenuation + bounds counters). The 3.4↔4.5 wasm collision resolved as divergence, not
  merge: `tau build --target wasm` rejects any workflow containing a dynamic region at
  build time (`FeatureUnsupported`), so the guest interpreter never sees one and needs no
  gate of its own. *Delta:* a user can run a bounded dynamic region: the coordinator
  spawns store-backed kinds via `agent.<kind>.spawn`, gated by membership + bounds +
  meet-attenuation — see [Dynamic regions](../../explanation/dynamic-regions.md).
- **4.6** ~~Conformance~~ **DELETED as a story.** Conformance/parity is in every slice's DoD
  above, never a trailing phase (per slicing-policy.md rule 4).

**Epic DoD:** a user can author + run Branch/Parallel/Loop/Suspend and bounded dynamic regions
from `tau.toml`; each construct is conformance-checked and its envelope enforced. **Fully met
as of 4.5 (2026-08-27)** — 4.5 was EPIC 4's last outstanding story.

## EPIC 5 — Polyglot sourcing + DX  [needs 0,2]
**Goal:** generated typed SDKs + `tau embed` + golden path + typed React/Angular; no surprises.
- **5.1** `tau build --target wasm-guest | rust-lib`. *Accept:* both artifacts build.
- **5.2** `tau embed --host c|rust|js` (generated host glue from WIT via wit-bindgen).
- **5.3** ✅ Authoring-SDK codegen from the IR JSON schema (Smithy/JSON-Schema style); ship TS +
  Python. *Accept:* same agent in TOML/TS/Python → identical IR.
  **[2026-09-01: SUPERSEDED]** — the acceptance criterion ("same *agent* in TOML/TS/Python")
  is invalidated by the three-surface split (redesign §1: agents/models/`[allow]` are
  TOML-dirs-only forever; TS authors *choreography* only, Python drops to consumer-first).
  The shipped static-extraction TS factories and the TOML-emitting Python SDK are deleted
  in Phase 1 (epic E-1) when their replacement lands; the one-validation-path pattern and
  the schema-driven codegen machinery are harvested by the synth contract (ADR-0072) and
  the `schemas/project-manifest/`-generated L1 factories (E-1/E-2).
- **5.4** Typed React hook + Angular service (jco + ergonomic `tau embed` wrappers; Web
  Worker; `RunEvent` stream). *Accept:* typed npm package; demo renders streaming.
- **5.5** Wire + document the 3-gate guarantee (compile-time types → `tau check` → conformance).
- **5.6** Browser caps profile + published bundle-size number (wasm-metadce).
**Epic DoD [re-pointed 2026-09-01, ADR-0071]:** ~~one agent → 3 frontends → identical IR~~
→ **one project → one IR, three surfaces** (TOML vocabulary / TS choreography / Rust
muscle, all through the single validator); typed React/Angular *consumer* package; gates
documented.

## EPIC 6 — Durability tiers  [A-minimal shipped (#373); mostly independent]
**Goal:** intent-knob + compose-with-orchestrator + gated A-full.
- **6.1** Intent-knob `durable="survive-restarts"` → host-resolved granularity+store;
  `tau check --target X` PRINTS the resolved durability. *Accept:* resolution shown, no hidden behavior.
- **6.2** Compose-with-orchestrator how-to (Temporal/Inngest/CF/Dapr) + reentrancy test.
- **6.3** *(gated on a named exactly-once need)* A-full event-sourced replay; also powers
  dynamic-region tier-3b conformance.
**Epic DoD:** intent-knob works + transparent; orchestrator how-to shipped.

## EPIC 7 — Embedded as component  [needs 0,5]
**Goal:** tau-as-component on devices; wasm-on-MCU gated; firmware a niche.
- **7.1** no_std lib (Variant B) embedding API + example (product links tau, impls ports).
- **7.2** wasm-guest (Variant A) embedding in a product runtime + example.
- **7.3** *(gated: WAMR Component Model)* wasm-on-MCU spine. Gate **re-verified closed
  2026-08-23** — upstream WAMR (2.4.5) ships zero Component Model code and its own
  `dev/cm_wasip2` branch is `ahead_by=0`; evidence in [#415](https://github.com/tau-rs/tau/issues/415).
- **7.4** *(gated: named gateway-less buyer)* tau-as-firmware niche (embassy+WAMR layering).
**Epic DoD:** a product embeds tau as a no_std lib AND a wasm-guest; MCU path documented as gated.

---

## The 2026-09-01 redesign epics — three surfaces, ops lane, instruction set

Source of record: the consolidated design
[`../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md`](../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md)
(its §10 decisions ledger is LOCKED) via the handoff
[`2026-09-01-handoff-redesign-backlog.md`](2026-09-01-handoff-redesign-backlog.md).
ADR wave: **ADR-0071..0077**. Implementation trees:
[`authoring-surfaces`](../implementation-trees/authoring-surfaces.md) ·
[`instruction-set`](../implementation-trees/instruction-set.md) ·
[`ops-lane`](../implementation-trees/ops-lane.md) ·
[`exposures`](../implementation-trees/exposures.md).

**v1 = E-0 → E-4** (phases 0–4, each independently shippable). Arc acceptance:
*north-star-v2* rebuilt on the new stack while the original north-star stays green as the
legacy witness. The four UX requirements (design §12 — the permission sheet,
journal-recording honesty, plan signal discipline, source-mapped synth errors) are
**acceptance criteria on their epics, not polish**. Sequencing:

```
E-0  Align & clean            ◀── unblocks everything; no behavior change
E-1  Rust declarations        (needs E-0 merged ADRs)
E-2  Flow lane                (needs E-1: tau.gen.ts + project-manifest schema)
E-3  Prove                    (needs E-2: pipelines to record/plan/inspect)
E-4  Local ops                (needs E-3: plan is apply's diff engine)
```

### E-0 — Align & clean (Phase 0)  [unblocks everything · no behavior change]
**Goal:** the repo stops contradicting the design; ADR wave merged; dead weight deleted.
Plan: [`2026-09-01-epic-e0-align-and-clean.md`](2026-09-01-epic-e0-align-and-clean.md).
- **E-0.1** ADR wave 0071–0077 merged (split, synth, IR v3, journal, ops lane,
  instruction-set umbrella, exposures) + supersession banners (ADR-0022 tau-workflow,
  ADR-0041 partial) + numbering-collision note in `docs/decisions/README.md`.
- **E-0.2** Doc amendments: ROADMAP killed-item argued narrowing + δ.2 QuickJS retirement
  + β.8 "one way" retirement; CONSTITUTION G6 + QG12 + cheatsheet; `tau-philosophy.md`
  §"What you author" (three surfaces; "TS is sugar" dropped).
- **E-0.3** Delete `tau-workflow` (crate + CLI verbs + docs; superseded twice).
- **E-0.4** Delete dead weight: `tau-plugin-base`, `landlock-exec-repro`, `embed_c` stubs,
  stale examples/refs.
- **E-0.5** `xtask/tests/architecture_md.rs` + `ARCHITECTURE.md` updated for the deletions.
**Epic DoD:** repo no longer contradicts the design; CI green; no behavior change.

### E-1 — Rust declarations (Phase 1)  [design §1, §3.4, §4]
**Goal:** the muscle surface — one tool authored via `#[tau::tool]` flows to gen + check +
card. Plan: [`2026-09-01-epic-e1-rust-declarations.md`](2026-09-01-epic-e1-rust-declarations.md).
- **E-1.1** Proc-macro crate: `#[tau::tool]`, `#[tau::deterministic]`, `tau::export![]`
  (name/schema/description/capabilities derive from signature + attribute).
- **E-1.2** Unified registry feeding both dispatchers (native + wasm).
- **E-1.3** Real content hashes for registered fns — closes the name-hash hole
  (`cmd/build.rs:533,597` sentinel).
- **E-1.4** `tau.gen.ts` emitter: typed bindings for agents/models/tools/deterministic
  fns/agent kinds; registry-content-hash-stamped (stale gen = loud build error).
- **E-1.5** `schemas/project-manifest/` published + drift-tested (like `schemas/ir/`).
- **E-1.6** Legacy authoring lane deleted: `tau-ts-extract` static factories + TOML-emitting
  Python SDK (harvest the TOML-bridge/one-validation-path pattern).
**Epic DoD:** one tool authored via `#[tau::tool]` flows to gen + check + card; name-hash
hole closed.

### E-2 — Flow lane (Phase 2)  [design §1, §3.4]
**Goal:** the choreography surface — TS pipelines synth to ProjectConfig JSON through the
one validator. Plan: [`2026-09-01-epic-e2-flow-lane.md`](2026-09-01-epic-e2-flow-lane.md).
- **E-2.1** Synth subprocess runner (`[synth] entry` in `tau.toml`; Node/tsx default;
  `tau-sandbox-native`, no network, fs read-only; canonical JSON on stdout; merge at the
  unchecked level; CI double-synth byte-identity).
- **E-2.2** `pipelines/` dir scanning (one file = one pipeline, id = file path,
  ADR-0069/0070 style).
- **E-2.3** IR v3 multi-pipeline (`pipelines: BTreeMap<PipelineId, Pipeline>`; the single
  MAJOR bump, frozen v2 reader; schema + `REACHABLE-TYPES.md` + conformance fixtures move
  together).
- **E-2.4** Pipeline imports → `SubflowKind::Compose` unblocked (acyclic; namespaced under
  the call-site id; capabilities unchanged).
- **E-2.5** `[steps]` / `[tools] native=` removal (deprecate-warn one cycle).
- **E-2.6** Predicate algebra + structured template access (`${steps.x.output.field}`,
  JSON-pointer read only).
- **E-2.7** `tau init --ts` golden path; wasm feature-registry repair
  (`crates/tau-ports/src/target/registry.rs:136-139`; check in-flight PR #687 first).
**Epic DoD:** north-star-v2 authors + builds; TOML twin byte-equal where applicable.

### E-3 — Prove (Phase 3)  [design §2, §5, §6]
**Goal:** the proof verbs — journal, plan, inspect, emitters.
Plan: [`2026-09-01-epic-e3-prove.md`](2026-09-01-epic-e3-prove.md).
- **E-3.1** Journal substrate (`.tau/runs/<id>/journal.jsonl`, typed events keyed
  `(instance path, per-instance seq)`); `CheckpointGranularity::EventSourced` becomes real;
  snapshots demoted to replay-shortcut optimization.
- **E-3.2** `tau record` / `tau replay` (named `ReplayDivergence` on request-hash mismatch;
  `--live-tools`); HTTP-VCR cassettes retired.
- **E-3.3** `tau plan` + versioned `schemas/plan/` + exit codes 0/2/3/1 (capability changes
  always first; 3 = widens capabilities).
- **E-3.4** `tau inspect` — the permission-sheet capability card (+ `--attempt`).
- **E-3.5** Pipeline RunEvents (StepStarted/StepCompleted/CheckEvaluated/Suspended —
  additive run-event schema; freezes the NDJSON interface contract).
- **E-3.6** skill (SKILL.md) + AGENTS.md emitters (`tau export --skill`); authoring skill +
  `tau new` scaffolder.
**Epic DoD:** plan renders a capability-diff-first PR comment; a journal replays a Dynamic
run with concurrent spawns.

### E-4 — Local ops (Phase 4)  [design §5]
**Goal:** the machine is environment `local`; pin → plan → apply.
Plan: [`2026-09-01-epic-e4-local-ops.md`](2026-09-01-epic-e4-local-ops.md).
- **E-4.1** Env `local` + committed secret-free pins (`.tau/envs/local.state.toml`).
- **E-4.2** `tau apply` — atomic per repo (+ `--pipeline` valve); systemd-user timer
  adapters from `[trigger]` (ADR-0043 "compile the trigger, delegate the substrate").
- **E-4.3** `[[moved]]` rename records (drive plan rename-not-replace + checkpoint remap).
- **E-4.4** Lockfile v8 `[synth]` provenance (SDK version, gen hash, fragment SHAs;
  additive).
- **E-4.5** Remaining repairs lot (design §3.4): `AgentBudget.max_tokens` read,
  `judge_model` wired, `output_schema` enforced, goals retry authorable, subflow args
  forwarded, scalar-coercion fix, capability-order-insensitive hashes, real
  `max_concurrency`.
**Epic DoD:** north-star-v2 applied, scheduled by timer, resumed after rename via moved
record; wasm bundles run-or-refuse per environment.

### v2 backlog (do NOT plan yet — backlog entries only)
Per design §3.2, §6, §11: ForEach · Sleep · WaitForEvent/EmitEvent (⊇ Suspend,
resume_schema) · per-step retry/catch (fault plane) · variables+reducers ·
on_exit+cancellation · Explore (Option B, budget + synthesis reserve) · event triggers ·
environments/promote · serve v2 (socket, sessions, reverse dispatch) · MCP facade
(`tau serve --mcp`) · OCI distribution + bundle gallery · Python consumer SDK · `tau add`
packs (agent packs = δ.1 pulled to v2). **v2.5+:** memoization · typed client
(`tau export --client`) · declarative concurrency/rate · stdlib packs (best-of-N, saga,
sensor, fallback) · second authoring language. **v3:** fleet matrix · catalog · policy
packs. Individual v2 step kinds get their own ADRs when built (ADR-0076 sets the rules).

---

## Constitutional-first note
EPIC 8 + the constitutional stories (2.1, 1.1) are the only items that protect the vision
against work landing on parallel branches right now. Do them first regardless of the
critical path.
