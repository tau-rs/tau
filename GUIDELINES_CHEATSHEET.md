# Tau Guidelines — Cheatsheet

All 59 guidelines, one line each. For the full text and rationale see `CONSTITUTION.md`.

## Identity (17)

- **G1** Core does four things: install packages, run agents, pass messages, observe.
- **G2** Everything domain-specific is a package.
- **G3** Terminal-native. No GUI, no TUI.
- **G4** Model-agnostic. Core never talks to a specific LLM.
- **G5** Messages are the universal interaction primitive.
- **G6** Extensions use public API only: the schema-defined contracts (authoring / artifact / operational interchange); SDKs are generated from them (ADR-0077).
- **G7** Package manager is the only way to add extensions.
- **G8** Global and project-local scopes; project overrides global.
- **G9** Solo or orchestrated agents use the same core machinery.
- **G10** Skills and MCP are first-class core concepts, no implementations bundled.
- **G11** Core ships empty.
- **G12** Security consent-based with real sandboxing enforcement.
- **G13** Credentials are references, never values.
- **G14** Packages declare capabilities; runtime enforces.
- **G15** Linux/macOS primary, Windows graceful degradation.
- **G16** Kernel overhead negligible (<100ms startup, <50MB memory).
- **G17** Developer inner loop under 3 minutes.

## Non-goals (12)

- **NG1** Not an LLM or an agent.
- **NG2** Not coding-specific.
- **NG3** Not a hosted service.
- **NG4** Not a package marketplace.
- **NG5** Not a general-purpose workflow engine.
- **NG6** No persistent agent memory in core.
- **NG7** No agent quality evaluation.
- **NG8** Not an AI safety harness.
- **NG9** No identity, authentication, or credential management.
- **NG10** No telemetry or training data collection.
- **NG11** Developer tool, not end-user tool.
- **NG12** Runtime, not framework.

## Quality (25)

- **QG1** Rustfmt and clippy block merge at `deny(warnings)`.
- **QG2** `thiserror` in libraries, `anyhow` in binaries.
- **QG3** No `unwrap`/`expect`/`panic` in library code (tests exempt).
- **QG4** `#![forbid(unsafe_code)]` default; exceptions require ADR.
- **QG5** Four test layers mandatory: unit, integration, doc, CLI; proptest for parsers; fuzz for IPC.
- **QG6** CI runs Linux + macOS + Windows on stable; MSRV verified on Linux only.
- **QG7** MSRV is stable-2, bumps are minor-version changes.
- **QG8** Documentation follows Diátaxis.
- **QG9** `#![deny(missing_docs)]` on library public items; broken intra-doc links fail CI.
- **QG10** Every repo has README, LICENSE, CONTRIBUTING, SECURITY, CODE_OF_CONDUCT, GOVERNANCE.
- **QG11** Strict SemVer; no breaking changes in patch releases.
- **QG12** Public API surface = the contract set (schemas + `tau-runtime` crate + serve-mode protocol), all SemVer'd + drift-tested.
- **QG13** Deprecations documented; one cycle pre-1.0, two post-1.0.
- **QG14** Performance budgets enforced in CI from Phase 2.
- **QG15** SECURITY.md with reporting channel.
- **QG16** `cargo audit` and `cargo-deny` block releases.
- **QG17** PRs pass CI, include tests, update docs, use Conventional Commits.
- **QG18** ADRs required for guideline/API/protocol/manifest/trait changes.
- **QG19** "Done" = implementation + tests + docs + changelog + migration notes + ADR if applicable.
- **QG20** Release cadence on-demand pre-1.0, time-based post-1.0.
- **QG21** Release checklist: CI green, CHANGELOG, SemVer bump, tag, artifacts, notes.
- **QG22** Code review required, even solo: overnight delay or external reviewer or self-checklist.
- **QG23** Every bug becomes a regression test.
- **QG24** No silent tech debt; refactor in PR or file tagged issue.
- **QG25** Every new dependency justified in PR description.

## Process (5)

- **PG1** ROADMAP.md in repo root; issues track work; no milestones/projects in Phase 0-3.
- **PG2** Phase 0-1 feature PRs need issue-discussion first; Phase 2+ alignment bar applies to all PRs including LLM-generated.
- **PG3** Non-ADR decisions recorded in commit messages and PR discussion.
- **PG4** Phase close produces `docs/retrospectives/phase-NN.md`.
- **PG5** Release notes first; blog in Phase 2+; chat channels deferred.

---

## Decision shortcuts

When building something, check in order:

1. **Is this covered by a guideline?** Apply it.
2. **Does this violate a non-goal?** Don't build it.
3. **Is this a class-A change** (guideline, public API, protocol, manifest, trait)? **File an ADR.**
4. **Is this ambiguous?** Surface to the maintainer.

## Anti-patterns

- Adding default content to core
- Using `.unwrap()` in library code
- Skipping tests because "the code is obviously right"
- Silent dependency additions
- Merging red CI
- Shipping undocumented public APIs
- Same-day self-merges
- Breaking changes in patch releases
- Bundling tau-specific features into plugin traits
