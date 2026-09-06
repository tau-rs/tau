// VISION FIXTURE — target state, not yet runnable.
// Transport parity (worked-examples C6): the SAME bundle, the SAME
// harness card, embedded in the internal web console as a wasm component.
// Obligations are satisfied by implementing the WIT world's imports —
// the card doesn't care which transport the host chose (invariant 7).

import { instantiate } from "./harness_star_component.js"; // jco-style bindings

const engine = await instantiate({
  // one import per declared [[harness.host_tool]] — same three obligations
  "metrics.query": (req) => consoleApi.prometheus(req.query, req.range),
  "pager.ack":     (req) => consoleApi.pagerduty.ack(req.incident_id),
  "runbook.apply": (req) => consoleApi.runbooks.apply(req.plan),
  // approvals: the elicitation surfaces as a component export → UI dialog
  "elicit.apply-fix": (e) => consoleUi.approvalDialog(e),
});

// Missing an import? Instantiation refuses with the unmet obligation
// named — the wasm twin of E-HARNESS-OBLIGATIONS.
const run = engine.session("oncall-investigate").run({ id: alertId });
for await (const event of run) consoleUi.render(event);
