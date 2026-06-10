# ADR-0040: `tau dev` REPL + the β.7/β.7.5 split

**Status:** Accepted
**Date:** 2026-06-10
**Supersedes:** none

## Context

β.7 as originally written in `ROADMAP.md` bundled two distinct deliverables:
the `tau dev` hot-reload REPL (Vercel-DX feel for the engine) and the
ahead-of-time IR-to-wasm compiler ("AOT lands in β.7" footnote on β.2).
After β.3 PR-5/PR-6 expanded the MCP facilitator's surface significantly,
the in-wasm MCP-facilitator path's complexity ballooned, making the bundled
β.7 a 6–10 week sub-project with a high-risk tail (wasm component model is
a moving target; no prior art for agent harnesses).

## Decision

1. **Split β.7 into two sub-projects:**
   - **β.7 (this ADR):** REPL only — `tau dev <project>` over the existing
     β.3 runtime path. ~2 weeks.
   - **β.7.5 (separate, ADR-0041 forthcoming):** IR-to-wasm AOT compiler.
     ~4–8 weeks.

2. **REPL uses explicit `:reload`, not auto-reload by default.** Industry
   prior art for agent dev loops is sparse (Mastra is the only meaningful
   one, and it picked Next.js-style auto-reload). For agents specifically,
   auto-reload destroys the iterative debug loop where the user wants to
   tweak an agent mid-conversation without restarting from turn 0. Erlang/
   Elixir's REPL with `recompile` is the better prior art. `--watch` flag
   opts into auto-reload for users who prefer Mastra's UX.

3. **Manifest-only hot reload in v1.** Tool code reload requires the TS
   surface (β.8); shipping it in β.7 would require an embedded JS engine
   (QuickJS/V8) or a Rust dylib reload story, both significant scope.

## Consequences

Positive:
- `tau dev` ships fast (~2 weeks) and unblocks β.8 + β.6 design work.
- AOT gets its own focused sub-project with its own ADR + conformance scope.
- The REPL's explicit-reload semantics let users iterate mid-conversation
  without losing context.

Negative:
- ROADMAP β.2's footnote needs amending (deferred AOT one phase).
- γ.1's dependency line gains β.7.5 (cosmetic).
- Two sub-projects to ship instead of one larger one — slightly more
  coordination overhead.

## Alternatives considered

- **Ship β.7 bundled (REPL + AOT) as originally specced:** rejected because
  AOT's complexity post-β.3 makes the bundled sub-project too large for one
  spec to manage; design holes more likely to slip through.
- **Mastra-style auto-reload as default:** rejected because the agent debug
  loop benefits from explicit reload (see §2 above).
- **Skip the REPL, go straight to AOT:** rejected because the REPL is the
  ergonomic on-ramp that the philosophy doc promises; deferring it leaves
  a hole in the Vercel-DX-feel story until β.7.5 + γ.1 both ship.

## References

- Spec: `docs/superpowers/specs/2026-06-10-beta-7-tau-dev-design.md`
- Plan: `docs/superpowers/plans/2026-06-10-beta-7-tau-dev.md`
- Philosophy: `docs/explanation/tau-philosophy.md` (DEV column of the
  two-profiles diagram)
- Related ADRs: 0037 (workflow IR), 0038 (MCP facilitator), 0039 (CI strategy)
