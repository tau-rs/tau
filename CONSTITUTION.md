# Tau Constitution

**Status:** Draft v0.1
**Scope:** Governs all decisions about what tau is, what tau does, and how tau is built.
**Audience:** Humans reading to understand tau's identity. LLMs reading as context for every agentic decision about the codebase.

---

## How to read this document

This document is tau's constitution. It is deliberately dense. Every statement is load-bearing; nothing is filler.

Four sections, each answering a different question:

1. **Identity** — what tau is
2. **Non-goals** — what tau is not
3. **Quality** — how tau holds quality
4. **Process** — how work happens

Guidelines are numbered and referenced throughout the rest of tau's documentation. A contributor proposing something that violates a guideline must either explain why the guideline should change (via ADR) or withdraw the proposal.

This document changes only via ADRs. No silent revisions. No "small tweaks." See §4 for how.

---

## 1. Identity — what tau is

### Thesis

> *Tau is a minimal, terminal-native Rust tool for installing and running agents — solo or orchestrated, globally or per-project — with skills, tools, MCP servers, LLM backends, and pipelines provided as installable packages.*

Compressed: *Tau installs and runs agents in the terminal. Everything else — models, tools, skills, pipelines — is a package.*

### Architecture at a glance

```
┌──────────────────────────────────────────────────────────────┐
│ USER-AUTHORED ARTIFACTS                                      │
│ Their agents, their project config, their data               │
└──────────────────────────────────────────────────────────────┘
                              ↑
┌──────────────────────────────────────────────────────────────┐
│ EXTENSION LAYER (all packages, all installable)              │
│ ┌─────────────┐ ┌──────────┐ ┌──────────┐ ┌───────────────┐  │
│ │ LLM plugins │ │  Tools   │ │ Pipelines│ │ SDK (addon)   │  │
│ │ @tau/claude │ │ fs,shell │ │ stature  │ │ new-agent,etc │  │
│ │ @tau/openai │ │ git,http │ │ others   │ │               │  │
│ └─────────────┘ └──────────┘ └──────────┘ └───────────────┘  │
└──────────────────────────────────────────────────────────────┘
                              ↑ plugs in via traits
┌──────────────────────────────────────────────────────────────┐
│ CORE (tau-runtime, published as Rust crate + CLI binary)     │
│                                                              │
│  ┌─────────────┐ ┌────────────┐ ┌────────────┐ ┌──────────┐  │
│  │  Package    │ │  Agent     │ │  Message   │ │ Observe  │  │
│  │  manager    │ │  lifecycle │ │  passing   │ │ (logs)   │  │
│  └─────────────┘ └────────────┘ └────────────┘ └──────────┘  │
│                                                              │
│  Public API (Rust library) ─── CLI (thin wrapper)            │
│                                                              │
│  Primitives: packages, agents, messages, plugins (traits)    │
│  Cross-cutting: worktrees, config, sandboxing                │
└──────────────────────────────────────────────────────────────┘
```

### Identity guidelines

**G1. Core does four things.** Installs packages, creates and runs agents, passes messages between entities, observes what happens. Anything else belongs in plugins, pipelines, SDKs, or outside tau entirely. *Rationale:* the minimal core is what makes tau durable; scope creep is the failure mode this guideline prevents.

**G2. Everything domain-specific is a package.** LLM backends, tools, pipelines, skills, MCP servers, SDKs, HITL surfaces, storage backends — all peers, all installable via the package manager, none bundled into core. *Rationale:* domain-specific code in core means tau takes sides on what agents are for; tau is for all agents.

