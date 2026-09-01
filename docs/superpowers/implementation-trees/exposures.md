# Implementation Tree — exposure surfaces (no consumer needs a tau library)

> ## ⚠️ THIS IS A LIVING DOCUMENT
>
> This tree is **alive**. It is the running map of how the outside world —
> agents, shells, services, harnesses, other taus — discovers and drives a tau
> artifact, and it **must be updated after every implementation** that touches
> an exposure surface.
>
> **Update protocol (do all four in the implementing PR):**
> 1. Flip the node's status marker and stamp its PR number.
> 2. Add a dated bullet to **Discoveries** for anything non-obvious learned.
> 3. Move any now-built item out of **Next slices**; add newly-revealed work in.
> 4. If a node's public surface changed, update its row in **Nodes**.

**Scope:** the design-§6 integration matrix — emitters, CLI contract, serve,
MCP facade, embed preludes. The JS/TS *consumer* path (embed-js/react/angular)
keeps its own tree: [tau-sdk-consumers](tau-sdk-consumers.md). Authoring is
[authoring-surfaces](authoring-surfaces.md); plan/inspect internals are
[ops-lane](ops-lane.md) (their *contract* status is tracked here).

**Sibling docs:** design
[`2026-09-01-tau-authoring-ops-and-primitives-design.md`](../specs/2026-09-01-tau-authoring-ops-and-primitives-design.md)
§6/§7 · ADR [0077](../../decisions/0077-agent-exposure-surfaces.md) (v1 set +
v2 plan + G6/QG12 reframe) · plan
[E-3](../plans/2026-09-01-epic-e3-prove.md) (T5, T8, T9) · backlog
[`vision-roadmap.md`](../plans/vision-roadmap.md) (v2 section).

---

## Legend

`✅ shipped (PR#)` · `🟡 in progress` · `⬜ not started (planned epic)` ·
`🔵 v2+ backlog (own ADR when built)` · `⛔ rejected (ADR-0077)` ·
`⟂ guarded by a drift/coverage/unit test`

---

## The tree

```
exposure surfaces — one artifact, every consumer (ADR-0077)
│
├── process contract (shell / cron / CI)
│   ├── deterministic exit codes                                          ✅ exists · plan's 0/2/3/1 ⬜ E-3 T6
│   ├── frozen NDJSON stdout (RunEvent stream)                            ⬜ E-3 T5 (freeze rides the RunEvents repair)
│   └── agent-grade CLI: ≤1,500-token help, tested                        ⬜ E-3 T9  ⟂ help-budget test
│
├── agent surfaces (v1)
│   ├── tau export --skill  → generated SKILL.md (AAIF)                   ⬜ E-3 T8  ⟂ emitter drift
│   ├── AGENTS.md emitter                                                 ⬜ E-3 T8  ⟂ emitter drift
│   ├── official authoring skill + tau new scaffolds                      ⬜ E-3 T9
│   └── plan-exit-3 PR gate (agents author, CI reviews power)             ⬜ E-3 T6 DoD (how-to)
│
├── observability / policy
│   ├── OTLP span mapping documented as contract (journal-derived)        ⬜ E-3 T10
│   └── plan JSON twin (schemas/plan/)                                    ⬜ E-3 T6  ⟂ drift
│
├── durable orchestrators — reentrant process + --resume/--signal         ✅ (ADR-0053)
├── event infra — emitted trigger adapters                                ✅ + systemd-user units ⬜ E-4 T4
├── web / edge / embedded — wasm component + WIT world                    ✅ (contract #2) · feature repair 🟡 PR #687/E-2 T10
├── custom harnesses — Rust embed prelude (tau_runtime_core::embed)       ✅ (EPIC 7.1) · dogfood: rebuild tau dev on it 🔵
│
└── v2+ (backlog; each needs its own ADR per ADR-0076/0077)
    ├── serve v2: Unix socket, warm daemon, session.* + reverse dispatch  🔵 v2
    │       host tools: declared in [allow], schema-validated, card-labeled
    ├── MCP facade: tau serve --mcp (pipeline = typed tool, card in _meta)🔵 v2
    ├── cross-org MCP both ways (double-bounded; tau mcp pin)             🔵 v2 (pin half ✅ exists)
    ├── typed clients: tau export --client ts|py (generated, never hand)  🔵 v2.5
    ├── Python consumer SDK                                               🔵 v2
    ├── A2A card projection                                               🔵 watch (invocation = ⛔ rejected)
    └── Wassette-style components-as-MCP-tools                            🔵 watch item
```

---

## Nodes

| Node | Path | Public surface | Status |
|---|---|---|---|
| NDJSON contract | `crates/tau-runtime-core/src/stream.rs`, `schemas/run-event/` | RunEvent stream incl. pipeline events | ⬜ E-3 T5 |
| skill/AGENTS emitters | `crates/tau-sdk-codegen/src/{skill_md,agents_md}.rs` (new), `cmd/export.rs` | `tau export --skill/--agents-md` | ⬜ E-3 T8 |
| CLI contract | `crates/tau-cli` | help budget + exit-code table (tested) | ⬜ E-3 T9 |
| authoring skill + tau new | `crates/tau-cli/src/cmd/new.rs` (new) + skill pkg | scaffolds; the agents-author-safely path | ⬜ E-3 T9 |
| plan JSON | `schemas/plan/` | policy-tool interchange | ⬜ E-3 T6 |
| embed prelude | `tau_runtime_core::embed` | Rust harness surface | ✅ EPIC 7.1 |
| serve v1 | `crates/tau-cli` serve mode (ADR-0033) | JSON-RPC over stdio | ✅ (v2 = 🔵) |

**Invariants (don't break without a deliberate slice):**
- No consumer ever needs a tau library — every surface is process/protocol/generated-file shaped (design §6).
- Emitters are generated from the IR + capability card, never hand-written; drift tests pin committed == fresh (the `embed_js_drift` pattern).
- The capability card travels with the surface (skill text, MCP `_meta`, inspect) — never re-stated by hand.
- Public surface = the schema-defined contract set (G6/QG12 as amended by ADR-0077); clients are generated or not shipped.
- Frozen means frozen: NDJSON/RunEvent changes are additive after E-3 T5.

---

## Discoveries (append-only, dated)

- **2026-09-01** (backlog session) The `embed_js.rs` emitter + `embed_js_drift` test is the house pattern for every new emitter (skill, AGENTS.md, gen_ts) — committed output == fresh emit, never hand-edited.
- **2026-09-01** (backlog session) `tau mcp pin` already freezes external MCP contracts — the v2 "MCP both ways, double-bounded" story reuses it for the inbound half; only the facade (outbound) is new.
- **2026-09-01** (backlog session) `embed_c` stubs are deleted in E-0 (T5): the C-consumer path is the wasm component + WIT, not generated C glue — keep the `tau embed --host c` error message pointing there.

---

## Next slices (ranked)

1. **E-3 T5** — pipeline RunEvents → freeze the NDJSON contract (everything agent-facing cites it).
2. **E-3 T8** — skill + AGENTS.md emitters (the v1 agent-surface core).
3. **E-3 T9** — CLI contract tests + authoring skill + `tau new`.
4. **E-3 T6/T10** — plan JSON + PR-comment how-to + OTLP contract doc.
5. **v2 ADR queue:** serve v2 → MCP facade → typed clients (ADR-0077 sketches the shape; each gets its own ADR when built).
