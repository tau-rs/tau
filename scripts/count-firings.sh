#!/usr/bin/env bash
# The throttle detector every Tier 3 workflow runs last (docs/HANDOFF.md §8;
# issue #7's first landmine; #156, #181).
#
# Counts *scheduled* runs of WORKFLOW created on the same UTC calendar day
# as this one. On a scheduled run the count includes this one, so exactly 1
# is healthy: 0 cannot happen, 2+ means the cron is firing more often than
# its expression. The window is a calendar day and not "the last 24 h"
# because GitHub's scheduler runs a cron hours late with jitter either way:
# a sliding window anchored at this run caught yesterday's run on every
# night that started earlier in the hour than the night before (#156). Keep
# every cron away from midnight, so a delay that crosses the day boundary
# is worth the report it produces.
#
# v1's crons fired every ~11-12h regardless of their expression, for
# months, undetected. "Ran recently" would not have noticed; a count does.
#
# On a manual run the count is written to the summary and nothing is filed:
# a dispatch inside the window would otherwise report a false double firing.
# A wrong count files one issue per workflow, deduplicated by open title.
#
# Inputs (environment): WORKFLOW (the file name, e.g. tier3.yml), CRON (its
# expression, for the report), EVENT (github.event_name), RUN_URL, REFS (the
# issues the report cites), GH_TOKEN, GITHUB_REPOSITORY; GITHUB_STEP_SUMMARY
# when present.
set -euo pipefail

WORKFLOW="${WORKFLOW:?the workflow file name}"
CRON="${CRON:?the cron expression}"
EVENT="${EVENT:?the event that started this run}"
RUN_URL="${RUN_URL:-}"
REFS="${REFS:-#7}"

day="$(date -u '+%Y-%m-%d')"
count="$(gh api -X GET "repos/$GITHUB_REPOSITORY/actions/workflows/$WORKFLOW/runs" \
  -f event=schedule -f created="$day" -F per_page=100 --jq '.total_count')"
echo "scheduled firings of $WORKFLOW on $day (UTC): $count (event: $EVENT)"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  echo "- scheduled firings of \`$WORKFLOW\` on $day (UTC): $count (this run: \`$EVENT\`)" >> "$GITHUB_STEP_SUMMARY"
fi
if [ "$EVENT" != "schedule" ]; then
  echo "not a scheduled run; count is informational"
  exit 0
fi
if [ "$count" -eq 1 ]; then
  exit 0
fi
title="tier3: $WORKFLOW did not fire exactly once today (UTC)"
existing="$(gh issue list --state open --limit 500 --json number,title \
  | jq -r --arg t "$title" '.[] | select(.title == $t) | .number' | head -1)"
if [ -n "$existing" ]; then
  gh issue comment "$existing" --body "Again: $count scheduled firing(s) on $day (UTC), counted by $RUN_URL."
  echo "already open as #$existing"
  exit 0
fi
body="$(cat <<EOF
\`$WORKFLOW\` counted $count scheduled run(s) of itself created on \`$day\` (UTC, this run included). Its cron is \`$CRON\`, so the healthy count is exactly 1.

v1's crons fired every ~11-12h regardless of their expression, for months, undetected. This is the counter that is supposed to notice. Check the run list for the workflow, then decide whether the schedule moved or GitHub's scheduler did.

- run: $RUN_URL

refs $REFS
EOF
)"
url="$(gh issue create --title "$title" --label status:ready --body "$body")"
echo "opened $url"
