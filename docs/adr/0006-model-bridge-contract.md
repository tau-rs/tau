# ADR-0006: The model bridge contract — request, reply, tool calls, `describe()`

**Status:** Accepted
**Date:** 2026-09-14
**Deciders:** tau core

## Context

M1c is "model driver (real), tool loop in libc" (HANDOFF §6). It splits into
two lanes that can run in parallel — the driver crate and the `libtau` tool
loop — *only* if they agree on the bytes first. Nothing in the kernel decides
those bytes: the kernel routes by capability and accounts by driver-reported
consumption, and never parses a payload ([ADR-0003](0003-substrate.md),
invariant 2). Left alone, each lane would invent half a contract.

Four constraints shape the answer:

- **No new crate.** Both lanes depend on `tau-kernel`; the shared vocabulary
  has to live there, and it has to live *outside* `kernel/src/abi/`, because
  it is not the envelope and must not be frozen by the ABI's gates.
- **The kernel must not learn to parse payloads by the back door.** A
  `describe()` on the `Driver` trait that returns opaque bytes is fine; a
  kernel-side tool registry that understands them is not.
- **Provider-neutral.** Anthropic first; an OpenAI-compatible endpoint (vLLM
  and friends) must fit without a second contract.
- **Errors ride in the reply.** `Driver::handle` returns `(Vec<u8>,
  Consumption)` and has no error channel until the M3 error envelope exists,
  so a driver's own failure — over the ceiling, an unsupported field, a
  provider 4xx — is a reply payload, not an exception.

The bridge is the pipeline HANDOFF §3.2 already names: namespace → projected
tool schema (per-driver `describe()`) → provider-side constrained decoding →
serde validation → name→capability resolution → `send`. Every failure in that
pipeline is fed back to the model as a tool result, and the model self-corrects.

## Decision

### 1. The types live in `tau_kernel::bridge`, and the kernel never imports them

`kernel/src/bridge/` holds the shared types. It is a vocabulary the kernel
*carries* for its neighbours, not one it *uses*: `kernel.rs`, `reducer.rs`,
`log.rs`, `syscall.rs`, `driver.rs`, and `blob.rs` never name the module, and a test
(`kernel/tests/bridge_isolation.rs`) asserts that they do not. The rule is
what keeps invariant 2 true after this ADR; the test is what keeps it true
after the next one.

The bridge is **not ABI**. It carries its own version, `v`, on every request
and reply (`bridge::VERSION`, `1` today), and it is governed by this ADR
rather than by `ABI` and the wire snapshots. The kernel's log stores a blob
hash either way; a bridge bump changes what the blob store holds, never what
the log says.

### 2. The request: one JSON object per `send` to a model capability

```json
{
  "v": 1,
  "system": "You are a research assistant with a key-value store.",
  "messages": [
    {
      "role": "user",
      "content": [{ "type": "text", "text": "What is stored under key a?" }]
    },
    {
      "role": "assistant",
      "content": [
        { "type": "text", "text": "Let me look." },
        { "type": "tool_call", "id": "call_1", "name": "store", "input": { "op": "read", "key": "a" } }
      ]
    },
    {
      "role": "user",
      "content": [
        { "type": "tool_result", "call_id": "call_1", "content": "hello", "is_error": false }
      ]
    }
  ],
  "tools": [
    {
      "name": "store",
      "description": "Read or write the run's key-value store.",
      "input_schema": {
        "type": "object",
        "oneOf": [
          { "properties": { "op": { "const": "read" }, "key": { "type": "string" } }, "required": ["op", "key"] },
          { "properties": { "op": { "const": "write" }, "key": { "type": "string" }, "value": { "type": "string" } }, "required": ["op", "key", "value"] }
        ]
      }
    }
  ],
  "max_tokens": 1024,
  "sampling": { "temperature": 0.0, "seed": 42 }
}
```