**G3. Tau is terminal-native.** No GUI. No TUI beyond what terminal prompts require. Structured output flows to stdout/stderr; rendering happens at the edge (by whatever consumes tau's output). *Rationale:* terminal is the universal interface; GUIs are platform- and ecosystem-coupled.

**G4. Tau is model-agnostic.** Core never talks to a specific LLM. Agents talk to whatever backend package the user configured. Adding support for a new provider is a new plugin, not a core change. *Rationale:* agent workflows outlive specific model providers; tau must too.

**G5. Messages are the universal interaction primitive.** Every communication — human to agent, agent to agent, agent to tool, tool to agent — happens through messages. The message schema is stable. *Rationale:* one primitive, applied consistently, produces a system that can be reasoned about.

**G6. Extensions interact with core only through defined interfaces.** The `tau-runtime` Rust library API and the serve-mode IPC protocol are tau's two public surfaces. Both are versioned; both are stable within minor versions. No hidden side channels. *Rationale:* public surfaces are a commitment; every unofficial way-in becomes a bug waiting to happen.

**G7. The package manager is the only way to add extensions.** Everything enters tau via `tau install`. Including plugins. *Rationale:* one path in means auditability; alternate mechanisms (env vars, magic directories) fracture the security model.

**G8. Tau supports global and project-local scopes.** Packages, configs, and agents can be installed at either scope. Project scope overrides global when both apply. Project scope is detected by walking up from cwd looking for `.tau/`. *Rationale:* personal use (Claude-like) wants global; project-embedded use (ECC-like) wants local; both are first-class.

**G9. Tau runs agents solo or orchestrated, without special cases.** A solo agent is an agent running without a pipeline. An orchestrated multi-agent flow is a pipeline package coordinating other agents. Same core machinery both ways. *Rationale:* two code paths for "one agent" vs "many agents" fragments the system; one path for both is simpler and more composable.

**G10. Skills and MCP are first-class concepts in core.** Tau understands the Agent Skills spec and the Model Context Protocol natively. Core provides the abstractions; specific skills and MCP server implementations are packages. *Rationale:* both protocols are widely adopted standards in 2026; tau speaks them rather than redefines them.

**G11. Core ships empty.** No bundled agents, skills, tools, or pipelines. First-run is `tau install <thing>`, then use. *Rationale:* purity of the extension model; no privileged "official" content.

**G12. Security is consent-based with sandboxing enforcement.** Package capabilities are declared at install and enforced at runtime through plugin isolation. The specific sandboxing mechanism (WASM, OS-native, other) is a Phase 1 implementation decision, not a guideline-level choice. Credentials are never in agent context or logs by default. *Rationale:* "plugins can do anything" is not acceptable; real enforcement is required.

**G13. Credentials are references, not values.** Agents see credential handles (like "the Claude credential"), never raw API keys. The core credential system is enforced; plugins cannot bypass it. *Rationale:* credentials leaked into agent contexts or logs are a class of catastrophic failure; prevent the class at the system level.

**G14. Packages declare their capabilities at install time.** Users see what a package claims to do before installing. Runtime enforcement prevents packages from exceeding declared capabilities. *Rationale:* informed consent at install time plus runtime enforcement catches malicious and mistaken overreach.

**G15. Linux and macOS are primary platforms; Windows degrades gracefully.** CI matrix covers all three; Windows failures do not block Linux/macOS releases. Platform-specific primitives (POSIX flock, etc.) have Windows-appropriate alternatives. *Rationale:* Windows support matters for corporate users but disproportionate engineering on it starves other work.

**G16. Kernel overhead stays negligible.** Startup under 100ms, memory under 50MB for the kernel itself, no measurable per-message latency beyond serialization. Regressions block PRs. *Rationale:* tau is infrastructure for agents; infrastructure overhead compounds and is paid by every user on every invocation.

**G17. Developer inner loop stays under 3 minutes.** `cargo test --workspace` completes under this budget at all phases. CI enforces the budget. *Rationale:* slow inner loops degrade contributor productivity and code quality simultaneously; a 3-minute ceiling forces architectural discipline.

### Architectural decisions implied by the identity

These are not guidelines but consequences of the above. They are listed here so they do not have to be re-derived:

- **Dual-mode binary** — tau ships one binary with CLI mode and serve mode. Serve mode is how other programs embed tau; CLI mode is how humans use it interactively.
- **Parent-process-spawned subprocess** is the primary production embedding model. The parent app (web server, desktop app, CI job) spawns `tau serve` as a child, talks to it over stdio or socket, kills it on shutdown. No system daemon, no custom compile step.
- **npm/pip/etc. SDKs** wrap the serve-mode protocol. Phase 3+ distribution. The SDK bundles tau's binary for each platform (esbuild pattern).
- **Package distribution via git URLs** initially (Phase 0-2); centralized registry later only if demand warrants.
- **Crate scope:** `tau-runtime` is the canonical Rust crate; `tau-cli` is the binary; other core crates (`tau-domain`, `tau-ports`, `tau-app`, `tau-pkg`, `tau-observe`) follow hexagonal architecture.
- **SDK is an addon package,** not part of core. `tau-sdk` (Rust) and `@tau/sdk` (npm) are separate distributions built on the public API.
- **Stature** (the opinionated coding pipeline) is a separate downstream project, authored by the same maintainer but in its own repository; not part of tau's Phase 0.

---

## 2. Non-goals — what tau is not

Writing non-goals down is rare in open-source projects. The ones that do it find it reduces scope-creep pressure and clarifies community expectations. These are tau's explicit non-goals.

**NG1. Tau is not an LLM or an agent.** It runs agents; agents run on LLMs. Users seeking "an AI to talk to" should interact with an installed agent, not tau itself. There is no `tau ask` that talks to tau's own reasoning.

**NG2. Tau is not a coding-specific tool.** Agents on tau can be for any domain: coding, research, writing, customer support, data analysis, anything. Coding-specific workflows live in pipelines (like stature) that are built on tau but separate from it.

**NG3. Tau is not a hosted service.** The tau project ships a binary and documentation. It does not run infrastructure, serve end users, or offer "tau cloud." Downstream users can self-host; the project itself does not.

**NG4. Tau is not a package marketplace.** Packages are installable via `tau install`. Tau does not curate, rank, feature, or moderate packages. Discovery happens through external channels (GitHub, blog posts, word of mouth).

**NG5. Tau is not a general-purpose workflow engine.** Pipelines in tau coordinate agents. They are not alternatives to Airflow, Temporal, n8n, or similar. A "pipeline" in tau's vocabulary means "coordinates agents," not "coordinates arbitrary tasks."

**NG6. Tau does not provide persistent agent memory.** Memory (cross-session state, long-term recall, learned behavior) is an agent-level or plugin-level concern. Tau provides per-invocation message passing; it does not retain agent state between invocations in core. Plugins may add persistence.

**NG7. Tau does not evaluate agent quality.** Tau runs agents. It does not rate, score, benchmark, or certify them. Quality is between agent authors and their users.

**NG8. Tau is not an AI safety harness.** Security mechanisms (sandboxing, capability declarations, credential abstraction) protect the system against misbehaving or malicious packages. They are not guarantees about agent output quality, alignment, truthfulness, or ethics. Those concerns are the agent author's or the LLM backend's responsibility.

**NG9. Tau does not manage identity, authentication, or credentials.** Tau references credentials; it does not store or manage them. Multi-tenant deployments handle authentication and authorization at a layer above tau.

**NG10. Tau does not collect telemetry or training data.** User interactions stay on user machines. Tau ships no "phone home" analytics, even opt-in. No conversation data, no usage metrics, no error reports are transmitted to any server controlled by the tau project.

**NG11. Tau is a developer tool.** It is not designed for non-developer end users. Products built on tau may serve non-developers; tau itself requires developer skills to install, configure, and use.

**NG12. Tau is a runtime, not a framework.** Tau provides protocols and primitives; it does not prescribe how agents or pipelines should be structured. Opinionated structure belongs to pipeline packages (like stature). Tau's documentation explains protocols; pipeline documentation explains patterns.

---

## 3. Quality — how tau holds quality

### Quality posture

Quality is a first-class constraint, not a trade-off to be balanced against speed. Cheap shortcuts are rejected. Practices that do not earn their keep are not adopted for their own sake. Every quality practice in this section either catches real bugs, improves clarity, or reduces future work. If a practice does not earn its keep, it gets removed, not added to.

The rule: **no cheap shortcuts.** Not "every possible quality practice." The distinction matters.

### Code quality

**QG1. Rustfmt and clippy enforced in CI.** Clippy at `deny(warnings)` level; failures block merge. No custom lints in Phase 0-2.

**QG2. Error handling is typed in libraries, flexible in binaries.** `thiserror` for `tau-runtime`, `tau-ports`, `tau-domain`, `tau-sdk`. `anyhow` for `tau-cli`, `tau-app`. Error types in public APIs are part of the stable surface.

**QG3. Panics are bugs in libraries.** Library crates deny `unwrap`, `expect`, and `panic` except in tests and documented invariants. Binary crates install `human-panic` for user-friendly crash output.

**QG4. Unsafe code is forbidden by default.** `#![forbid(unsafe_code)]` on all workspace crates. Exceptions require an ADR documenting why, confined to specific modules with safety comments.

### Testing

**QG5. Testing has four mandatory layers.** Unit tests inline with code. Integration tests in `tests/` per crate. Doc tests on all public API items. CLI behavioral tests via `assert_cmd`. Property-based tests (proptest) for parsers of external input: manifest files, IPC messages, user configuration. Fuzz targets for the IPC protocol. Coverage thresholds are not enforced numerically; every public behavior has a test.

**QG6. CI runs the full platform matrix on stable.** Linux, macOS, Windows on stable Rust. MSRV is verified on Linux only — MSRV is a rustc-version property, not an OS property, and OS-gated code paths are already exercised by stable-toolchain runs on their native OS. Windows failures tracked but do not block Linux/macOS releases (per G15). Full feature-powerset testing added when and if feature combinations become non-trivial.

**QG7. MSRV is declared and conservative.** `Cargo.toml` declares MSRV at stable-2 (two stable releases behind latest). MSRV bumps are minor-version changes pre-1.0 and minor changes post-1.0, never silent patch bumps.

### Documentation

**QG8. Documentation follows Diátaxis.** Tutorials, how-to guides, reference, explanation in `docs/`. ADRs in `docs/decisions/`. Each topic has one canonical location. Generated documentation (CLI reference from clap, config schema from schemars) is produced by CI and kept out of the authored tree.

**QG9. Library crates enforce `#![deny(missing_docs)]` on public items.** Broken intra-doc links fail CI. Each public item has at least one example in rustdoc.

**QG10. Every repository has standard governance files.** Tau core mandates: README, LICENSE, CONTRIBUTING, SECURITY, CODE_OF_CONDUCT, GOVERNANCE. Plugin and pipeline repositories are encouraged but not required to have these.

### Stability and compatibility

**QG11. Strict SemVer.** Pre-1.0: `0.X.Y` where X bumps for breaking changes (allowed), Y for additions or fixes (non-breaking). Post-1.0: `X.Y.Z` full SemVer. Breaking changes never ship in a patch release.

**QG12. Two public API surfaces.** The `tau-runtime` crate's exported items and the serve-mode IPC protocol schema. Both follow the same SemVer policy. CLI flags are intermediate stability: stable within minor versions but not part of the formal API surface.

**QG13. Deprecations are documented and cycled.** `#[deprecated]` with migration instructions. Pre-1.0: deprecated items removed in next breaking release. Post-1.0: deprecated items remain for at least two minor versions before removal in a major release.

### Performance and security

**QG14. Performance budgets enforced.** Starting Phase 2, CI fails PRs exceeding G16 budgets (100ms startup, 50MB kernel memory, bounded per-message overhead). Baselines declared in `docs/performance/baselines.md`. Budget changes are ADRs.

**QG15. Security disclosure policy.** `SECURITY.md` describes how to report vulnerabilities (email, GPG key, response time). Security issues handled privately until fixed, then disclosed via CVE-style advisory if severity warrants.

**QG16. Dependency auditing in CI.** `cargo audit` blocks releases on vulnerable dependencies. `cargo-deny` configured for license and dependency policy enforcement starting Phase 2.

### Contribution quality

**QG17. PRs must pass CI, include tests, update docs, follow Conventional Commits.** Behavioral or architectural changes require an ADR per QG18.

**QG18. ADRs required for specific change classes.** Changes to project guidelines. Additions or breaking changes to public APIs. Changes to the serve-mode protocol. Changes to the package manifest format. Changes to plugin trait boundaries. Other changes (bugfixes, refactors within a crate, docs updates) do not require ADRs.

**QG19. "Done" is defined.** A feature is done when: implemented with tests at appropriate layers, documented in the relevant Diátaxis section, release notes reflect it, any breaking change has migration notes, ADR exists if QG18 requires one. Partial implementations ship behind feature flags or stay on branches.

### Release discipline

**QG20. Release cadence is phase-dependent.** Pre-1.0: on-demand when meaningful changes accumulate. Post-1.0: predictable minor-release cadence (monthly or quarterly) with patch releases on-demand for critical fixes.

**QG21. Every release satisfies a checklist.** CI green on all target platforms. CHANGELOG.md updated with changes, breaking changes, migration notes. Version bumped per SemVer. Git tag pushed. Artifacts published (crates.io, cargo-dist binaries, SDK packages if applicable). Release notes attached to the GitHub release.

### Anti-shortcut practices

**QG22. Code review is required, even for solo maintainer.** In a solo-maintainer project there is no second pair of eyes by default. Approximate one: every change waits overnight before merge for fresh-eyes re-review, OR invites an external reviewer if one is available, OR at minimum completes a review self-checklist (tests exist, edge cases considered, docs updated, no accidental regressions).

**QG23. Every bug becomes a test.** Any bug found in tau, internally or by a user, produces a regression test before the fix lands. The fix is not done when it works; the fix is done when a test ensures it stays fixed.

**QG24. No "refactor later" debt.** If code goes in that needs refactoring, it either gets refactored in the same PR or an issue with explicit scope is filed and tagged `tech-debt`. No silent debt accumulation.

**QG25. No dependency is added without justification.** Each new dependency in a `Cargo.toml` is justified in the PR description: why this crate, why not std, what is the license, how actively maintained. This prevents the slow accumulation of supply-chain risk.

---

## 4. Process — how work happens

**PG1. Work is planned in `ROADMAP.md` and tracked in GitHub issues.** ROADMAP lives in the repo root, documents current phase, near-term priorities, and explicitly-out-of-scope items. Updated at phase transitions. Individual work items are GitHub issues. No milestones or project boards in Phase 0-3.

**PG2. External contributions are gated by alignment, not restricted by volume.** Phase 0-1: bugfixes and documentation PRs welcome directly; feature contributions require an issue discussion before a PR. Phase 2+: PRs aligned with published guidelines and non-goals welcome regardless of origin. LLM-generated PRs face the same alignment bar; provenance does not excuse misalignment.

**PG3. Decisions below ADR threshold are recorded in commit messages and PR discussion.** Commit bodies explain the why, not just the what (Conventional Commits format). PR descriptions capture the reasoning. No separate `docs/notes/` directory (decisions there go to die).

**PG4. Phases close with a retrospective.** `docs/retrospectives/phase-NN.md` at each phase boundary covers: what shipped, what slipped, what was learned, what changes to guidelines or plans follow. ROADMAP.md updates with the next phase's priorities. Retrospectives are public artifacts.

**PG5. User communication is release-notes-first.** Phase 0-1: release notes are the sole user-facing communication channel. Phase 2+: a blog (in-repo or external) complements releases with philosophy, architectural essays, post-release reflections. Chat channels deferred until clear community demand; empty chat channels are worse than none.

### Amendment process

This document changes only via ADRs in `docs/decisions/`. ADRs that propose guideline changes:

1. Explain what guideline is being added, modified, or removed.
2. Explain the situation that motivated the change (new evidence, changed context, discovered contradiction).
3. State the replacement text explicitly.
4. Reference any PRs, issues, or retrospectives that contributed to the decision.

Approval: for a solo-maintainer project, the maintainer decides. In the overnight-delay spirit of QG22, a guideline change waits at least 24 hours between drafting the ADR and merging it, unless the guideline change is a typo or formatting correction.

---

## Appendix A — Guideline index

Total: **59 guidelines.**

| Prefix | Count | Section |
|--------|------:|---------|
| G1-G17 | 17 | Identity |
| NG1-NG12 | 12 | Non-goals |
| QG1-QG25 | 25 | Quality |
| PG1-PG5 | 5 | Process |

## Appendix B — Reading order for contributors

1. §1 Identity — what tau is
2. §2 Non-goals — what tau is not (often clarifies more than §1)
3. §3 Quality — the bar
4. §4 Process — how work happens
5. `ROADMAP.md` — current phase, near-term priorities
6. `CONTRIBUTING.md` — mechanics of contributing
7. `docs/decisions/` — ADRs for deep context on specific decisions

## Appendix C — Terminology

- **Agent** — a process that consumes messages, produces messages, may call tools, may invoke sub-agents, with behavior shaped by a definition (prompt, config, LLM backend) that tau instantiates.
- **Core** — tau itself. The four-verb runtime: install, run, message, observe.
- **Extension** — any installable package. LLM backend, tool, pipeline, skill, MCP server, SDK, storage backend.
- **Pipeline** — a package that orchestrates multiple agents through a methodology. Stature is an example.
- **Skill** — a reusable behavior package following the Agent Skills spec.
- **MCP** — the Model Context Protocol, a standard for tools exposing capabilities to agents.
- **Scope** — installation scope: global (user-wide) or project-local (`.tau/` in a project).
- **Serve mode** — tau running as a long-lived subprocess, speaking JSON-RPC over stdio or a socket. Primary production-embedding mechanism.
- **Stature** — the opinionated coding pipeline, a separate downstream project built on tau.
