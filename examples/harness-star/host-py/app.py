# VISION FIXTURE — target state, not yet runnable.
# The team's ENTIRE harness: ~40 lines. Their existing Python integrations,
# their approval UI — nothing else. Everything agentic is tau's, across
# the socket.

from fieldbook_harness import Harness, HostTools, types

import promql_client
import pagerduty_client
import runbook_runner
import slack_ui


class Tools(HostTools):  # exactly the declared obligations, typed
    def metrics_query(self, req: types.MetricsQueryIn) -> types.MetricsQueryOut:
        return types.MetricsQueryOut(series=promql_client.query(req.query, req.range))

    def pager_ack(self, req: types.PagerAckIn) -> types.PagerAckOut:
        return types.PagerAckOut(acked=pagerduty_client.ack(req.incident_id))

    def runbook_apply(self, req: types.RunbookApplyIn) -> types.RunbookApplyOut:
        status, log = runbook_runner.apply(req.plan)
        return types.RunbookApplyOut(status=status, log=log)


def on_approval(e: types.ApplyFixElicitation) -> types.ApplyFixDecision:
    # A human decides, in Slack. Typed both ways; the run stays suspended
    # (and journaled) until this returns.
    return slack_ui.prompt_apply_fix(e.plan, e.rationale)


def main(alert_id: str) -> None:
    h = Harness.connect(tools=Tools(), on_approval=on_approval)
    session = h.session("oncall-investigate")
    for event in session.run(types.AlertRef(id=alert_id)):
        slack_ui.render(event)  # typed run events: steps, checks, budget


if __name__ == "__main__":
    import sys

    main(sys.argv[1])