| Field | Type | Meaning |
|---|---|---|
| `v` | `u16` | Bridge version. A driver that does not know it replies `error.unsupported`. |
| `system` | string, optional | The system prompt. Top-level, because Anthropic has it top-level and an OpenAI-compatible driver can fold it into a `system` message; the reverse mapping is lossy. |
| `messages` | array | Alternating `user` / `assistant` turns. Only those two roles: tool results are `user` content, as Anthropic frames them; an OpenAI-compatible driver unfolds them into `tool` role messages. |
| `messages[].content[]` | `text` \| `tool_call` \| `tool_result` | The three block types. See §4 for the tool blocks. |
| `tools` | array, may be empty | Projected tool definitions: `name` (a validated `Name`, see §5), `description`, `input_schema` (JSON Schema, draft 2020-12). |
| `max_tokens` | `u32` | The output cap for *this* call. The driver clamps or refuses above its registered maximum (§7). |
| `sampling` | object, optional | `temperature`, `top_p`, `seed`, `stop_sequences`. Each optional; absent means the provider's default. |

**A driver refuses what it cannot honour.** A present sampling field the
provider rejects (`temperature` on a model that has removed it, `seed` on a
provider without one) is `error.unsupported`, never silently dropped. HANDOFF
§4.10 makes controlled branches depend on pinned sampling, and a `seed` that
was quietly ignored is a branch that was quietly not controlled.

**Which model answers is the driver's configuration, not the request's.** A
capability is one endpoint at one model; the harness chose it at registration.
The reply says which model actually answered, for the record.

**Not in v1**: thinking and effort controls (driver configuration), images
and documents, streaming via `MsgKind::Partial`, server-side tools. Each is
additive when it comes.

### 3. The reply: one JSON object per `Reply`

```json
{
  "v": 1,
  "model": "claude-opus-5",
  "content": [
    { "type": "text", "text": "I will search for that." },
    { "type": "tool_call", "id": "call_2", "name": "search", "input": { "query": "tau kernel" } }
  ],
  "stop": "tool_call",
  "usage": { "input_tokens": 120, "output_tokens": 34 }
}
```

`stop` is one of `"end_turn"`, `"tool_call"`, `"max_tokens"`,
`"stop_sequence"`, `"refusal"`, or an error object:

```json
{
  "v": 1,
  "model": "claude-opus-5",
  "content": [],
  "stop": { "error": { "kind": "over_ceiling", "message": "input estimated at 9210 tokens, bound is 8000" } },
  "usage": { "input_tokens": 0, "output_tokens": 0 }
}
```

| `error.kind` | When |
|---|---|
| `over_ceiling` | The driver refused to make the call: the input estimate or `max_tokens` would exceed what the harness registered (§7). Nothing was billed. |
| `unsupported` | A `v`, a sampling field, or a block type the driver cannot honour. Nothing was billed. |
| `provider` | The provider answered with an error (4xx/5xx, or a 200 the driver could not map). `message` carries the provider's text. |
| `transport` | The request never got an answer: connection, timeout, abandoned by `cancel`. |

`usage` is what the model consumed, for the agent's information. It is *not*
the accounting: that is the `Consumption` the driver attaches to the same
reply, which the kernel settles against the reservation. The two come from the
same numbers, and a driver reports them honestly even on an error reply — a
provider error after a partial stream is still billed, and the report says so.

`refusal` is a model-side decline (Anthropic returns it with HTTP 200). It is
a stop reason, not an error: the call happened, the usage is real, and the
loop's response to it is a policy question for `libtau`, not a retry.

### 4. Tool calls, tool results, and how failure is fed back

A `tool_call` block carries the provider's call id, the tool `name` as the
model wrote it, and the parsed `input`. The name is a plain string here, not a
`Name`, on purpose: a model can write `serch`, and the loop must be able to
represent that call in order to answer it.

