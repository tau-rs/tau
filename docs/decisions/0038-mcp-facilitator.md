# ADR-0038 — MCP facilitator (β.3)

**Status:** Placeholder. Finalized in PR-6 of the β.3 sub-project, after
implementation truth is captured.

**Date:** 2026-06-01 (placeholder); finalize date set at PR-6 merge.

**Context:** ROADMAP §β.3 — MCP host runtime + capability gate at the
contract boundary + Workflow IR integration via the existing
`ToolImpl::Mcp` variant. See the
[β.3 design spec](../superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md)
for the locked architectural decisions (Q1–Q8) and the
[β.3 PR-1 plan](../superpowers/plans/2026-06-01-beta-3-mcp-facilitator-pr-1.md)
for the foundation this ADR will eventually document.

**Decision:** _Final ADR text authored in PR-6 from implementation
truth. The locked architectural decisions list in the design spec is
the authoritative source; PR-6 transposes them into ADR form with any
post-implementation revisions._

**Consequences:** _See PR-6._

**Supersedes / Superseded by:** none.

**References:**

- [β.3 design spec](../superpowers/specs/2026-06-01-beta-3-mcp-facilitator-design.md)
- [tau philosophy](../explanation/tau-philosophy.md) (the MCP FACILITATOR
  block in *The architecture, in one picture*)
- ADR-0037 (workflow IR — defines the `ToolImpl::Mcp` variant β.3 wires up)
- ADR-0035 (bundle format — extended by β.3 to embed pinned contracts)
