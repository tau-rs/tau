# `tau trace` — execution-trace waterfall TUI

`tau trace` opens the execution-trace waterfall TUI (M1) on a run's JSONL
trace log. It reads the same `<scope>/.tau/runs/<run-id>.jsonl` file described
in [Multi-agent orchestration](../explanation/multi-agent-orchestration.md),
so it works as a post-mortem viewer for a finished run and, when pointed at
a run that is still writing, as a live-attach follower — it tails the file,
folding each new line into the view as it is appended.

## Synopsis

```
tau trace <RUN_ID>
tau trace --last
```

## Arguments

| Argument / flag | Type | Description |
|---|---|---|
| `<RUN_ID>` | positional string | Run id to open; resolves to `.tau/runs/<RUN_ID>.jsonl` under the current directory's project scope. Errors (with a list of available run ids) if that file doesn't exist. |
| `--last` | flag | Open the most recently modified `*.jsonl` file under `.tau/runs` instead of an explicit id. Mutually exclusive with `<RUN_ID>`. |

Exactly one of `<RUN_ID>` or `--last` must be given; omitting both errors
with a hint to pass one.

## Live-attach

`tau trace` doesn't distinguish a finished run from an in-progress one: it
opens the file, reads to EOF, then keeps polling for appended lines. Running
`tau trace --last` (or `tau trace <run_id>` with the id of a run started in
another terminal) against a run that is still executing tails it live, the
same way `tau run --tui` does — the two share the same interactive shell and
renderer. See [`--tui` under `tau run`](../explanation/multi-agent-orchestration.md#observing-a-run-live)
for the difference between attaching after the fact and opening the TUI
in-process at `tau run` time.

## Keybindings

| Key | Effect |
|---|---|
| `↓` | Select the next span. Also re-arms follow mode (auto-scroll to the newest span) once you catch back up to the newest visible row. |
| `↑` | Select the previous span. Disarms follow mode — the view stops auto-scrolling while you browse history. |
| `Enter` | Toggle the selected span's expanded-detail state. Reserved: not yet reflected in the rendered view — a later milestone consumes it. |
| `/` | Enter search mode: subsequent characters filter spans by label substring. `Backspace` deletes; `Enter` or `Esc` exits search mode (without quitting). |
| `f` | Cycle the row filter: `All` → `Errors` → `Tools` → `Reasoning` → `All`. |
| `q` / `Esc` | Quit (outside search mode). |

## See also

- [Multi-agent orchestration](../explanation/multi-agent-orchestration.md) —
  what a `TraceEvent` is and how the JSONL trace log is produced.
