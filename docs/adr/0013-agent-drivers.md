# ADR-0013: Agent drivers — the `claude` and `codex` CLIs as subprocess tools behind `send`

**Status:** Accepted
**Date:** 2026-09-16
**Deciders:** tau core
**Amends:** nothing. Extends [ADR-0006](0006-model-bridge-contract.md) (a
second driver family beside the model drivers, with its own wire and its own
mapping table) and follows [ADR-0009](0009-sandbox-driver.md) (a subprocess
behind one `send`, `abandon` as a signal ladder, `error.lost` bills the
ceiling). Evidence: [#130](https://github.com/tau-rs/tau/issues/130), the
CLI surfaces as pinned on 2026-09-16.

## Context

### In plain language

tau has one way to make a language model do work: a *model driver* speaks a
provider's HTTP API, answers one completion per `send`, and the tool loop in
`libtau` does the rest — call a tool, feed the result back, call the model
again. That family is [ADR-0006](0006-model-bridge-contract.md). It is paid
for with an API key.

Two pasted handoffs (#126) ask for something different: hand a *whole task*
to the user's own Claude Code or Codex command-line tool, running as a child
process, and get back a structured job report. The loop runs inside the CLI,
not in `libtau`. The CLI owns its login, whether that is a subscription or an
API key, and tau never sees a credential.

One analogy. A model driver is a typist: you dictate one sentence, read it
back, dictate the next; every keystroke is yours to pay for and yours to
steer. An agent driver is a contractor: you hand over a work order and the
keys to one workshop, and later you get a signed report — what was done,
what was assumed, what was left. You never see the contractor's bank account,
and you cannot lean over their shoulder mid-job. Both are useful; they are
not the same kind of thing, and this ADR keeps them apart.

Two axes fix where the family sits: *what you get* (a raw completion or a
whole task) and *who pays* (an API key or a subscription).

|  | API key | Subscription |
|---|---|---|
| **raw completion** — loop in `libtau` | model drivers (ADR-0006) | *empty — deliberate* |
| **whole task** — loop inside the CLI | agent drivers (this ADR), CLI given a key | agent drivers (this ADR), CLI logged in |

The empty cell is the point. Reaching it would mean taking the CLI's OAuth
token and speaking the provider's API with it, which both vendors ban and
which Anthropic has already acted on. That cell stays empty on purpose, and
the agent-driver row exists so that a subscription is usable at all.

The agent-driver row straddles both billing cells with one code path. The
harness picks the cell by the environment it hands the child (a key in the
environment, or none); the driver reports whatever mode the CLI *states*, as
an opaque string; and no type in this crate names a billing mode. Anthropic
changed its programmatic-versus-interactive billing policy twice in 2026;
a `BillingMode` enum would have been wrong twice.

### What the handoffs assumed that tau does not have

The handoffs describe a driver "spawned by the kernel and mapped onto all
seven syscalls", with `send` steering a running task, `recv` yielding typed
events, and log-level crash resume. That is not tau's driver model. A driver
is a `Driver` impl — `handle(Delivery) -> (reply, Consumption)`,
`abandon(corr)`, `describe()` — reached by one `send` and answering with one
`Reply` ([`driver.rs`]). It has no other surface. Three things the handoffs
lean on are not in v1: `MsgKind::Partial` streaming (ADR-0006, "not in v1"),
driver-health entries in the log ([#122](https://github.com/tau-rs/tau/issues/122),
spec-only), and log-level resume across a driver restart (M4). This ADR
settles what the family looks like *inside* the surface that exists, so the
implementation lanes ([#131](https://github.com/tau-rs/tau/issues/131), then
[#127](https://github.com/tau-rs/tau/issues/127) and
[#128](https://github.com/tau-rs/tau/issues/128) in parallel) share one
contract instead of forking it.

### What #130 found that neither handoff knew

The spike ran the pinned CLIs on the tasks the handoffs describe. Five of
its findings change the design below, and they are stated here once:

1. **A bare `claude -p` is not headless.** It loads the user's global
   `CLAUDE.md`, hooks, plugins and skills; the hook output arrives on stdout
   *before* the `init` event, and the plugins' text leaked into the worker's
   envelope. `--safe-mode` turns all of that off while keeping the login
   working; `--bare` is unusable for a subscription because it never reads
   OAuth. `--safe-mode` is therefore not a preference but part of the
   driver's fixed argv.
2. **`claude` has an in-band interrupt** on its stream-json stdin — a
   `control_request` with `subtype: interrupt`, acknowledged by a
   `control_response`, followed by a terminal `result` with usage and exit 1.
   `SIGINT` does the same abort with exit 0. `SIGTERM` prints nothing more
   and exits 143: no terminal event, no usage. That is the ladder of §5.
3. **`claude --max-turns` is hidden but works**, and a run that hits it ends
   with `subtype: error_max_turns`, `result: null`, and no envelope: the
   model gets no turn to write one. A bound hit is a *stop reason*, not a
   `partial` envelope (§2).
4. **`codex exec` on an unsigned machine does not fail fast**: it emits
   `thread.started`, `turn.started`, then loops on `401 Unauthorized`
   retries until killed. The probe of §6 is mandatory for `codex`, not a
   nicety, and a run-time 401 is a terminal event the adapter must
   recognise.
5. **`codex exec --ask-for-approval` does not exist**; the same option is
   accepted before the subcommand (`codex -a never exec …`). `--full-auto`
   is `-a on-failure`, which can still block.

The flags named in §7 are those #130 ran; where a row says *#127* or
*#128*, the adapter lane pins it from its own transcript, and a row this
ADR gets wrong is corrected here, as an amendment, before that lane starts.

### Constraints

- **The kernel never parses payloads** ([ADR-0003](0003-substrate.md),
  invariant 2). No `kernel/src/` change; `kernel/src/abi/` is frozen
  ([ADR-0004](0004-abi-freeze.md)).
- **Drivers never raise.** A missing binary, a logged-out CLI, a rate limit,
  a malformed final message, a child that ignored every signal: each is a
  reply payload, never an exception or a panic (ADR-0006, "Errors ride in
  the reply").
- **No new crate.** A module under `drivers/`, for the reason ADR-0006 and
  ADR-0009 gave.
- **No clock.** `Instant::now` and `SystemTime::now` are denied; the driver
  bounds time with durations and channels, as the sandbox does, and never
  reports `wall_ms`.
- **Subprocess-only auth.** The driver never reads a credential file, never
  touches the Keychain, never names a provider endpoint. Stated as an
  invariant with a test (§8), not as a review comment.

## Decision

### 1. One module, one feature, one tool per registration

The family is `tau_drivers::agent` (`drivers/src/agent/`), behind a feature
`agent` that is on by default and Unix-only, like `sandbox`. The module
holds everything the two CLIs share, and each CLI is a thin adapter:

```
drivers/src/agent/
  mod.rs        AgentConfig, AgentDriver<C: Cli>, the probe, the ceiling
  wire.rs       Request { v, op, ... }, Reply { v, stop, envelope, ... }, schema()
  envelope.rs   Envelope, its JSON Schema, the tolerant parser, EnvelopeViolation
  process.rs    spawn with an exact environment, bounded drain, the signal ladder
  claude.rs     impl Cli: argv, event mapping, in-band interrupt
  codex.rs      impl Cli: argv, event mapping, SIGINT
```

`Cli` is a crate-private seam, not a public trait: argv for `run` and
`resume`, how to read the session id and the usage out of the CLI's event
stream, which event is terminal, how to interrupt. `AgentDriver<Claude>` and
`AgentDriver<Codex>` are the two drivers; nothing in `libtau` or the kernel
learns either.

Like every driver, one registration is **at most one tool** (ADR-0006 §5): one
CLI binary, one model, one permission cage, one task bound, chosen by the
harness at construction. A harness that wants a cheap reviewer and an
expensive implementer registers two drivers, named `reviewer` and
`implementer` by the operator, each with its own ceiling and capability.

```
agent ──send(cap, request)──▶ kernel ──Delivery──▶ AgentDriver<Claude>     (protocol boundary)
                                                        │ spawn, new process group,
                                                        │ cwd = workspace, env = exactly config.env
                                                        ▼
                                                  claude -p --safe-mode --input-format stream-json
                                                         --output-format stream-json ...
                                                        │ task in as the first stdin message;
                                                        │ its own tool loop, its own login
                                                        ▼
                                                  one JSONL event stream on stdout,
                                                  the last assistant text = the envelope
```

### 2. The wire shape: `v` on both sides, two ops, one reply

The types live in `tau_drivers::agent::wire`, not in `tau_kernel::bridge`,
on ADR-0009's argument: one lane owns the shape, `libtau` renders bytes
verbatim, and the kernel carries no vocabulary it has no neighbour for. The
JSON is the contract; the Rust types are versioned with the crate.

**The request**, one JSON object per `send`, tagged by `op`:

```json
{
  "v": 1,
  "op": "run",
  "task": "Add a `--json` flag to `tau replay` that prints the state hash as JSON. Keep the existing text output as the default.",
  "workspace": "kernel",
  "tools": ["Read", "Edit", "Bash"],
  "budget": { "cost_microusd": 500000, "turns": 40 }
}
```

```json
{
  "v": 1,
  "op": "resume",
  "session": "affe155e-9ed1-4477-95f0-c17d40bd9a89",
  "task": "The test you added fails on macOS: `Path::new(\"/tmp\")` is a symlink there. Use `tempfile::tempdir()`.",
  "workspace": "kernel"
}
```

| Field | Type | Meaning |
|---|---|---|
| `v` | `u16`, optional | Agent wire version, `1`. Absent means the version this driver's `describe()` projected (ADR-0009 §2's rule). A present `v` the driver does not implement is `error.unsupported`. |
| `op` | `"run"` \| `"resume"` | Start a fresh CLI session, or continue one the CLI already has (§3). |
| `task` | string | The work order. On `run`, the whole task; on `resume`, the amendment. Bounded by the driver's `task_bytes`; over it is `error.unsupported`, nothing spawned. |
| `workspace` | string, optional | A path **relative to the configured `workspace_root`**, the child's `cwd`. Absent is the root itself. A path that escapes the root (`..`, absolute, a symlink out) is `error.unsupported`, nothing spawned. |
| `tools` | array of strings, optional | The CLI-native tool names the session may use. A driver whose CLI has a per-session allowlist honours it; it must be a subset of the configured `tools`, else `error.unsupported`. A driver whose CLI has no such flag refuses a present `tools` (`error.unsupported`); its `describe()` schema omits the field. |
| `budget` | object, optional | A tightening of the registered task bound, honoured where the CLI enforces it, refused (`error.unsupported`) where it does not. `cost_microusd` and `turns`, each optional, each at or under the configured bound. |
| `session` | string, `resume` only | The session id a previous reply reported (§3). Unknown to the CLI is `error.provider` with the CLI's text. |

No `model`, no `effort`, no `permission_mode`, no `env`. Each is the
harness's decision at registration, on ADR-0006 §2's rule — "which model
answers is the driver's configuration, not the request's" — and ADR-0009's:
the requester, which may be a model, does not size its own cage.
`workspace` is relative and `tools` may only narrow for the same reason. A
program that needs one of these is one optional field away, without a `v`
bump, on the day it exists.

**The reply**, one JSON object per `Reply`:

```json
{
  "v": 1,
  "stop": "done",
  "envelope": {
    "status": "ok",
    "summary": "Added --json to tau replay; the text output is unchanged by default. One new test covers the flag.",
    "artifacts": [
      { "path": "kernel/src/bin/tau.rs", "kind": "file" },
      { "path": "kernel/tests/replay_json.rs", "kind": "file" }
    ],
    "assumptions": ["The hash is printed as a lowercase hex string, matching `tau replay`'s text form."],
    "events": ["Ran `just check`; green."],
    "error": null
  },
  "session": "affe155e-9ed1-4477-95f0-c17d40bd9a89",
  "cli": { "name": "claude", "version": "2.1.272" },
  "model": "claude-opus-5",
  "mode": "none",
  "usage": { "input_tokens": 44125, "output_tokens": 381, "cost_microusd": 259148, "turns": 3 },
  "transcript": [
    { "type": "system", "subtype": "init", "session_id": "affe155e-9ed1-4477-95f0-c17d40bd9a89", "model": "claude-opus-5[1m]", "apiKeySource": "none", "permissionMode": "acceptEdits" },
    { "type": "assistant", "message": { "role": "assistant", "content": [ { "type": "tool_use", "id": "toolu_01…", "name": "Write", "input": { "file_path": "…", "content": "…" } } ] } },
    { "type": "user", "message": { "role": "user", "content": [ { "type": "tool_result", "tool_use_id": "toolu_01…", "content": "File created successfully at: …" } ] } },
    { "type": "assistant", "message": { "role": "assistant", "content": [ { "type": "text", "text": "{\"status\":\"ok\",…}" } ] } },
    { "type": "result", "subtype": "success", "is_error": false, "terminal_reason": "completed", "num_turns": 3, "total_cost_usd": 0.259148,
      "usage": { "input_tokens": 4, "cache_creation_input_tokens": 23851, "cache_read_input_tokens": 20270, "output_tokens": 381 }, "result": "{\"status\":\"ok\",…}" }
  ],
  "truncated": { "transcript": false }
}
```

| Field | Meaning |
|---|---|
| `v` | Agent wire version, `1`. |
| `stop` | How the run ended: the table below. |
| `envelope` | The worker envelope (§4) parsed from the CLI's final message, or `null` when there was none to parse. Present on `done`; `null` on every `limit`, every `abandoned` (#130: no interrupt path gives the model a turn to write one), and every error. |
| `session` | The session id the CLI reported, or `null` if it never did. What `resume` takes. |
| `cli` | The binary's name and the version it printed at construction (§6). |
| `model` | The model the CLI said answered, verbatim, or `null`. |
| `mode` | **Verbatim, opaque.** The one string the CLI itself stated about how it is logged in, or `null` if it stated nothing. The driver never interprets it and no type enumerates it. For `claude` it is `init.apiKeySource` (`"none"` on a subscription login); for `codex` it is the one line `codex login status` printed. Never the whole probe output: `claude auth status` carries the user's email and organisation, and those do not belong in the log. |
| `usage` | What the CLI's terminal event reported: `input_tokens` (every input class summed — uncached, cache creation, cache read; the transcript keeps the breakdown), `output_tokens`, and `cost_microusd` and `turns` when it stated them, else `null`. Information for the caller; the accounting is the `Consumption` of §6. |
| `transcript` | The CLI's own event stream, one JSON value per stdout line, parsed and carried unread. Bounded by `transcript_bytes`; the driver keeps the first events up to the bound **and always the terminal one**, and sets `truncated.transcript` if it dropped any. Lines that are not JSON are carried as `{ "raw": "…" }`. |

This is how "every observable driver event lands in the log" holds with one
reply and no `Partial`: the transcript is the reply, the reply is a blob the
log references by hash, and a refold sees exactly what the driver saw.

`stop` is one of:

| `stop` | Meaning | Usage is |
|---|---|---|
| `"done"` | The CLI emitted its terminal event with a final message and exited. `envelope.status` says whether the *task* succeeded; `done` says the *run* did. | real |
| `{ "limit": "turns" }` \| `{ "limit": "cost" }` | The CLI stopped the session at a bound it enforces (`--max-turns`, `--max-budget-usd`). No envelope: the model got no turn to write one (#130 run 6). The caller's `resume` continues it. | real |
| `{ "limit": "wall" }` | The driver's `wall` bound climbed the ladder of §5 and the CLI reported before the last rung. | real if the CLI reported it, else the ceiling |
| `"abandoned"` | The requester was cancelled; the CLI stopped on the ladder of §5 before the last rung. | real if the CLI reported it, else the ceiling |
| `{ "error": { "kind": ..., "message": ... } }` | The driver could not run it, the CLI refused, or the run was lost. | see the table |

```json
{ "v": 1, "stop": { "error": { "kind": "unavailable", "message": "codex login status: exit 1: Error checking login status: No such file or directory (os error 2)" } },
  "envelope": null, "session": null, "cli": { "name": "codex", "version": "0.46.0" }, "model": null, "mode": null,
  "usage": { "input_tokens": 0, "output_tokens": 0, "cost_microusd": null, "turns": null },
  "transcript": [], "truncated": { "transcript": false } }
```

| `error.kind` | When | Billed |
|---|---|---|
| `unsupported` | A `v`, an `op`, a field, a `tools` name outside the configured set, a `budget` above the bound, a `workspace` outside the root, or a `task` over `task_bytes`. Nothing spawned. | nothing |
| `host` | The driver could not start the child: binary missing, workspace not a directory, spawn refused. Nothing ran. | nothing |
| `unavailable` | The CLI is not logged in: the probe of §6 failed, or the run's own events say so (`codex`'s `401 Unauthorized` loop, which the adapter ends by killing the child). The fix is a human logging in; the driver never tries. | nothing, or what the CLI reported before the driver stopped it |
| `throttled` | The CLI's terminal event names a rate limit or a quota window, under either billing cell. | what the CLI reported, else nothing |
| `provider` | The CLI's terminal event is any other error: provider 4xx/5xx, a model refusal the CLI surfaced as an error, an unknown `session`. `message` carries the CLI's text. | what the CLI reported |
| `envelope` | The run finished with a final message that was not an envelope even after the tolerant parse of §4. The `transcript` carries what the CLI actually said. | real |
| `lost` | The run started and its terminal event never arrived: the child crashed, `SIGTERM` reached it (`claude` prints nothing after one), or the last rung of §5 did. Something ran and the driver cannot say how much. | **the ceiling** |

`unavailable`, `throttled` and `provider` are reply kinds *until*
[#122](https://github.com/tau-rs/tau/issues/122) gives driver health a home in
the log; when it does, `unavailable` becomes a health entry and the reply
keeps the kind for the corr that hit it. Nothing here waits on that.

### 3. Two ops, no steering: `run` and `resume` are each one `send`

`run` starts a fresh CLI session in `workspace`. `resume` continues the
session a previous reply named, with `task` as the amendment, in the same
`workspace`. Each is one `send`: reserved before delivery, one reply, one
settlement, one blob in the log. #130 run 6 is the shape: `--resume <id>`
with the amendment as the first stdin message continued the session under
the same id, without the hook noise of a fresh start, and produced a second
envelope.

**There is no mid-task steering in v1**, and it is not an omission. Steering
would be a second message to a corr already in flight. The kernel's contract
is one `send`, one `Reply` (ADR-0006 §5's symmetry is what lets the tool loop
carry this driver without knowing it exists); a second channel to a running
child — a `send` to the same corr, or a driver-side stdin the harness pokes
— is a bypass channel invisible to the log, which HANDOFF §4 invariant 4
forbids outright. And the two CLIs disagree: `claude` has a stream-json
stdin in print mode, `codex exec` has nothing but `resume`. One op with two
behaviours is the drift a mapping table exists to prevent. The `claude`
stdin is used for exactly one thing after the task: the interrupt of §5,
which the kernel initiates through `abandon` and the log sees as a `cancel`.

What the handoffs wanted from steering, `resume` gives as a *visible* act:

| The caller wants to | v1 does it as |
|---|---|
| change course mid-task | `cancel` the child agent (its `abandon` stops the CLI, §5) — then `resume` with the amendment. Two log entries, one session. |
| add information after a `done` | `resume` with the new fact |
| continue after a `limit` | `resume` with "continue" — the session has the context, and the bound applies afresh |
| retry an `error.envelope` | `resume` with "reply with the JSON envelope only" |

`resume` after an `abandoned` run is ordinary for `claude`: the session is
persisted on its side and picked up by id. `resume` after `error.lost` may
or may not work; the CLI decides, and an unknown id is `error.provider`.

The session id lives in a reply blob and nowhere else in v1. A program that
wants to survive its own restart reads it from the log; a *supervisor* that
resumes a task across a driver restart, with a resume marker so "one task,
not two" holds in the log, is M4's log-level resume and out of scope here.

### 4. The envelope and the tolerant parser

The **worker contract** is driver-owned text, stamped with the wire version,
appended to the CLI's own system prompt where the CLI allows that and
prepended to the task where it does not (§7). It tells the session: you are
headless, never ask a question, record every assumption, stay inside the
workspace, and end with exactly one JSON object of this shape. #130 ran a
contract of this shape and both `claude` runs answered with the object as
the whole final message, no fences.

```json
{
  "status": "ok",
  "summary": "Up to three sentences.",
  "artifacts": [ { "path": "relative/to/workspace", "kind": "file" } ],
  "assumptions": [ "…" ],
  "events": [ "notable decisions" ],
  "error": null
}
```

| Field | Type | Meaning |
|---|---|---|
| `status` | `ok` \| `partial` \| `failed` \| `cancelled` | The session's own verdict on the task. `partial` is what the contract asks for when the session stops short on its own; `cancelled` is what a session writes if a `resume` tells it to wind down. Neither a CLI bound nor an interrupt produces an envelope at all (§2, #130). |
| `summary` | string | Up to three sentences. |
| `artifacts[]` | `{ path, kind: file \| patch \| report }` | What it produced. The contract asks for paths relative to the workspace; #130's session wrote absolute ones, so the parser accepts either and the driver rewrites nothing. |
| `assumptions[]` | strings | What it decided alone, because it could not ask. |
| `events[]` | strings | Decisions worth a line. |
| `error` | string or `null` | Why, when `status` is `failed`. |

The JSON Schema for it lives in `envelope.rs`, derived from the same type the
parser deserializes with (one source of truth, ADR-0006 §5), and is handed to
each CLI that can enforce a final-message shape (§7). For that use it is
*strict*: every field required, `additionalProperties: false`, `error`
nullable — the shape structured-output modes accept.

The **parser** is tolerant before it is strict, because a CLI whose contract
was ignored still ran a task somebody paid for:

1. Take the final assistant text the CLI reported: for `claude`, the
   `result` event's `result` field; for `codex`, the `--output-last-message`
   file first, the JSONL's last agent message second.
2. Strip a surrounding code fence if there is one.
3. Find the **last balanced `{…}`** in what remains, scanning for braces
   outside string literals.
4. `serde_json` it as the envelope, with `artifacts`, `assumptions`,
   `events` defaulting to empty and `error` to `null`; `status` and
   `summary` are required.
5. Only then `EnvelopeViolation { reason }`, which the driver reports as
   `error.envelope` with the transcript intact.

The parser is a pure function over bytes, so it is a
[`fuzz/`](../../fuzz) target on the model of `envelope` and `model-reply`
(#6 item 3): the CLI's stdout crosses a boundary, and a parser that panics on
a hostile final message faults the driver.

An `error.envelope` is **not** rewritten into an envelope with `status:
failed`. A synthesized envelope would look exactly like one the session
wrote, and the caller would not know the contract was broken; the kind says
so, and the transcript says what happened instead.

### 5. Cancel and the wall bound: one ladder, and an unreported run bills up

`cancel` freezes the subtree and calls `Driver::abandon(corr)` for every open
request it holds ([ADR-0002](0002-seven-syscalls.md), M1a). The driver keeps
the flight registry the sandbox and the model drivers keep — `corr → child`
in a `BTreeMap` behind its own lock, with `abandoned_early` for a delivery
queued but not yet polled — and climbs:

```mermaid
sequenceDiagram
    participant K as kernel
    participant D as AgentDriver
    participant C as CLI child (pgid A)
    K->>D: abandon(corr)
    D->>C: interrupt — claude: control_request{interrupt} on stdin; codex: SIGINT to -A
    Note over D: wait up to abandon_grace for the terminal event
    D->>C: SIGTERM to -A
    Note over D: wait up to abandon_grace for exit
    D->>C: SIGKILL to -A
    D-->>K: reply {stop: "abandoned"} or {error: lost}, Consumption per the table
```

The first rung is what #130 §5 observed: `claude` answers the in-band
interrupt with a `control_response`, a rejected `tool_result`, and a
`result` carrying real usage (`terminal_reason: aborted_tools`, exit 1);
`SIGINT` gives the same with `aborted_streaming` and exit 0. `SIGTERM` gives
nothing — exit 143, no `result` — which is why it is the second rung and
not the first, and why a run that needed it is billed as unknown.

| What happened | `stop` | `usage` | `Consumption` |
|---|---|---|---|
| abandoned before spawn | `abandoned` | zeros | nothing |
| interrupt → the CLI emitted its terminal event and exited | `abandoned` | reported | reported |
| interrupt → the CLI exited without a terminal event | `abandoned` | last usage seen in the transcript, if any | **the ceiling** |
| SIGTERM was needed → exited | `abandoned` | last seen, if any | **the ceiling** |
| SIGKILL was needed | `error.lost` | none | **the ceiling** |

Unknown consumption is billed *up*, never down (ADR-0009 §5): the
reservation was already taken, so nothing new leaves the agent, and a driver
that guessed low would be the one place where a cancel is cheaper than a
completion. The in-flight turn a CLI was inside when it was interrupted is
spent at the provider and reported by nobody; the ceiling is the honest
number.

The **wall bound** (`wall` in config, enforced as a duration on a channel,
never an `Instant`) climbs the same ladder with the same table, reporting
`{ "limit": "wall" }` in place of `abandoned`, and `error.lost` at the last
rung. A **driver drop** climbs it too: `AgentDriver`'s `Drop` signals every
open child, so a harness shutting down leaves no CLI behind.

The child is spawned in a **new process group** (`CommandExt::process_group`,
safe `std`), and every signal goes to the group: the CLIs spawn their own
subprocesses (shells, test runners, sub-agents) and a `kill(pid)` would
orphan them. A grandchild that calls `setpgid` escapes the group; that row
is in the cannot-do table and its home is the same as the sandbox's.

One consequence #130 made concrete: an interrupt does not undo work. Its
run 5a found `a.txt` on disk although the `tool_result` said the write was
rejected — the tool had landed before the interrupt did. An abandoned
task's workspace is whatever state the session left it in, and the caller
reads the transcript to learn which.

### 6. Accounting, the ceiling, and availability

**What the driver reports**, from the CLI's terminal usage event:

| Dimension | Reported as | When |
|---|---|---|
| `tokens` | every input class the CLI reported (uncached, cache creation, cache read) + output | always, when the CLI stated usage; the ceiling when it did not (the unknown rows of §5) |
| `cost_microusd` | the CLI's stated cost, in microdollars, rounded up | when the CLI states a cost — `claude` states `total_cost_usd` on every `result`, list-price, whatever the login; `codex` (*#128*) as observed. Otherwise **derived from `tokens` at the prices the harness configured**, if it configured any; otherwise not reported |

Not reported, on purpose: `calls` (the kernel adds one at `send`); `wall_ms`
(the reducer charges it on every `Tick` the requester waits through, and the
driver never reads a clock); `compute_ms` (the CLI's local CPU is not what is
being paid for).

Cache-read tokens count in `tokens`. They are the bulk of an agent task's
input (#130 run 1: 4 uncached, 23,851 cache creation, 20,270 cache read),
they are what the provider meters, and a `tokens` figure that dropped them
would make a long session look cheaper than a short one.

The cost rule is the one place the two billing cells touch the accounting,
and it does so without naming either. `claude` states a list-price cost
under a subscription login too; the driver reports what the CLI said and
attaches no opinion about whether a bill exists. A harness on a
subscription that would rather not budget in list-price dollars leaves
`cost_microusd` out of its grants and meters in `tokens`; one on an API key
gets the same figure and it is the bill. Both are stated in `describe()`'s
sentence.

**The ceiling** (ADR-0006 §7 style) follows from the configured *task
bound*:

| Dimension | Derivation | Example |
|---|---|---|
| `tokens` | `task_tokens` | 400,000 |
| `cost_microusd` | `task_cost_microusd` when set; else `task_tokens × max(input_price, output_price)` when prices are set; else absent | 2,000,000 (≈ $2) |

One honest consequence, and the difference from a model call: **an agent
task's ceiling is not enforced by the driver before the fact.** A model
driver refuses a prompt over its input bound and clamps `max_tokens`; an
agent task's cost is unknowable until the CLI stops. What bounds it is, in
order: the CLI's own bound flags where it has them (`claude
--max-budget-usd` and `--max-turns`, from the task bound or the request's
tightening); the worker contract's instruction to stop and report `partial`
when it judges the task too large; and the driver's `wall` bound, which
climbs the ladder and bills the ceiling if the CLI does not report. A CLI
that overshoots its flag, or has none (`codex` at 0.46.0), reports above the
ceiling; the kernel records the excess as overdraft on the agent, and the
supervision policy ([#18](https://github.com/tau-rs/tau/issues/18)) acts on
it. v1 does not pretend the fence is hard; it makes the overshoot visible.

An agent with no `tokens` grant cannot run an agent task, however many agent
capabilities its namespace holds: `NoGrant` before delivery, like any other
dimension. Granting `tokens` where sub-tasks should run is the harness's
obligation, as `compute_ms` is for the sandbox.

**Availability.** At construction the driver:

1. runs the binary with its version flag, with a short timeout; a missing
   binary, a timeout, or output that does not match `expect_version` (when
   the harness set one) is a `ConfigError` — loud, at registration, the
   ADR-0009 pattern for a limit the host cannot honour;
2. runs the CLI's own login-status command (§7) and keeps the verdict: the
   one mode string of §2, or unavailable with the CLI's text.

Construction never fails because a CLI is logged out: that is a runtime
state a human changes by logging in, not a configuration error. Every `send`
while the verdict is unavailable answers `error.unavailable` and **re-probes
once**, so a user who logs in mid-run is not stuck with a stale verdict and
a driver that is truly logged out costs one short subprocess per refused
`send`, never a retry loop. A run whose events name an auth failure — the
`codex` 401 loop, a `claude` `result` that says so — flips the verdict back
to unavailable. The driver never attempts a login. For `codex` the probe is
the only gate that works: an unsigned `codex exec` runs, retries, and never
reports "not authenticated" on its own (#130 §3).

### 7. Mapping: the wire, and each CLI at its pin

Every unmarked row below was run by #130 on `claude` 2.1.272 and `codex`
0.46.0, or by #128 on `codex` 0.157.1. A row marked *#127* or *#128* is one
the adapter lane pins from its own transcript, because #130 could not reach
it (no `codex` login on the spike machine; `--json-schema` and
`--max-budget-usd` exhaustion untested). This table is the ADR-0006 §6
obligation for this family; a CLI added later fills in a column before it
ships. The `codex` column is at **0.157.1**: the 0.46.0 pin was refused
every model for a ChatGPT login by the time #128 recorded (amendment
2026-09-26), and the transcript that shows it is kept under
`cassettes/cli/codex-0.46.0/`.

| Wire | `claude` 2.1.272 | `codex` 0.157.1 |
|---|---|---|
| binary, version pin | `claude --version` → `2.1.272 (Claude Code)` | `codex --version` → `codex-cli 0.157.1` |
| login probe (§6) | `claude auth status`: JSON on stdout, exit 0 when signed in; `mode` is not read from here (§2) | `codex login status`: exit 0 signed in, exit 1 otherwise (stderr `Error checking login status: …` at 0.46.0); the one line — `Logged in using ChatGPT` — is on **stderr**, stdout empty (#128 run 0), and is `mode` |
| fixed argv, every run | `-p --safe-mode --input-format stream-json --output-format stream-json --verbose --permission-prompts none --permission-mode <config>` | `-a never exec --json --skip-git-repo-check --ignore-user-config --output-schema <file>`, then `--sandbox <config> --cd <workspace>` on `run` and `-c sandbox_mode="<config>"` on `resume`, then `-m <config model>` and `-c model_reasoning_effort="<config>"`, then the prompt. Flags go **after** `exec`: a top-level `-m` parses and is silently ignored (#128 probed it); `-a` is the one flag `exec` rejects and the top level takes. |
| why `--safe-mode` | without it the user's hooks, plugins, skills and `CLAUDE.md` load; hook events precede `init` and plugin text reached the envelope (#130 runs 1, 7). `--bare` never reads OAuth, so it is not an option. | `--ignore-user-config`: `codex` reads `~/.codex/config.toml` — model, MCP servers, instructions — and this leaves it out while the login is read regardless (#128 run 7). Never `--ephemeral`: a `resume` needs the thread on disk. |
| `op: run` | the task is the **first stdin message**, `{"type":"user","message":{"role":"user","content":"<task>"}}`; stdin stays open for the interrupt and is closed when the terminal event arrives | the task is the positional prompt, worker contract prepended, `Task:` between them (#128 run 1). stdin is a pipe the driver never writes; a non-tty stdin makes the CLI print `Reading additional input from stdin...` on stderr and append nothing. |
| `op: resume` | `--resume <session>`, the amendment as the first stdin message (#130 run 6) | `exec resume <thread> --json --skip-git-repo-check --output-schema <file> -c sandbox_mode="…" "<amendment>"` — **at 0.46.0 `exec resume` had none of `--json`, `--output-schema`, `-o`**, so `resume` was `error.unsupported`; 0.157.1 lists them all (#128 run 4). `exec resume` still has **no `--sandbox` and no `--cd`**: without the `-c` override the recipe's resume ran read-only and the envelope said `failed` (run 4); a resumed thread works in the **child's `cwd`**, not the thread's original directory (run 5), so the driver spawns it in the workspace. |
| worker contract (§4) | `--append-system-prompt "<contract>"` — never `--system-prompt`, which strips the CLI's own scaffolding | no system-prompt flag in `exec`: prepended to the prompt on `run`; the amendment alone on `resume`, the thread has the contract |
| envelope enforcement | `--json-schema <schema>` is listed; *#127* runs it once and keeps it only if the final message is still the plain envelope text | `--output-schema <file>`, the strict schema written to a scratch file under the host's temporary directory, named by the corr, removed after the run. OpenAI's structured-output mode is strict all the way down: every nested object closed, `anyOf` never `oneOf` (#128 run 6; `envelope::schema()` does both). No `-o`: the final message is the last `agent_message` item, the same text. |
| `workspace` | child `cwd` | `--cd <dir>` on `run`, the child's `cwd` on both |
| `tools` | `--allowedTools <tools...>`, must ⊆ config | *no per-session allowlist*: a present `tools` is `error.unsupported`; the cage is `-s` in config |
| headless permissions (config) | `--permission-mode <acceptEdits \| auto \| bypassPermissions \| manual \| dontAsk \| plan>` and `--permission-prompts none` (anything that would prompt is denied) | `-a never` **before the subcommand** (`exec --ask-for-approval` is rejected; `--full-auto` is `-a on-failure`, which can still block) and `-s <read-only \| workspace-write \| danger-full-access>` |
| `budget.cost_microusd` | `--max-budget-usd <usd>`; exhaustion is `{ "limit": "cost" }` (*#127* pins the `result.subtype`) | *no flag*: `error.unsupported` |
| `budget.turns` | `--max-turns <n>` — **absent from `--help`, accepted and enforced** (#130 run 6: `subtype: error_max_turns`, `terminal_reason: max_turns`, `result: null`, exit 1) → `{ "limit": "turns" }`. A hidden flag is a drift-job row (§10). | *no flag*: `error.unsupported` |
| model (config) | `--model <model>` | `exec -m <model>` — after the subcommand, never before |
| effort (config) | `--effort <level>` | `exec -c model_reasoning_effort="<level>"`, a TOML string |
| `session` | `init.session_id`. Not the first line: with hooks, `hook_started`/`hook_response`/`session_state_changed` precede it; the adapter takes the first `init`. | `thread.started`'s `thread_id`, the first line; a `resume` prints the same id first |
| `mode` | `init.apiKeySource` (`"none"` on a claude.ai login) | the `codex login status` line, from stderr |
| `model` (reply) | `init.model` (`claude-opus-5[1m]`) | **`null`**: no event names the model at 0.157.1 (the default was `gpt-6-luna` at recording; the driver does not guess) |
| terminal event | `result`: `subtype`, `is_error`, `terminal_reason` (`completed`, `max_turns`, `aborted_tools`, `aborted_streaming`), `num_turns`, `total_cost_usd`, `usage{input_tokens, cache_creation_input_tokens, cache_read_input_tokens, output_tokens}`, `api_error_status`, `errors[]`, `result` (the final text) | `turn.completed{usage{input_tokens, cached_input_tokens, cache_write_input_tokens, output_tokens, reasoning_output_tokens}}`, or `turn.failed{error{message}}`; the final text is the last `item.completed` whose `item.type` is `agent_message`, in `item.text` |
| `usage` | `result.usage` summed per §6; `total_cost_usd × 10⁶` → `cost_microusd`; `num_turns` → `turns` | `input_tokens` and `output_tokens` **as stated**: `cached_input_tokens` and `reasoning_output_tokens` are subsets of them, not further classes, so nothing is summed. No cost stated, no turn count: both `null`. |
| `unavailable` at run time | a `result` whose `errors[]` or `api_error_status` names authentication — *#127* | an `error` or `turn.failed` whose `message` names `401 Unauthorized` (#130 §3) or a login — *#128*, read by name. The unsigned loop never ends on its own; the probe (§6) is the gate, and the driver flips the verdict when a run's events name it. |
| `throttled` | `result.api_error_status` 429, or a terminal `rate_limit_event` whose `status` is not `allowed` — *#127* pins the strings | a `turn.failed` naming 429, a rate limit, a quota or a usage limit — *#128*, read by name |
| `provider` | any other `result` with `is_error` | any other `turn.failed`: a rejected model (0.46.0 run 1), a rejected schema (run 6), a backend 4xx/5xx; five `error` retries precede it. And a refusal before any thread started (the row below) |
| unknown `session` on `resume` | *unrecorded* | **nothing on stdout, exit 1**, `Error: thread/resume: thread/resume failed: no rollout found for thread id <id> (code -32600)` on stderr (#223 run 9). No thread started, so it is `error.provider` with that text and **nothing is billed**: §5's ceiling is for a turn that may have been spent, and this run never reached the provider. The adapter reads it before `settle`: a run that printed nothing and exited non-zero. |
| interrupt (§5, first rung) | `{"type":"control_request","request_id":"<id>","request":{"subtype":"interrupt"}}` on stdin (`init.capabilities` lists `interrupt_receipt_v1`); `control_response` acknowledges | `SIGINT` to the group — `exec` has no in-band channel. **Nothing printed, exit 1, no terminal event** (#128 run 2): the first rung answers but reports nothing, so an abandoned `codex` run bills the ceiling. |
| after `SIGTERM` | nothing printed, exit 143, no `result` (#130 §5c) | nothing printed, **exit 0**, no terminal event (#128 run 3) |
| environment | exactly `config.env` — `HOME` so the CLI finds its own login (and `USER` on macOS, where the Keychain lookup keys on it: 2026-09-26 amendment), `PATH` if the binary path is not absolute, and any key the harness *chooses* to pass | same |
| stdin | the task, then the interrupt; closed when the terminal event lands | **none** (`/dev/null`). `exec` reads a piped stdin to end of file before it starts — *Reading additional input from stdin...* — and printed nothing for the five seconds #223 run 8 held the pipe, then `thread.started` 60 ms after the close. A pipe nobody writes is a stall to the wall bound, so the supervisor gives a child it will never speak to (no first message, a signal for the interrupt) no stdin at all. |
| transcript | one JSON value per stdout line; kinds seen: `system/{hook_started,hook_response,session_state_changed,init}`, `rate_limit_event`, `assistant`, `user`, `control_response`, `result` | one JSON value per stdout line (`--json`): `thread.started`, `turn.started`, `item.started`, `item.completed` (`item.type` `agent_message` or `command_execution`), `error`, `turn.completed`, `turn.failed` |

Both columns share one rule for the environment: **nothing is inherited**.
The child gets exactly what the harness listed at registration, as the
sandbox's does. `HOME` is in that list because the CLI's login state lives
under it; that is the CLI reading its own files, not the driver reading
them. A harness that wants the API-key cell puts the key in the list; one
that wants the subscription cell does not. The driver cannot tell which it
got and does not try.

### 8. The invariant: subprocess-only auth, with its test

The driver never reads a credential file, never queries the Keychain, never
calls a provider endpoint. It spawns a binary and reads its stdout. The
binary owns its login.

`drivers/tests/agent_guard.rs` holds the line: it walks every source file
under `drivers/src/agent/` and fails if any of them contains
`.credentials.json`, `auth.json`, `find-generic-password`, `Keychain`,
`api.anthropic.com`, `api.openai.com`, `chatgpt.com`, `reqwest`, or
`home_dir`. A planted-string test proves the scan catches each. The list is
in the test, not in config, and grows on the day a new way to cheat is
named. The `agent` feature does not enable `reqwest`; a
`cargo tree -e features` check in the same test file says so.

Two things the guard cannot see, and where they are handled instead. The
environment the harness passes is the harness's choice by design (§7), and
`describe()` names it. The probe's output is read by the driver — it has to
be, to learn the verdict — and §2's `mode` rule is what keeps the email and
organisation in `claude auth status` out of the log: one field, never the
document.

### 9. `describe()`: what a model that delegates sees

An agent driver *is* a tool: a model in the `libtau` loop may hand a
sub-task to it. The projected schema is derived from `Request` with
`schemars`, the type the driver deserializes with; `v` is skipped; each
branch closes itself with `additionalProperties: false`; and the codex
adapter omits `tools`, `budget` and the `resume` op because it would
refuse them.

```json
{
  "description": "Delegate a whole task to a Claude Code session in the shared workspace. It runs headless with tools Read, Edit and Bash, may spend up to $2 (400k tokens, 40 turns) per call, and answers with a JSON report: status, summary, artifacts, assumptions, events. Pass `session` from a previous report to continue it.",
  "input_schema": {
    "type": "object",
    "oneOf": [
      { "properties": { "op": { "const": "run" }, "task": { "type": "string" }, "workspace": { "type": "string" }, "tools": { "type": "array", "items": { "type": "string" } }, "budget": { "type": "object", "properties": { "cost_microusd": { "type": "integer" }, "turns": { "type": "integer" } } } }, "required": ["op", "task"], "additionalProperties": false },
      { "properties": { "op": { "const": "resume" }, "session": { "type": "string" }, "task": { "type": "string" }, "workspace": { "type": "string" } }, "required": ["op", "session", "task"], "additionalProperties": false }
    ]
  }
}
```

The sentence is composed at construction from the config: the CLI, the
tools, the bound, and whether cost is metered. The harness may replace the
sentence; it cannot remove the bound from it (ADR-0009 §7's rule).

### 10. Testing offline, and the drift job

- **The fake CLI** ([#132](https://github.com/tau-rs/tau/issues/132)):
  `tau-fake-cli`, a dev-only binary that replays a recorded stdout
  transcript line by line with optional delays, reacts to stdin lines and to
  `SIGINT`/`SIGTERM` as a script says (emit lines then exit after a delay,
  or ignore, to reach `SIGKILL`), and can withhold the terminal event to
  produce `lost`. Every test points `AgentConfig::binary` at it. #130's five
  cancel runs are its first scripts: the in-band interrupt's
  `control_response` + `result` + exit 1, `SIGINT`'s `result` + exit 0,
  `SIGTERM`'s silence + exit 143.
- **The transcripts** ([#130](https://github.com/tau-rs/tau/issues/130)):
  committed under `drivers/tests/cassettes/cli/<cli>-<version>/`, one file
  per recorded run, with the CLI version in a header line, scrubbed by the
  same guard `cassette_guard.rs` applies to provider cassettes — and with
  the `init` event's `plugins`, `slash_commands` and `skills` lists and the
  hook-response bodies cut, because they are one user's machine, not the
  CLI's surface. They are the fixtures the parser and the event mapping are
  tested against. #130's machine had no `codex` login, so #128 began by
  recording one on a machine that did: `codex-0.157.1/`, eight runs, and one
  run on the 0.46.0 pin that shows why the pin moved.
- **The fixtures**: the JSON in this ADR round-trips through the wire types
  byte-for-byte in value, under `drivers/tests/fixtures/agent/`, the
  ADR-0006 §8 rule.
- **One test per row** of the `stop` table, the `error.kind` table, and the
  ladder table of §5, each against the fake binary, none against a mock of
  the driver.
- **One `libtau` test** that projects `describe()`, runs one delegated task
  through the tool loop against the stub model, and reads the envelope back
  as a `tool_result`.
- **The drift job**: a scheduled Tier 2 leg that installs the *latest* CLI,
  runs the probe and one envelope round-trip on a trivial task, checks that
  every flag in the fixed argv is still accepted — the hidden `--max-turns`
  above all — and compares the flag surface to the pin. It runs in the
  API-key cell, because CI can hold a key and cannot hold a login. The
  subscription cell is verified on a laptop, by a human, when the pin moves
  — stated here so nobody mistakes the job's green for coverage of both.

Moving the pin is a PR that re-records the transcripts, updates the
`cli.version` fixtures, and corrects §7's table.

### 11. What v1 cannot do, and where each goes

| Not possible in v1 | Why | Home |
|---|---|---|
| a raw completion on a subscription | needs the CLI's OAuth token; banned by both vendors | nowhere — the empty cell, on purpose |
| steer a running task | one `send`, one reply; a second channel is a bypass invisible to the log | `resume` as a second `send` (§3); a steering op only if the bridge ever streams |
| resume across a driver restart from the log alone | the session id lives in a reply blob; nothing reads it back | M4 log-level resume, with #122's health entries |
| know the billing mode | the driver reports the one string the CLI stated and interprets nothing | the operator's eyes, and the docs |
| bound cost or turns before the fact at `codex` | no flag at 0.157.1 either | the wall bound and overdraft (#18); the flags, when the CLI grows them |
| an abandoned `codex` run at the reported usage | `SIGINT` ends the run with nothing printed (#128 run 2) | the ceiling; an in-band channel, if `exec` ever grows one |
| an envelope from a bound or an interrupt | the CLI ends the session before the model gets a turn (#130) | `resume` with "wind down and report" |
| undo work an interrupt arrived too late for | a tool that landed before the interrupt stays landed (#130 run 5a) | the caller reads the transcript; a workspace it can discard |
| configure MCP servers for the child | a second config surface to pin | additive config, v2 |
| `codex app-server` | a second protocol to pin | a second backend behind the same wire |
| confine the child to the workspace | the driver adds no fence; the permission mode and `-s` are the CLI's own | the CLI's cage, via config; or the sandbox shim at a later rung |
| guarantee every grandchild is dead | a process that calls `setpgid` leaves the group | pid namespace, `cgroup.kill` (ADR-0009 §6) |
| stream the transcript (`MsgKind::Partial`) | one reply per run | additive, with the model bridge's streaming |
| guarantee an envelope on `done` | a CLI may ignore its contract; the parser is tolerant, then `error.envelope` | `--json-schema` / `--output-schema` where the adapter verifies them; the caller's `resume` |
| run without `HOME` in the environment | the CLI finds its login under it | nowhere; the list is exactly what config says |
| report `compute_ms`, `wall_ms` | not what is paid; the reducer charges wall | nowhere |

### 12. Versioning

ADR-0009 §8's rules, restated for this wire:

- `v` is bumped for a change a v1 reader cannot parse: a new `op`, a new
  `stop` shape, a new required field, a changed meaning, a change to the
  envelope's required fields. An optional field with a defaulted
  deserialization is additive and does not bump `v`.
- The driver replies `error.unsupported` to a present `v` it does not
  implement. An absent `v` is the projected version, never a guess.
- The worker contract text is versioned with `v`: a CLI session sees the
  contract for the `v` it was asked in.
- A CLI pin is not a version: moving it changes §7's table, the transcripts,
  and the `cli.version` fixtures, not the bytes.

## Consequences

- **The kernel is untouched.** No syscall, no `abi/` change, no new
  `DimKey`, no `bridge` change. Agent drivers are drivers behind `send`.
- **Three implementation lanes, one contract.**
  [#131](https://github.com/tau-rs/tau/issues/131) builds the shared module
  from §1, §2, §4, §5, §6, §8, §9; then
  [#127](https://github.com/tau-rs/tau/issues/127) (`claude`) and
  [#128](https://github.com/tau-rs/tau/issues/128) (`codex`) fill their
  columns of §7 in parallel. [#132](https://github.com/tau-rs/tau/issues/132)
  (the fake CLI) runs beside #131 and depends on neither the wire nor the
  envelope. #131 and #132 flip to `status:ready` when this ADR merges; #128
  additionally needs a `codex` transcript recorded on a logged-in machine
  before its adapter can be tested offline.
- **#130 is the authority on flags.** A row of §7 that a later transcript
  contradicts is corrected in this ADR, as an amendment, before the adapter
  that needs it starts.
- **The harness has new obligations**: install the CLIs and log in as a
  human; list `HOME` (and, for the API-key cell, the key) in `config.env`;
  grant `tokens` where sub-tasks should run; decide whether `cost_microusd`
  is a budget line it wants, knowing `claude` reports a list-price figure
  under any login; and read `mode` in the log with its own eyes, because
  nothing else will.
- **Overdraft gains its second producer.** The sandbox's fork tree was the
  first; a CLI that overshoots its budget flag, or has none, is the second.
  [#18](https://github.com/tau-rs/tau/issues/18) has two concrete drivers.
- **The empty cell is documented, not just empty.** A future contributor who
  reaches for the OAuth token finds the grid in §Context and the guard test
  of §8, not a code review.
- **The obligations this creates:**
  - every `stop` variant, every `error.kind`, and every ladder row has a
    test that produces it against the fake CLI;
  - a new CLI fills in a column of §7 before it ships, and its transcripts
    join `drivers/tests/cassettes/cli/`;
  - a new request field is optional, defaulted, and in §2's table before it
    ships;
  - moving a pin re-records the transcripts and corrects §7 in the same PR;
  - the drift job checks the hidden `--max-turns` on every run, because a
    hidden flag is the one most likely to vanish without a changelog line.

## Alternatives considered

**The handoffs' shape: a driver spawned by the kernel and mapped onto all
seven syscalls.** Rejected. A driver has three methods and one `send`
reaches it; "`recv` yields typed events from the child's stdout" is
`Partial` streaming, which the bridge does not have, and "`send` steers the
running task" is a second channel to a corr in flight, which the log cannot
see. The transcript in one reply carries every event the handoffs wanted in
the log; `resume` carries the steering as a visible second `send`.

**One crate per driver** (`driver-claude-sub`, `driver-codex-sub`). Rejected
on ADR-0006's and ADR-0009's argument: a module is the same code with one
fewer `Cargo.toml`, and the two adapters share more than they own. The names
also put a billing mode in a crate name, which §Context forbids in a type
and which would be no better on a crate.

**Read the CLI's OAuth token and speak the provider API with it.** The empty
cell. Rejected: banned by both vendors, acted on by one, and it would make
tau the thing the sanctioned path exists to avoid. The guard test of §8 is
the tripwire.

**A `BillingMode` enum (`Subscription | ApiKey | Unknown`) on the reply or
the config.** Rejected. The policy behind the enum changed twice in 2026;
each change would have been a wire change. A verbatim string from the CLI is
stable by construction: it is whatever the CLI said, and interpreting it is
the operator's job.

**Mid-task steering by writing to the CLI's stdin.** Rejected as a bypass
channel (invariant 4) and as an asymmetry: one CLI has a stdin in print mode,
the other has nothing but `resume`. The handoff for `codex` already
implements its `send` as "cancel then resume with the amendment", which is
exactly `abandon` plus `resume` — so the ADR makes that the one shape, and
makes it visible as two log entries. The stdin is still used, for the one
message the kernel initiates: the interrupt.

**`SIGINT` as the first rung for `claude` too**, for symmetry with `codex`.
Rejected: #130 showed both paths yield a `result` with usage, but the
in-band one is acknowledged (`control_response`) and is the mechanism the
CLI advertises in `init.capabilities`; a signal is what the driver reaches
for when the advertised path is silent.

**A bound hit as a `partial` envelope.** The handoffs' acceptance criterion
6. Rejected by evidence: `--max-turns` ends the session with `result: null`
and the model never writes an envelope. A `limit` stop reason says what
happened; a synthesized `partial` would claim the session said something it
did not.

**Stream the transcript as `MsgKind::Partial`.** Rejected for v1: the bridge
has no streaming, and adding it for one driver family would put the first
`Partial` producer in a place the tool loop cannot consume. Additive when
the bridge streams.

**`codex app-server` as the v1 binding.** Deferred. It is the right
long-term shape — one process, many threads, bidirectional — and a second
protocol surface to pin at a version that just migrated `exec` onto it. A
second backend behind the same wire when the surface settles.

**Fail construction when the CLI is logged out.** Rejected: a login is a
runtime state a human changes, not a configuration error, and a harness
that boots on a fresh machine should say `error.unavailable` on the first
`send` rather than refuse to boot. A missing binary *is* a `ConfigError`.

**Skip the probe and let the run report "not authenticated".** Rejected by
evidence: unsigned `codex exec` starts a thread and retries 401s until
killed. The probe is the only cheap, deterministic gate.

**Per-request `model`, `effort`, `permission_mode`, absolute `workspace`,
arbitrary `tools`.** Rejected, each on the same rule: the requester may be a
model, and the model does not size its own cage (ADR-0009). Each is one
optional, defaulted field away on the day a program needs it.

**Bill an abandoned run at the last usage figures seen.** Rejected: the turn
the CLI was inside when interrupted is spent and reported by nobody, and
billing low there makes a cancel cheaper than a completion — the one
outcome the sandbox's `lost` rule exists to forbid.

**Count only uncached input tokens.** Rejected: #130's runs had four
uncached input tokens and forty-four thousand cached ones. The cached ones
are what the provider meters and what a subscription window counts.

**Rewrite an `EnvelopeViolation` into `{ "status": "failed" }`.** Rejected:
a synthesized envelope is indistinguishable from one the session wrote. The
error kind says the contract was broken; the transcript says what happened.

**Derive `cost_microusd` from tokens unconditionally, at a default price.**
Rejected: a subscription has no per-token price, and a made-up one would be
a billing-mode assumption wearing a number. Prices are configured or absent.

**Fold `tools` into the prompt at `codex` instead of refusing it.**
Rejected as silent degradation: an allowlist the CLI does not enforce is
advice, and ADR-0006's rule is that unsupported means refused. The codex
adapter's `describe()` omits the field, so a model never writes it.

**Carry the whole `claude auth status` document as `mode`.** Rejected: it
holds the user's email, organisation id and name, and a reply is a blob the
log keeps forever. One field.

**Types in `tau_kernel::bridge`, like the model bridge.** Rejected for the
sandbox's reason: no second lane needs the shape before it exists, `libtau`
renders bytes verbatim, and the kernel should carry no vocabulary it has no
neighbour for.

[`driver.rs`]: ../../kernel/src/driver.rs
[`Consumption`]: ../../kernel/src/abi/budget.rs

## Amendments

- **2026-09-16** — §2's "`unavailable` becomes a health entry" is resolved by
  [ADR-0014](0014-driver-supervision.md) §6: it stays a reply kind, because
  the kernel never reads a payload. A harness that wants a logged-out CLI
  to be *down* reads the driver's verdict by its own means and calls
  `retire_driver` or `replace_driver`; that is what writes the health
  entry. The ladder of §5 is how an agent driver *reports* `lost`; ADR-0014
  is what happens when it cannot report at all.
- **2026-09-20** — §1's `Cli` trait is **deferred, not dropped**, by
  [#131](https://github.com/tau-rs/tau/issues/131). The shared module ships
  the seam as two types instead: a `process::Invocation` going in (argv, the
  first stdin message, the interrupt, which line is terminal) and an
  `Outcome` coming out (session, model, mode, usage, final message). An
  adapter that forgets a field does not compile, which is the contract the
  trait was there to give. Three reasons the trait could not ship in that
  lane: a crate-private trait cannot bound a public generic; a trait with no
  implementors fails `-D warnings` as dead code; and nothing under `tests/`
  can implement one, so the cancel ladder of §5 — the code that decides what
  an abandoned run costs — would have shipped untested. #128 extracts the
  trait once two implementors exist to shape it; its methods return these
  same two types, so the extraction is a move with no behaviour change.
  §1's file layout is unchanged.
- **2026-09-26** — The `claude` column is implemented by
  [#127](https://github.com/tau-rs/tau/issues/127) as
  `tau_drivers::agent::claude` behind the feature `agent-claude`, on the
  seam the 2026-09-20 amendment describes: `claude::invocation` builds the
  `process::Invocation`, `claude::outcome` reads the `Outcome`, and
  `ClaudeDriver` is `impl Driver` over decode → flights → run → settle. Four
  rows of §7 moved, none of the wire:
  - **`--json-schema`** is *off*. #127 did not run it live, so the row's
    "keeps it only if the final message is still the plain envelope text"
    condition is unverified; the tolerant parser of §4 is the enforcement
    until the drift job (§10) or a later pin runs it once and records the
    `result` shape it produces. `--allowedTools` is passed as **one
    comma-separated argument**: the flag is variadic (`<tools...>`) and a
    space-separated list would swallow whatever flag followed it.
  - **`budget.cost_microusd` exhaustion** is read by name, not pinned: a
    `result` whose `subtype` or `terminal_reason` contains `budget` is
    `{ "limit": "cost" }`. #130 run 6 pinned only the turns row.
  - **`unavailable` at run time** is read tolerantly: `api_error_status`
    401 or 403, or an `errors[]` entry naming authentication, an OAuth
    token, or `/login`. The exit status and text of `claude auth status`
    when logged out were not observed either (#130's machine was signed
    in; so was #127's); the probe follows §6's general rule, exit ≠ 0 is
    unavailable. A CLI that answered exit 0 with `"loggedIn": false` would
    surface as `error.unavailable` from the run's own events, one run late.
  - **`throttled`** is `api_error_status` 429, an `errors[]` entry naming a
    rate limit or a quota, or any `rate_limit_event` in the transcript whose
    `status` is not `allowed`; #130 saw only `allowed`.

  The interrupt is exactly #130 §5a's `control_request` with
  `request_id: tau-cancel-1`. `mode` is `init.apiKeySource`, and the login
  probe's document contributes nothing to a reply (`mode_from_login` is
  `|_| None`). The worker contract dropped #130's "messages arriving on
  stdin are instructions from your parent" line, because v1 has no steering
  (§3) and the only thing written after the task is the interrupt. Every
  other row is as the table says, tested against the seven committed
  transcripts, with the rows that wait on a grace period in the `ci`
  profile (`agent_claude_ladder`). `tau-fake-cli` grew one directive,
  `touch`, a readiness marker written after its signal handlers are
  installed, so that those rows do not race the binary's start-up under
  load — the shape of [#182](https://github.com/tau-rs/tau/issues/182).
- **2026-09-26** — §9's projection closed the *root* with
  `additionalProperties: false`, beside the `oneOf`. Under draft 2020-12
  that keyword sees only the `properties` of its own schema object — it
  does not look into `oneOf` — and the root has none, so the schema refused
  every key, `op` and `task` included. No test caught it before
  [#193](https://github.com/tau-rs/tau/issues/193): the fixtures compared
  the projection to itself, and the driver decodes with serde, never with
  the schema. The first time the projection was *validated against*
  ([#200](https://github.com/tau-rs/tau/issues/200)) was the `libtau` loop,
  which checks a `tool_call` against the tool's schema before it sends. Each
  branch now closes itself, as `schemars` derives it; the root carries only
  `type: object` and the `oneOf`. The single-op flat projection is unchanged:
  its `additionalProperties` was always beside its `properties`.
- **2026-09-26, later** — [#194](https://github.com/tau-rs/tau/issues/194)
  ran the four rows the amendment above left tolerant, on `claude` 2.1.272
  on a logged-in laptop, and recorded them as runs 8–11 beside #130's
  seven. Each row of §7 now reads as follows; the adapter matches these
  strings and no others.
  - **envelope enforcement** — `--json-schema <envelope::schema()>` is
    **on**, in the fixed argv. The CLI adds a `StructuredOutput` tool to
    the session (it is not gated by `--allowedTools`), the session calls it
    with the envelope as the tool's input, and the `result` carries the
    envelope twice: as `structured_output`, an object, and as `result`, the
    same object serialised. The final message is therefore still the plain
    envelope text, which is the condition the row set, and the tolerant
    parser reads it unchanged. `stop_reason` is `tool_use` on such a run,
    `terminal_reason` `completed`, `num_turns` one more than without the
    flag (run 8).
  - **`budget.cost_microusd` exhaustion** — `subtype: error_max_budget_usd`,
    `terminal_reason: budget_exhausted`, `errors: ["Reached maximum budget
    ($0.001)"]`, `result: null`, `is_error: true`, exit 1, and
    `total_cost_usd` is **the cap itself** with every usage count zero
    (run 9): the CLI reports the bound it hit, not what the aborted call
    spent. `{ "limit": "cost" }`, billed at the cap.
  - **login probe** — `claude auth status` while logged out is exit 1 with
    the same JSON document on stdout and `"loggedIn": false` in it; stderr
    is empty; `--text` says `Not logged in. Run claude auth login to
    authenticate.` (run 10). §6's exit-status rule holds unchanged. One
    honest consequence: the unavailable message carries that document, so
    a logged-out reply names the CLI's config paths; it names no identity,
    because a logged-out document has none.
  - **`unavailable` at run time** — a logged-out `claude -p` prints an
    `init`, then an `assistant` event from `model: "<synthetic>"` carrying
    `error: "authentication_failed"` and the text `Not logged in · Please
    run /login`, then a `result` with **`subtype: success`**, `is_error:
    true`, `terminal_reason: api_error`, `api_error_status: null`, no
    `errors[]`, that text in `result`, zero usage and cost, exit 1 (run
    11). The adapter reads the assistant event's `error` field, the pinned
    text, or `api_error_status` 401/403; the word list is gone. `subtype`
    alone never decides a success: `is_error` does.
  - **`throttled`** — unreached, on purpose: #194 did not spend a
    five-hour window to see one. The row stays on the two structured
    fields the CLI has for it, `api_error_status` 429 and a
    `rate_limit_event` whose `status` is not `allowed`; the word list is
    gone, so an `errors[]` entry that merely mentions a rate limit is
    `provider` with the text, and the drift job (§10) pins the row the day
    a transcript reaches it. Every run so far, #130's and #194's, saw
    `rate_limit_event.status: allowed`.
  - **environment** — on macOS the `claude` login is a Keychain item whose
    lookup keys on `USER`: with `HOME` and `PATH` alone a logged-in laptop
    reads as logged out, at the probe and at the run. `USER` joins `HOME`
    in the list the harness passes (§7's environment row, and the
    obligations under Consequences), which is still the CLI reading its
    own store and not the driver reading it.
- **2026-09-26** — The `codex` column is implemented by
  [#128](https://github.com/tau-rs/tau/issues/128) as
  `tau_drivers::agent::codex` behind the feature `agent-codex`, on the same
  seam as `claude`: `codex::invocation` builds the `process::Invocation`,
  `codex::outcome` reads the `Outcome`, and `CodexDriver` is `impl Driver`
  over decode → flights → run → settle. **The pin moved to 0.157.1.** On the
  day of recording, 0.46.0 answered every model name with `400 … not
  supported when using Codex with a ChatGPT account`, five retries, then
  `turn.failed` — the version, not the account, was what the backend
  refused — so the transcripts are at the current release, installed from
  npm into a scratch prefix and sharing the machine's login. §7's `codex`
  column is rewritten from those transcripts; the rows that moved, and one
  fixture:
  - **`resume` is served**, not `error.unsupported`: `exec resume` grew
    `--json`, `--output-schema`, `-o` and `--skip-git-repo-check`. It has no
    `--sandbox` and no `--cd`, so the cage travels as `-c
    sandbox_mode="…"` and a resumed thread works in the child's `cwd` —
    which is where the driver spawns it. Caps at `codex` are `{ tools:
    false, budget: false, resume: true }`, and the `describe()` projection
    offers both ops. §11's `resume` row is gone.
  - **The login probe's line is on stderr**, stdout empty. `Probe` now
    hands `mode_from_login` both streams (`LoginOutput`); the `claude`
    adapter's `|_| None` is unchanged.
  - **A top-level `-m` is silently ignored** by `exec`; every flag goes
    after the subcommand except `-a`, which `exec` rejects and the top level
    takes. The isolation row is `--ignore-user-config` (run 7); never
    `--ephemeral`, because a `resume` needs the thread on disk.
  - **`SIGINT` reports nothing**: exit 1, no terminal event, so an
    abandoned `codex` run is the ceiling row of §5's table, not the
    reported one. `SIGTERM` is silence and exit 0.
  - **`usage` is not summed**: `cached_input_tokens` and
    `reasoning_output_tokens` are subsets of `input_tokens` and
    `output_tokens`, not further classes. No cost, no turn count.
  - **`model` is `null`**: no `--json` event names it.
  - **The envelope schema** (§4) was rejected twice by OpenAI's
    structured-output mode — a nested object left open, then `oneOf` — so
    `envelope::schema()` closes every object and spells enums `anyOf`. The
    parser is unchanged. The `--output-last-message` file is not used: the
    last `agent_message` item carries the same text.
  - **`unavailable` and `throttled` at run time** are read by name (*#128*):
    `401`, `unauthorized`, a login; `429`, a rate limit, a quota, a usage
    limit. Neither was recorded (the machine stayed signed in and under its
    limits), and the drift job (§10) pins them when it first sees one.

  §1's `Cli` trait extraction (amendment 2026-09-20) waits on both
  adapters being on `main`; until then `CONTRACT` is spelled once in each,
  the same text.
- **2026-09-26** — §1's `Cli` trait is **extracted**, by
  [#201](https://github.com/tau-rs/tau/issues/201), closing the 2026-09-20
  deferral. `tau_drivers::agent::AgentDriver<C: Cli>` is the one `impl
  Driver` — decode, the flight registry, the run thread and its one-shot
  channel, the read-back, the settlement — and `claude::ClaudeDriver` and
  `codex::CodexDriver` are type aliases over `claude::Claude` and
  `codex::Codex`, each a unit `impl Cli`. Every test under
  `drivers/tests/agent_*` passes unchanged, which is the proof the move
  changed no behaviour. The three objections, answered:
  - *A crate-private trait cannot bound a public generic.* The trait is
    `pub` and **sealed** (`Cli: sealed::Sealed`, the seal reachable only
    from inside the module), so it bounds the public driver and still has
    exactly the implementors this crate ships — and its method set can grow
    without a breaking change, which is how the two rows below get in.
  - *No implementors is dead code under `-D warnings`.* Two exist.
  - *Nothing under `tests/` can implement one.* Nothing needs to: the ladder
    tests drive the real `AgentDriver<Claude>` and `AgentDriver<Codex>` over
    `tau-fake-cli`.

  The shape, confirmed by the lane and differing from #201's sketch in two
  places, each forced by a test that stays as it is:
  - `CAPS`, `probe()`, `invocation(&self, &AgentConfig, &Accepted,
    &Self::Scratch) -> Invocation`, `outcome(&self, &Run, &Verdict) ->
    Outcome`, and two defaulted hooks. **No `prepare(&AgentConfig)`**: the
    adapter is `Default`, because `codex`'s schema file is a *per-run*
    scratch — `agent_codex` asserts it is gone after the run while the
    driver is alive — so it is the associated `Scratch` type, written by
    `scratch(&self, Corr) -> Result<Self::Scratch, RunError>` and dropped
    the moment the process ends. `claude`'s is `()`.
  - **`auth_failure(&self, &Run, &Outcome) -> Option<String>`**, defaulted
    to the outcome's `unavailable` stop (`claude`, #194 run 11), overridden
    by `codex` to scan its `error` events (#130 §3). What #201 called
    `codex`'s post-settle `unavailable` override is, on `main` since #195,
    a verdict flip and never a `stop` rewrite; the hook is that flip, and
    the driver does it once for both.
  - **`refused_before_start(&self, &Run) -> Option<RunError>`**, defaulted
    to `None`: the reply a run that exited without a terminal event should
    get instead of `error.lost` at the ceiling, when the CLI's exit makes
    the reason plain. Nothing overrides it yet; it is where #223's
    unknown-thread `resume` row lands.

  Two smaller moves. `CONTRACT` is spelled once, in `agent`, and re-exported
  by each adapter. §1's file layout gains `driver.rs`, holding the trait and
  the driver, gated on either adapter's feature because the run thread's
  one-shot channel needs `tokio` and the shared half does not; `agent`
  re-exports both, so §1's `mod.rs` line reads as written. §1's "crate-
  private seam, not a public trait" now reads "public and sealed", which
  keeps what that sentence was for: nothing in `libtau` or the kernel learns
  either adapter, and no third implementor exists.
- **2026-09-26, later** — [#223](https://github.com/tau-rs/tau/issues/223)
  recorded the two rows [#196](https://github.com/tau-rs/tau/pull/196) had
  at 0.154.0 and #195 had no transcript for, on 0.157.1 with the driver's
  fixed argv (runs 8 and 9). Both are the CLI saying nothing on stdout,
  and both moved code, not just the table:
  - **stdin** — `exec` reads a piped stdin to end of file before it starts.
    The driver never writes this CLI's stdin, so the pipe the supervisor
    held open for every child was a stall to the wall bound: a `codex`
    run through the driver as merged by #195 would have been
    `{ "limit": "wall" }` at the ceiling, every time, and no test saw it
    because `tau-fake-cli` does not wait on stdin. The supervisor now gives
    a child it will never speak to — no first message, a signal for the
    interrupt — `/dev/null` (§7's new stdin row); `claude`, which takes its
    task on stdin, is unchanged. The `codex` test stub reads its stdin to
    end of file the way the CLI does, so the stall cannot come back
    unseen, and the fake grew `{"on": {"eof": true}, "ignore": true}` for
    the scripts that must outlive a closed stdin to be signalled.
  - **unknown `session`** — `exec resume` of an id with no rollout prints
    nothing, exits 1, and puts the reason on stderr. §2's `session` row
    said `error.provider` with the CLI's text; the adapter read `provider`
    only from a `turn.failed`, so the reply was `error.lost` at the
    ceiling — a run that never started, billed as one that vanished.
    `Codex` now overrides the `Cli::refused_before_start` hook the
    amendment above left for it: a run that ended by itself having printed
    nothing, with a non-zero exit, is `error.provider` with the exit code
    and stderr in the message, nothing billed. §5's ceiling is for a turn
    the provider may have spent; a CLI that printed nothing started none.
    The kind is still read by name for a login or a limit in that text, so
    a pre-start refusal that names one lands on its own row.
