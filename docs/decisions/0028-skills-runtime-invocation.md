# ADR-0028 — Skills runtime invocation (Skills-4)

> **Updated 2026-05-31 (β.1.4):** `tau_runtime::*` paths below now read
> as `tau_runtime_core::*` for kernel-side symbols and
> `tau_runtime_tokio::*` for host-shell symbols. `skill.<name>.spawn`
> dispatch lives in `tau_runtime_core::orchestration`.

**Status:** Accepted 2026-05-14.
**Branch / PR:** `feat/skills-4-runtime-invocation` (PR pending).
**Spec:** `docs/superpowers/specs/2026-05-14-skills-4-runtime-invocation-design.md` (merged at PR #69).
**Plan:** `docs/superpowers/plans/2026-05-14-skills-4-runtime-invocation.md`.
**Depends on:** ADR-0025 (Skills-1), ADR-0026 (Skills-2), ADR-0027 (Skills-3), ADR-0024 (multi-agent orchestration v1.1 + v1.2).

## Context

Fourth of 6 sub-projects from ROADMAP §16 (Skills as first-class packages, Constitution G10). The runtime piece that makes installed skills usable by agents.

When an agent emits a `skill.<name>.spawn` virtual tool call, the runtime resolves `<name>` to an installed skill, builds a child agent run from the skill's declared `system_prompt` + capabilities (with `${SKILL_DIR}` substituted + optional caller `scope_paths` narrowing), and recursively invokes `Runtime::run_with_history` via the v1.1 agent-spawn machinery shipped in PR #60.

## Decision

Three locked decisions (from brainstorming):

### D1: Separate URI namespace + capability variant

`skill.<name>.spawn` parallel to existing `agent.<kind>.spawn`. New `Capability::Skill(SkillCapability::Spawn { allowed_skills })`.

TOML form:

```toml
[[capabilities]]
kind = "skill.spawn"
allowed_skills = ["critic", "fact-checker"]
```

No namespace collision possible between custom agent kinds and skill names. Authorization (parent has `<name>` in `allowed_skills`) is enforced in `validate_skill_spawn`, independent of the agent-spawn authorization path.

### D2: Caller `scope_paths: Option<Vec<String>>` narrows fs.* paths only

Per-kind intersection; hard-fail on uncovered `scope_path` (typo detection); non-fs capabilities pass through unchanged. Caller cannot add new capabilities or change capability kinds — skill author owns the capability contract; caller can tighten scope only.

Semantics (per `apply_scope_paths`):

- For each `fs.*` capability the skill declares, the child's effective paths = intersect(declared paths, `scope_paths`). Non-fs caps unchanged.
- If a `scope_path` is not covered by ANY declared fs.* path → `SkillScopePathNotCovered` error (typo detection).
- If intersection is empty for a given fs.* kind → drop that capability entirely.

### D3: Full multi-turn `MockLlmBackend` test fixture in this PR

Lifts the existing `ScriptedLlm` pattern from `tests/run_with_tool_calls.rs` into reusable `crates/tau-runtime/tests/common/mock_llm.rs`. Builder API: `MockLlmBackend::new(name).add_text("...").add_tool_call(name, args).add_end()`.

Bonus: 5 `#[ignore]`'d orchestration pattern test skeletons from PR #59 are un-ignored using this fixture (Tasks 9 patterns: linear, worker-pool, supervisor-critic, hierarchical, plan-revise).

## Alternatives considered

- **Reuse `agent.<kind>.spawn` URI for skills** (Option A in brainstorm): namespace collision risk with custom kinds; capability conflation between agent-defined and skill-installed children.
- **Caller-supplied `grant: Vec<Capability>` override** (Option B in brainstorm): too much rope; precedence rules become unnecessary public surface; skill author no longer owns capability contract.
- **No caller knob at all** (Option X in brainstorm): loses per-spawn narrowing use cases (one skill, multiple project scopes).
- **Defer MockLlmBackend to follow-up PR**: weaker Skills-4 coverage; fixture has to be built eventually for the multi-agent pattern tests anyway.

## Consequences

- `tau-domain` public surface grows by `SkillCapability` + `Capability::Skill` variant + `CapabilityShape::SkillSpawn`.
- `tau-pkg` public surface grows by `find_installed_skill` + `InstalledSkill` + `FindSkillError`.
- `tau-runtime` adds `orchestration::skill_resolve` module + 5 `OrchestrationError` variants + `validate_skill_spawn` + `is_skill_spawn` branch in `run_streaming_inner`.
- `crates/tau-runtime/tests/common/mock_llm.rs` is the canonical multi-turn LLM test fixture — future test suites can reuse. Copied verbatim into `crates/tau-cli/tests/common/mock_llm.rs` for cross-crate test reuse (~180 LOC; not a real maintenance burden, but ROADMAP-tracked for future shared-test-support extraction).
- 5 previously-`#[ignore]`'d pattern tests from PR #59 are now running, providing real e2e coverage of multi-agent orchestration patterns.
- No new external deps.
- No CI changes.

## Discovered during implementation

Skills-4 T8 e2e tests surfaced three gaps in `capability_satisfies`:

1. **`Capability::Skill` arm missing** — fixed in this PR. Added `skill_satisfies` helper with `string_subset` semantics matching `agent_satisfies`. Without this fix, every `skill.<name>.spawn` would silently PolicyDeny.

2. **`Capability::TaskList` / `Capability::Plan` arms missing** — fixed independently on main (merged before this PR rebased; visible in `capability.rs` as `task_list_satisfies` + `plan_satisfies` helpers with mode-rank subsumption). Merge resolved by keeping both branches' arms.

3. **`check_capability_subset()` uses literal JSON string comparison** (`tau-runtime/src/orchestration/virtual_tools.rs`): subset law check compares JSON serialisations rather than semantic capability comparison. Means a narrowed `agent.spawn(["coder","tester"])` isn't recognized as subset of `agent.spawn(["team-lead","coder","tester"])`. **Still deferred** — T9 pattern D works around by granting the child the full grant. Deserves its own ROADMAP entry.

## Out of scope (deferred to Skills-5+)

- **Agent Skills spec export / import** → Skills-5
- **Reference skill packages + user docs** → Skills-6
- **Sub-skill `requires_skills` runtime enforcement** — advisory only
- **Body parse caching across spawns** — YAGNI
- **Caller-side capability merge (`grant_extend`)** — add when use case surfaces

## References

- Spec: `docs/superpowers/specs/2026-05-14-skills-4-runtime-invocation-design.md`
- Plan: `docs/superpowers/plans/2026-05-14-skills-4-runtime-invocation.md`
- Skills-1 ADR: `docs/decisions/0025-skills-foundation.md`
- Skills-2 ADR: `docs/decisions/0026-skills-install-pipeline.md`
- Skills-3 ADR: `docs/decisions/0027-skills-discovery.md`
- Multi-agent ADR: `docs/decisions/0024-multi-agent-orchestration.md`
- Multi-agent v1.1 PR: #60 (recursive `agent.<kind>.spawn`)
- Multi-agent v1.2 PR: #61 (per-spawn `system_prompt`)
- ROADMAP §16