A `tool_result` block answers one call by id. `content` is text the model
reads. `is_error` marks a failed call. `error_kind`, present only with
`is_error`, is a typed reason that provider drivers drop (no provider has a
slot for it) but that `libtau`'s tests, logs, and metrics can assert on:

```json
[
  { "type": "tool_result", "call_id": "call_2", "is_error": true, "error_kind": "unknown_tool",
    "content": "unknown tool `serch`; available: search, store" },
  { "type": "tool_result", "call_id": "call_3", "is_error": true, "error_kind": "bad_args",
    "content": "bad args for `search`: missing field `query`" },
  { "type": "tool_result", "call_id": "call_4", "is_error": true, "error_kind": "denied",
    "content": "denied: this agent does not hold `search`" },
  { "type": "tool_result", "call_id": "call_5", "is_error": true, "error_kind": "failed",
    "content": "search: upstream timeout" }
]
```

| `error_kind` | Produced when | Who produces it |
|---|---|---|
| `unknown_tool` | The name resolves to nothing in the projected table. | `libtau`, before any `send`. |
| `bad_args` | `input` fails serde validation against the driver's schema. | `libtau`, before any `send`. |
| `denied` | The `send` was refused on authority or policy: `NotHeld`, or a hook `Deny` once M2 lands. | `libtau`, from the kernel's refusal. |
| `failed` | The `send` happened and the driver's reply says it failed, or the loop could not render the reply. | `libtau`, from the driver's reply. |

**Budget refusals are not tool results.** A `send` refused because the
ceiling cannot be reserved (M1b) is terminal for the loop: feeding it back
would only make the model try again with the same empty purse.

**Parallel calls.** A reply may carry several `tool_call` blocks. The loop
answers all of them in *one* `user` turn, in call order, one `tool_result`
each. A missing result is a malformed request at the provider.

### 5. `describe()`: one capability, one driver, one tool, named by the operator

```rust
// kernel/src/driver.rs
/// What a driver offers to a model, for schema projection. Opaque to the kernel.
pub struct ToolSchema {
    /// One or two sentences the model reads to decide when to call this tool.
    pub description: String,
    /// A JSON Schema (draft 2020-12) for the tool's input, as bytes.
    pub input_schema: Vec<u8>,
}

pub trait Driver {
    /// The tool this driver is, if it is one. Default: `None` — not a tool.
    fn describe(&self) -> Option<ToolSchema> { None }
    // handle, abandon as before
}

// kernel/src/syscall.rs — a read, not a syscall, like `Handle::read`
impl Handle {
    pub fn describe(&self, cap: Capability)
        -> Result<Option<(DriverId, ToolSchema)>, KernelError>;
}
```

**The tool's name is the `DriverId`.** The driver does not name its tool; the
harness named the driver at registration, the way a filesystem is named by its
mount point ([ADR-0005](0005-kernel-allocated-ids.md)). A `DriverId` is a
validated `Name` — `[a-z][a-z0-9_-]{0,62}` — which is a strict subset of both
Anthropic's and OpenAI's tool-name grammar, so no provider driver ever has to
rename. Two instances of one driver type (`web` and `intranet`, both a search
driver) cannot collide, and the name in a model transcript is the name in the
log's `DriverRegistered` entry, with no table in between.

**A driver is at most one tool.** A driver with several operations puts the
discriminator in its own schema (`{"op": "read", ...}` above), which
`schemars` derives from a tagged enum and constrained decoding handles. The
invocation payload `libtau` sends is therefore *exactly* the `tool_call.input`
bytes — no wrapper — and the driver's reply bytes are *exactly* the
`tool_result.content`, rendered as UTF-8 (lossily, if the driver replied with
something else). That symmetry is what lets `EchoDriver` stand in as a tool
in `libtau`'s tests without knowing this ADR exists.

