# tau vision roadmap — epics & stories

Derived from the 2026-06-20 roadmap-challenge session. Source of record:
`.context/SESSION-SYNTHESIS.md` (working notes) — this doc is the committed,
implementable backlog.

**Vision (one line):** tau is a compiler+engine — declare what agents are *allowed* in a
root `tau.toml` constitution, build workflows beautifully in any language (generated typed
SDKs or the tau-native DSL), and tau proves behavior ⊆ constitution at build time and emits
one hardware-agnostic, capability-bounded, conformance-proven component you embed anywhere,
with no runtime surprises.

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

## EPIC 0 — Core no_std readiness (de-std the run loop)  [BLOCKS 3,4,5,7]
**Goal:** `run`/`stream`/`interpreter` compile *and run* no_std (today gated behind
`tool-validation` → pulls std via `jsonschema` + `tau-domain/std`).
- **0.1** Inventory every std pull in the run path. *Accept:* documented list.
- **0.2** Move tool-arg JSON-schema validation to build-time (`tau check`) or a no_std
  validator, so the run loop drops `jsonschema`. *Accept:* run loop builds without it.
- **0.3** Remove `tau-domain/std` from the run path; feature-gate remaining std uses.
- **0.4** `serde_json` alloc-only in the core.
- **0.5** CI lane: `check (tau-runtime-core no-default-features WITH run loop / linux)`.
**Epic DoD:** the agent loop compiles + runs no_std; new CI lane green.

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
- **3.5** `verify --bundle`: generated WIT reproducible from declared caps.
**Epic DoD:** an ungranted cap is un-importable at the ABI; wasm caps == `[allow]`-bounded set.

## EPIC 4 — IR control flow: structured blocks + dynamic regions  [needs 2]
**Goal:** Branch/Parallel/Loop/Suspend + capability-bounded dynamic regions; IR ≥ v2.3.0.
- **4.1** ADR + IR bump: `StepRun` gains Branch/Parallel/Loop/Suspend (reuse
  `Locus`/`GoalPredicate`/`OnFail::Retry` rewind). *Accept:* byte-stable when unused.
- **4.2** Interpreter: recursive structured walk; bounded fork-join for Parallel.
- **4.3** `Suspend` reuses the shipping `per_tool_call` checkpoint (HITL = checkpoint + wait
  for signal + seed-and-skip resume). *Accept:* suspend/resume round-trip.
- **4.4** `StepRun::Dynamic` + ceiling + bounds; build-time envelope verify (tier 2).
- **4.5** Runtime gate: membership + attenuation + bounds counters for dynamic regions.
- **4.6** Conformance: normalize parallel-branch event order by index; tier-3a CI conformance.
**Epic DoD:** blocks + dynamic regions run + conformance-checked; envelope enforced.

## EPIC 5 — Polyglot sourcing + DX  [needs 0,2]
**Goal:** generated typed SDKs + `tau embed` + golden path + typed React/Angular; no surprises.
- **5.1** `tau build --target wasm-guest | rust-lib`. *Accept:* both artifacts build.
- **5.2** `tau embed --host c|rust|js` (generated host glue from WIT via wit-bindgen).
- **5.3** Authoring-SDK codegen from the IR JSON schema (Smithy/JSON-Schema style); ship TS +
  Python. *Accept:* same agent in TOML/TS/Python → identical IR.
- **5.4** Typed React hook + Angular service (jco + ergonomic `tau embed` wrappers; Web
  Worker; `RunEvent` stream). *Accept:* typed npm package; demo renders streaming.
- **5.5** Wire + document the 3-gate guarantee (compile-time types → `tau check` → conformance).
- **5.6** Browser caps profile + published bundle-size number (wasm-metadce).
**Epic DoD:** one agent → 3 frontends → identical IR; typed React/Angular package; gates documented.

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
- **7.3** *(gated: WAMR Component Model)* wasm-on-MCU spine.
- **7.4** *(gated: named gateway-less buyer)* tau-as-firmware niche (embassy+WAMR layering).
**Epic DoD:** a product embeds tau as a no_std lib AND a wasm-guest; MCU path documented as gated.

---

## Constitutional-first note
EPIC 8 + the constitutional stories (2.1, 1.1) are the only items that protect the vision
against work landing on parallel branches right now. Do them first regardless of the
critical path.
