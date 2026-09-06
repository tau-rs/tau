// VISION FIXTURE — target state, not yet buildable.
// Exposed pipeline #2: the approval-gated fix. The "apply-fix" check is
// routed to the host by the harness declaration ([harness.approvals]) —
// the run SUSPENDS, the host's human answers a typed elicitation, the run
// resumes with the decision. The pipeline itself stays ordinary.

import { pipeline } from "tau";
import { agents, tools, type RunbookRef, type Applied } from "../tau.gen";

export const runbookExec = pipeline("runbook-exec", (p) => {
  const exec = p.agent("prepare", agents.runbookExecutor, { input: p.input });

  // Typed elicitation point: suspends until the host resumes with an
  // ApplyFixDecision (resume_schema). Declared here; routed by TOML.
  const approval = p.check("apply-fix", exec.output.field("plan").isNonEmpty(), {
    elicit: true,
  });

  p.branch("approved?", approval.decision.field("approved").isTrue(), (b) => {
    const applied = b.tool("apply", tools.runbook.apply, {
      plan: exec.output.field("plan"),
    });
    b.tool("ack", tools.pager.ack, { incident: p.input.field("incident_id") });
  });

  return { result: exec.output.as<Applied>() };
});