**`Handle::describe` is a read, not an eighth syscall.** It has no effect: it
checks that the agent holds the capability, resolves it to the driver, and
forwards the driver's bytes without reading them. Nothing is logged, for the
same reason `Handle::read` logs nothing — the projected tool list ends up
inside a request payload the log already references by hash, so a refold that
never re-runs `describe` loses nothing. It fails with `NotHeld` for a
capability outside the agent's namespace: authority stays in the kernel, and
a child projects its own namespace, never a copy of its parent's table.

The schema is a single source of truth per driver: `#[derive(Deserialize,
JsonSchema)]` on the input type, serialized once in `describe()`, validated
with the same type on the way in. `schemars` is a driver-side dependency;
this ADR adds none to the kernel.

### 6. Provider mapping

| Bridge | Anthropic Messages | OpenAI-compatible chat |
|---|---|---|
| `system` | top-level `system` | first message, role `system` |
| `messages[].role` | `user` / `assistant` | same |
| `text` block | `text` block | `content` string |
| `tool_call` block | `tool_use` (`id`, `name`, `input`) | `tool_calls[]` (`id`, `function.name`, `function.arguments` as a JSON *string*) |
| `tool_result` block | `tool_result` (`tool_use_id`, `content`, `is_error`) in a `user` turn | one message per result, role `tool`, `tool_call_id`; `is_error` folded into the text |
| `tools[]` | `tools[]` (`name`, `description`, `input_schema`) | `tools[]` (`type: function`, `function.{name,description,parameters}`) |
| `max_tokens` | `max_tokens` | `max_tokens` / `max_completion_tokens` |
| `sampling.seed` | *unsupported* → `error.unsupported` | `seed` |
| `sampling.stop_sequences` | `stop_sequences` | `stop` |
| `stop` | `end_turn`, `tool_use`→`tool_call`, `max_tokens`, `stop_sequence`, `refusal` | `finish_reason`: `stop`→`end_turn`, `tool_calls`→`tool_call`, `length`→`max_tokens`, `content_filter`→`refusal` |
| `usage` | `usage.input_tokens`, `usage.output_tokens` | `usage.prompt_tokens`, `usage.completion_tokens` |

Anthropic's `pause_turn` only arises with server-side tools, which v1 does not
declare; a driver that sees one anyway reports `error.provider`.

### 7. Deriving a model driver's ceiling

M1b registers every driver with a ceiling: the most one request may cost,
reserved from the sender before delivery and settled against the report. For
a model driver the harness derives it from two numbers it configures and the
driver enforces:

- an **input bound**, in tokens — the largest prompt the driver will send;
- a **maximum `max_tokens`** — the largest output cap the driver will set.

The ceiling is then `tokens` = input bound + maximum `max_tokens`, and
`cost_microusd` = input bound × input price + maximum `max_tokens` × output
price, with prices in microdollars per token and every product rounded up.
`calls` is not part of the ceiling; the kernel adds one itself.

Worked example at Claude Opus 5 list prices ($5 per million input tokens,
$25 per million output, i.e. 5 and 25 µUSD per token), with an input bound of
8,000 tokens and a maximum `max_tokens` of 1,024:

| Dimension | Derivation | Ceiling |
|---|---|---|
| `tokens` | 8,000 + 1,024 | 9,024 |
| `cost_microusd` | 8,000 × 5 + 1,024 × 25 = 40,000 + 25,600 | 65,600 (≈ $0.066) |

The driver keeps the ceiling honest from its side: it **refuses** a request
whose estimated input exceeds the bound (`error.over_ceiling`, nothing sent),
and it **clamps** a request's `max_tokens` to its maximum (or refuses, if the
harness prefers loud). Estimation is the one soft spot — a byte count is not a
token count — so the driver either calls the provider's token-counting
endpoint, which costs a request but no tokens, or estimates with a margin
(bytes ÷ 3, then × 1.2 is conservative for English and JSON) and lets the
margin absorb tokenizer drift. Price at the *uncached* rate: prompt-cache hits
only ever come in under, and a ceiling that assumed a hit would overdraft on
the first miss.

