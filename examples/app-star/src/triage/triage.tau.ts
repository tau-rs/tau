// VISION FIXTURE — target state, not yet buildable.
//
// The co-located pipeline: ONE exported symbol, TWO projections
// (worked-examples B3, invariant 4).
//   • synth time  — this body runs in the sandbox; pipeline() registers
//     choreography into the emitted ProjectConfig. Same lane, id grammar,
//     and single validate() as pipelines/*.ts. Sugar, literally.
//   • app runtime — the same `triage` export resolves through the
//     generated typed client to an invocation handle bound to the PINNED
//     bundle. This body never executes in the app process.
//
// Design §4 API rules apply: typed non-coercible handles, interpolation
// only via the tau`` tagged template, predicates as handle methods.

import { pipeline, tau } from "tau";
import { agents, tools, type TicketIn, type TriageVerdict } from "../../tau.gen";

export const triage = pipeline("triage", (p) => {
  // Choreography only. Prompts, model, and tool grants live in
  // agents/triage.md and tau.toml — a definition here is a synth error.
  const t = p.agent("classify", agents.triage, { input: p.input });

  p.branch("flagged?", t.output.field("billing_flag").isTrue(), (b) => {
    b.tool("billing", tools.billing.lookup, {
      customer: tau`${t.output.field("customer_id")}`,
    });
  });

  p.check("categorized", t.output.field("category").isNonEmpty());

  return { verdict: t.output.as<TriageVerdict>() };
});
