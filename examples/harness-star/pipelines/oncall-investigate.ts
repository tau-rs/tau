// VISION FIXTURE — target state, not yet buildable.
// Exposed pipeline #1: investigation only. The agent uses a host tool
// (metrics.query) exactly like a native one — the difference lives on the
// card, not in the choreography.

import { pipeline } from "tau";
import { agents, type AlertRef, type Findings } from "../tau.gen";

export const investigate = pipeline("oncall-investigate", (p) => {
  const inv = p.agent("investigate", agents.investigator, { input: p.input });

  p.check("evidence-backed", inv.output.field("evidence").isNonEmpty());

  return { findings: inv.output.as<Findings>() };
});