Overdraft, when it happens, is visible on the agent and is the supervision
policy's business (#18), not the loop's.

### 8. Versioning

- `v` is bumped for a change a v1 reader cannot parse: a new block type, a
  new stop reason, a new required field, a changed meaning.
- An optional field with a defaulted deserialization is additive and does not
  bump `v`. Readers ignore fields they do not know.
- A driver replies `error.unsupported` to a `v` it does not implement; it
  never guesses.
- The Rust types are versioned with the crate, like the selectors
  ([ADR-0002](0002-seven-syscalls.md), "Selectors are Rust API"). The JSON
  examples in this ADR are the fixtures `kernel/tests/fixtures/bridge/*.json`
  round-trip through the types byte-for-byte in value; if the examples and
  the types disagree, the test says so.

## Consequences

- **#21 and #22 can run in parallel.** The driver implements §2, §3, §5, §6,
  §7; the loop implements §2, §4, §5. Neither touches `kernel/src/`.
- **The kernel gained a trait method and a read, not a registry.** `describe`
  forwards bytes; `Handle::describe` checks authority and forwards bytes. The
  isolation test is the tripwire against the day someone finds it convenient
  to parse them in the kernel.
- **Naming is the operator's, end to end.** The name a model calls is the
  name in the log, because both are the `DriverId`. A rename is a config
  change in the harness and a new registration entry in the log.
- **Errors are ordinary replies until M3.** A driver that fails always
  answers; "no answer" is reserved for the kernel's own error envelope when
  supervision arrives. The loop therefore has exactly one reply shape to
  parse.
- **Silent degradation is forbidden by the contract, not by review.**
  Unsupported means refused. A loop that wants best-effort sampling drops the
  field itself, visibly.
- **The obligation this creates**: a new provider driver must fill in a
  column of the §6 table before it ships. A mapping that lives only in code
  is the drift this ADR exists to prevent.

## Alternatives considered

**Driver-chosen tool names, several tools per driver** (the issue's sketch:
`describe() -> Vec<ToolDescriptor { name, description, schema }>`). Rejected.
It needs a collision rule in `libtau` for two instances of one driver, a
rename table between the model transcript and the log, and a wrapper
invocation payload (`{"op": ..., "input": ...}`) so the driver knows which of
its tools was called — which breaks the echo driver as a test tool. A driver
that wants several top-level tools can put them in one tagged schema today;
if a real need for several appears, a second, additive trait method is the
path, not a v1 wrapper.

**The harness projects at boot and passes the table down by closure.** No
kernel change. Rejected: every `spawn` with a narrower namespace needs a
hand-filtered copy of the table, and nothing checks the copy against the
namespace. Authority would have a second home in userspace memory, which
HANDOFF §4.2 forbids.

**`describe` as a `send` with a reserved payload.** Passes the irreducibility
test outright. Rejected: each description reserves a full model-call ceiling
plus one `calls`, so projecting six tools on a small budget is refused, and
every driver would have to parse a magic request, making the kernel's opacity
a fiction one layer down.

**Put the types under `kernel/src/abi/`.** Rejected: they are not the
envelope. Freezing them by wire snapshot would make every provider-driven
field addition an ABI event, and the ABI's gates would train contributors to
label reflexively — the failure ADR-0004 warns about.

**A `tau-bridge` crate.** Rejected as sequencing: the issue rules out a new
crate, and a module is the same code with one fewer `Cargo.toml`. If the
bridge grows a dependency the kernel should not carry (`schemars`, say), a
crate split is mechanical then.

**Errors as a separate reply shape** (`{"ok": {...}}` / `{"error": {...}}`).
Rejected: a driver that fails after a partial stream has both a usage report
and an error, and one shape with `stop.error` carries both without a rule
about which wins.

[`Msg`]: ../../kernel/src/abi/msg.rs
[`Consumption`]: ../../kernel/src/abi/budget.rs
