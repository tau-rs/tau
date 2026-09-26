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

`codex-0.154.0/` is the [#128](https://github.com/tau-rs/tau/issues/128)
gate, recorded on 2026-09-26 while signed in with a ChatGPT account:
`0-login-status` is the auth probe, `1-hello` through `4-resume` are the
runs #128's gating comment asked for, `5-model-rejected` is a backend 400
(`-m gpt-5-codex`, which that account does not serve), and `6-stdin-held`
holds stdin open for five seconds after spawn: nothing is printed until it
closes, because `codex exec` with a piped stdin reads it to EOF as more
prompt before starting. Three things the recording pinned that the gate
did not know: the `brew` pin `0.46.0` refuses every model for a ChatGPT
account (`gpt-5.5` answers "requires a newer version of Codex"), so the
pin is the `0.154.0` Conductor ships; the strict structured-output
validator behind `--output-schema` rejects `oneOf` and any object without
`additionalProperties: false`, so these runs pass the projection at
`fixtures/agent/envelope-schema.openai-strict.json`, not the committed
schema; and with a schema in force every `agent_message` is a schema-shaped
object, including the model's opening "I am about to..." one, so the
envelope is the last `agent_message` of the turn, never the first. The
prompt is `<contract>\n\nTask: ...` as one argv, so there is no `stdin`
record; `stdin_closed` carries `at_ms` only when the runner delayed it.
Where the CLI died by a signal, `exit.code` is the shell's `128 + n`.

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

A `stdout` record that was cut says so in `cut`. Nothing else is altered,
and the same secret-shape guard that covers the provider cassettes covers
these files.

## Moving the pin

A new CLI version is a new directory beside this one, with its own
recording. Do not re-record over an old pin: the old one is what its
adapter's mapping was tested against, and the drift job compares against
it.
