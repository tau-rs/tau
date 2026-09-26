# Agent CLI transcripts

Recorded runs of the agent CLIs (ADR-0013 §10), one directory per CLI at one
pin, one file per run: `cli/<cli>-<version>/<n>-<run>.jsonl`. They are the
fixtures the envelope parser, the event mapping and `tau-fake-cli`'s
scripts are tested against. `cassette_guard.rs` checks every file here on
every run.

## Provenance

`claude-2.1.272/` is the seven runs of
[#130](https://github.com/tau-rs/tau/issues/130), recorded on 2026-09-16
by the runner script in that issue's gist, converted by hand once.

`codex-0.157.1/` is the runs of
[#128](https://github.com/tau-rs/tau/issues/128), recorded on 2026-09-26
on a machine signed in with ChatGPT, by a `codex` variant of the same
runner that writes these records directly. The CLI came from
`npm install @openai/codex@0.157.1` into a scratch prefix, sharing the
machine's login; the model was the CLI's default. Run 0 is the login probe
(nothing on stdout; the mode line is on stderr). Runs 1 to 3 are the
recipe: the task to completion, then `SIGINT` and `SIGTERM` to the process
group two seconds after `turn.started`. Run 4 is the recipe's `exec resume`
argv, which has no sandbox flag, so the CLI ran read-only and the envelope
says `failed`; run 5 is the same resume with
`-c sandbox_mode="workspace-write"`, and shows a resumed thread working in
the resumer's cwd, not the thread's original one. Run 6 is the first
attempt at run 1: OpenAI's structured-output mode rejected the envelope
schema as committed before #128 (an open nested object, then `oneOf`), and
the turn failed before any item; `envelope::schema()` now closes every
object and spells enums `anyOf`.

`codex-0.46.0/` is one run on the pin ADR-0013 §7 named: on 2026-09-26 that
version was refused every model for a ChatGPT login (five retries, then
`turn.failed`, exit 1), so the pin moved. The directory stays as the
record of why.

## Format

One JSON object per line, each with a `tau` key naming the record:

| `tau` | fields | meaning |
|---|---|---|
| `header` | `v`, `cli`, `version`, `recorded_at`, `run`, `source`, `argv`, `scrubbed` | the first line; `<cli>-<version>` is the directory's name |
| `stdin` | `at_ms`, `line` | a line the runner wrote on the CLI's stdin |
| `stdin_closed` | | the runner closed stdin |
| `signal` | `at_ms`, `name` | the runner sent this signal to the CLI |
| `stdout` | `at_ms`, `line`, `cut`? | a line the CLI printed, verbatim except where `cut` says otherwise |
| `exit` | `code`, `elapsed_ms`, `stderr`, `files` | the last line: how the CLI ended, and what it left in its cwd |

`at_ms` is milliseconds since spawn, as the runner clocked it. Records are
in the order the runner logged them.

## Scrubbing

Per ADR-0013 §10, and listed in every header's `scrubbed`:

- the `init` event's `plugins`, `slash_commands` and `skills` lists are
  cut, because they are one user's machine, not the CLI's surface;
- a `hook_response` event keeps `type`, `subtype` and `hook_name` only:
  the body is one user's hook output.

For the `codex` runs: the `--output-schema` path in `argv` is made
repo-relative, and the account name a `command_execution` item echoed in
`aggregated_output` (`ls -l`) is `<user>`.

A `stdout` record that was cut says so in `cut`. Nothing else is altered,
and the same secret-shape guard that covers the provider cassettes covers
these files.

## Moving the pin

A new CLI version is a new directory beside this one, with its own
recording. Do not re-record over an old pin: the old one is what its
adapter's mapping was tested against, and the drift job compares against
it.
