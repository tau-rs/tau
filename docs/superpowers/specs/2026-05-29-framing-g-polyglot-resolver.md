# Framing G — polyglot resolver

**Status:** Scoping document. Not a spec. Must land before any resolver
work begins. Should land before engine implementation, because it informs
how the IR resolves dependencies.

**Date:** 2026-05-29.

**Relates to:** [`docs/explanation/tau-philosophy.md`](../../explanation/tau-philosophy.md)
acknowledged risk G.

---

## Why this needs framing first

tau's package-manager posture is "polyglot resolver + content-hashed
lockfile over crates.io, npm, the MCP registry, Anthropic skills, and git
URLs — no walled registry." This is sound directionally but **uncharted in
combination**: no tool spans this set today.

Each ecosystem has its own solver semantics: cargo's strict SemVer, npm's
loose SemVer with peer-dep complications, MCP's exact-pin reachability,
Anthropic skills' bundle hashing, git's revision/tag/branch identification.
Trying to unify them is years of work that delivers no immediate user value.

**The scope-discipline outcome:** Phase 1 does *not* unify ecosystems. tau
owns a thin meta-layer; each native ecosystem keeps its own resolver.

---

## Decisions this framing must reach

### G-1. The five source classes and tau's posture toward each

| source | Phase 1 posture | Why |
|---|---|---|
| **tau-native units** (agent templates, workflow templates, capability profiles) | tau owns: git URL + content-hash lockfile (Go-modules pattern) | only thing tau *must* solve; the proven pattern |
| **crates.io** (Rust native-tool code) | tau delegates: `cargo add` / `Cargo.toml`; tau reads the resolved `Cargo.lock` | cargo is the canonical solver; reinventing loses |
| **npm** (TS native-tool code, future) | tau delegates: rely on npm/pnpm; tau reads `pnpm-lock.yaml` / `package-lock.json` | same |
| **MCP registry** (external servers) | tau contracts at runtime; the registry is a discovery convenience, not a resolution input | servers aren't installed by tau; they're contracted |
| **Anthropic skills** | already shipped (skill import/export); content-hashed in `lock.toml` | preserve current behavior |

The unifying primitive is the **lockfile** — `lock.toml` (currently v6)
records resolved versions and content hashes for everything tau pins,
regardless of source. The resolver-per-source asymmetry is invisible to
consumers because they see one lockfile.

### G-2. Lockfile schema impact

Determine whether the v6 lockfile can absorb tau-native units as a new
entry type, or whether a schema bump to v7 is needed. The current v6 is
flat (single `packages` vec with nested optional plugin/skill); adding
`tau-native unit` as a third inner kind should be additive.

Decide: bump or extend in-place. Either is fine; declare it explicitly.

### G-3. What a "tau-native unit" actually is

The minimal set of things tau owns the distribution of:

- **Agent templates** — declarative agent definitions (manifest fragments)
  that compose into a project.
- **Workflow templates** — same, for workflow automation.
- **Capability profiles** — reusable, named capability sets that tools or
  agents can reference.
- **Context-pipeline presets** — named context-manager configurations.

Each lives at a git URL, pinned by commit + content-hash. No central
registry to operate. Discoverability is by user-curated lists initially;
GitHub topics / a curated awesome-list later.

### G-4. Resolution semantics

For tau-native units: **exact-pin by content hash, no SemVer-style range
solving in Phase 1**. The manifest references a unit by git URL + a tag or
rev; the lockfile pins by content hash. Upgrades are explicit (`tau
update`). This is the simplest defensible semantics; ranges can be added if
demanded.

### G-5. Trust model

git-distributed units can be malicious. Decide the Phase-1 trust posture:

- **Recommendation:** capability declarations of any tau-native unit are
  surfaced at install time (`tau install` shows what capabilities a unit
  pulls in, just like today's plugin install does). Audit happens via the
  capability gate, not via a curated registry. This is consistent with the
  rest of the philosophy.

### G-6. CLI surface

The Phase-1 commands that this framing implies:

```
tau add <git-url>          # adds a tau-native unit, pins by hash
tau update [name]          # re-resolve to latest tag/rev on configured branch
tau list units             # show installed tau-native units + sources
tau verify                 # already exists; verifies unit hashes match lockfile
```

No new package-resolution primitives beyond these in Phase 1.

---

## Out of scope for Phase 1

The framing makes these explicit non-goals:

- A cross-ecosystem version solver (don't unify cargo + npm + MCP semantics).
- A tau-operated package registry / server / website.
- A unit publish workflow beyond "push to git, share the URL."
- Discovery infrastructure (curated lists are out-of-tree).
- Dependency-of-a-dependency resolution across ecosystems (cargo and npm
  each handle their own transitives).

---

## Deliverable shape

The framing is complete when:

1. The lockfile schema decision (G-2) is recorded as an ADR (`0036-…` or
   next).
2. A design spec at `docs/superpowers/specs/<date>-tau-native-units-design.md`
   covers G-3, G-4, G-5, and G-6 with chosen options.
3. An example tau-native unit (one agent template + one workflow template)
   is described concretely — same way Skills-6 reference packages provided
   a concrete starting point for the skills story.

Until that exists, no `tau add` / unit-resolution code lands.

---

## Risk acknowledgment

The hard part is *not* what's in scope here. The hard part is what's
deliberately out of scope: the cross-ecosystem solver. The risk is scope
creep — users will ask for "just one more thing" that tips toward unifying
solver semantics. The mitigation is to point at this document and say no.

The framing succeeds if Phase 1 ships a useful sharing primitive
(git-pinned, content-hashed, capability-audited tau-native units) without
attempting cross-ecosystem unification. Phase 2+ can reconsider if a clear
user need emerges that the delegate-to-host pattern can't meet.
