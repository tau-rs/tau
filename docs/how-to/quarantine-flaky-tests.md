# How to quarantine a flaky test

If a test fails intermittently (≥5 times in a rolling 7-day window) without a clear root cause, quarantine it. Quarantined tests still run but their failures are non-blocking — surfaced in CI output but don't fail the run.

## When to quarantine

- The test fails ≥5 times in 7 days without a code change that should affect it.
- You've tried to reproduce locally and can't.
- Triaging would take longer than ~30 min and is blocking other work.

## How to quarantine

1. Find the test's full name from a failing nextest run (e.g. `tau-cli::cmd_chat_persistence::chat_ephemeral_writes_no_file`).

2. Edit `.config/nextest.toml`. Add a new `[[profile.ci.overrides]]` block:

   ```toml
   [[profile.ci.overrides]]
   # Quarantined 2026-MM-DD by @<your-handle>
   # Reason: <one line — what's flaky + link to ≥2 failing run URLs>
   # De-quarantine TODO: <what we'd need to do to root-cause>
   filter = 'test(/chat_ephemeral_writes_no_file/)'
   failure-output = 'final-fail'
   success-output = 'final-fail'
   retries = 3
   ```

3. Open a PR with title `test(quarantine): <test name>`. Link the failing runs in the PR body.

4. After the PR merges, file a follow-up issue with label `quarantined-test` to track the de-quarantine work.

## How to de-quarantine

- Identify the root cause (race, env dependency, infra flake masquerading as test bug).
- Fix the test OR the infrastructure.
- Open a PR removing the `[[profile.ci.overrides]]` block.
- Watch the test for ≥7 days post-merge; if it doesn't flake, close the `quarantined-test` issue.

## Automation

Currently quarantine promotion is manual. Auto-promotion based on rolling-window flake counts is a planned follow-up (not yet implemented).
