// VISION FIXTURE — target state, not yet buildable.
//
// A pipeline definition: runs ONLY at build time, inside tau's synth
// sandbox — the way a Terraform file is only ever read by terraform. It
// is not part of the app bundle; nothing in src/ imports this file.
// One file = one pipeline; the id ("triage") comes from the file path.
//
// The typed-reference rule (2026-09-06 review): you write a string only
// when DECLARING a new name (step ids). Every REFERENCE to declared
// vocabulary — agents, tools, output fields — is a generated typed
// symbol: a typo is a compile error, autocomplete lists exactly your
// vocabulary. Field access (t.output.category) is a typed proxy that
// compiles to the locked JSON-pointer read — typed surface, unchanged
// semantics.

import { pipeline } from "tau";
import { agents, tools } from "../gen";

export default pipeline((p) => {
  const t = p.agent("classify", agents.triage, { input: p.input });

  p.branch("flagged?", t.output.billing_flag.isTrue(), (b) => {
    b.tool("billing", tools.billing.lookup, {
      customer: t.output.customer_id,
    });
  });

  p.check("categorized", t.output.category.isNonEmpty());

  return { verdict: t.output };
});
