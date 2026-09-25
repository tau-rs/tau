# ADR-0007: Thinking blocks in the model bridge — bridge v2

**Status:** Accepted
**Date:** 2026-09-14
**Deciders:** tau core
**Amends:** [ADR-0006](0006-model-bridge-contract.md) §2, §3, §6, §8
**Amended:** 2026-09-25 (§2, the OpenAI-compatible column; see Amendments)

## Context

ADR-0006 §2 listed thinking as "not in v1 (driver configuration)". That was
half right. *Whether* a model thinks is driver configuration; *what it
thought* comes back in the reply, and the provider requires it back on the
next call.

The provider facts, verified against the Anthropic documentation on the date
above:

- Current models (Claude Opus 5, Sonnet 5, Fable 5 and 5.1) think by default.
  A reply carries `thinking` blocks — `{"type": "thinking", "thinking": "...",
  "signature": "..."}`, where the text is empty under the default display
  setting — and may carry `redacted_thinking` blocks, `{"type":
  "redacted_thinking", "data": "..."}`.
- "Within the latest assistant message, the sequence of consecutive thinking
  blocks must match what the model generated in the original request: you
  can't rearrange, edit, or partially drop them. This includes
  `redacted_thinking` blocks. Modified thinking blocks are rejected with a
  400 error."
- "A thinking block is readable only by the model that produced it or a newer
  one, and the API ignores or drops the blocks the target model can't read",
  without an error and without billing them.
- From Claude Fable 5.1 on, a block's signature also binds the prefix that
  produced it — the system prompt, the tools, and every earlier message — so
  a transcript must be append-only to keep its thinking valid.
- `thinking: {"type": "disabled"}` is accepted on Sonnet 5, Opus 4.7 and
  4.8, and Opus 5 at effort `high` or below; Fable and Mythos reject it with
  a 400.

The #21 driver drops `thinking` and `redacted_thinking` on the way out
because the bridge has nowhere to put them. The second call of a #22 tool
loop over a thinking-on model therefore sends an assistant turn missing its
thinking, which the provider may reject. A tool loop on a current model is
broken after its first step ([#42](https://github.com/tau-rs/tau/issues/42)).

## Decision

### 1. A fourth content block: `thinking`, opaque, provider-tagged

```json
{ "type": "thinking", "provider": "anthropic",
  "data": { "type": "thinking", "thinking": "", "signature": "EqQBCkYIBxgCKkBtX3Rob3VnaHRfc2ln" } }
```

```json
{ "type": "thinking", "provider": "anthropic",
  "data": { "type": "redacted_thinking", "data": "EmwKAhgBEgxyZWRhY3RlZC1ibG9i" } }
```

| Field | Type | Meaning |
|---|---|---|
| `provider` | string | The wire format `data` is written in, named by the driver that produced it. `anthropic` for the Messages API. Not a model: the provider decides per model what it can read. |
| `data` | JSON value | The provider's block, verbatim. Opaque to everything but a driver of the same `provider`. |

The block appears wherever the reply put it: in the reply's `content`, and
then in the assistant turn the loop appends. `libtau` carries it without
reading it. The Rust type is `Content::Thinking { provider, data }`, and the
enum stays exhaustive: a loop that matches on it is told by the compiler.

### 2. What a driver does with it

| Bridge | Anthropic Messages | OpenAI-compatible chat |
|---|---|---|
| `thinking` block, own `provider` | reply → request: unwrap `data`, send it verbatim, in place. Request ← reply: wrap every `thinking` and `redacted_thinking` block in order. | Request ← reply: the message's in-band reasoning — `reasoning` (Ollama) or `reasoning_content` (vLLM), either key, verbatim — becomes one block, `provider: openai-compatible`, first in `content`. Reply → request: **dropped**. Read-only: no chat-completions server takes it back. |
| `thinking` block, foreign `provider` | **dropped**, silently | dropped |

A driver replays its own provider's blocks *exactly as received*, in the
position they had, whatever model they came from: the provider decides what
its current model can read, the way it does for its own SDK users, and a
block it cannot read costs nothing.

The OpenAI-compatible column is the read-only half of that. The chat wire
has no thinking format: a server that reasons in-band returns the text on
the reply's message, under a key of its own, and nothing asks for it on the
next turn. The driver seals what it got so the program can display and log
what it paid for, and drops it on the way out like any foreign block — the
same bytes on the wire as before, so no recorded exchange moves. OpenAI's
own reasoning models expose reasoning only on the Responses API, which is
a different wire and out of scope here.

**The drop is a deliberate exception** to ADR-0006's rule that unsupported
means refused. A foreign provider's reasoning is unreadable by definition;
the provider itself drops what its models cannot read; and refusing would
make a transcript unmovable between providers. Nothing the model could have
used is lost. The exception is this narrow: a block of the driver's *own*
provider is never dropped, and a block type the driver does not know at all
is still `error.provider` (reply) or `error.unsupported` (request).

### 3. What the loop does

Nothing new. It already appends the reply's content to the transcript intact
and never filters by block type; the only change is the exhaustive match
gaining an arm that ignores the block. Two properties the loop already had
now matter: it is append-only, and it projects the same tool list on every
round, which is what the prefix-binding check requires.

### 4. `v` becomes 2

`Content` is exhaustive on purpose, so a v1 reader cannot parse the new
block, and ADR-0006 §8 says that is a bump. `bridge::VERSION` is `2`, every
fixture and every example in ADR-0006 says `"v": 2`, and a driver or a loop
that receives a `1` refuses it as it would any other number. Nothing shipped
speaks v1; the cost is mechanical.

### 5. Thinking off is driver configuration, and stays that way

`AnthropicConfig` gains `thinking: ThinkingMode` with two variants:

| Variant | Wire | When |
|---|---|---|
| `ProviderDefault` (default) | the `thinking` parameter is omitted | The model's default: thinking on, on every current model. |
| `Disabled` | `"thinking": {"type": "disabled"}` | The harness wants it off, on a model that allows it. |

No effort control, no adaptive or budget variants: those stay out of v2.
A model that rejects `disabled` answers 400, which the driver reports as
`error.provider` with the provider's text — a harness misconfiguration,
surfaced loudly on the first call. Mapping it to `error.unsupported` instead
would need a per-model table the provider already keeps, and would go stale
the day a model changes its mind.

## Consequences

- **A tool loop over a current model works past its first step.** The
  recorded fixtures carry a thinking block, and a stub-server test in
  `drivers/tests/` proves the second call of a two-turn exchange sends it
  back unchanged in value.
- **The kernel is untouched.** `kernel/src/abi/` did not change; the
  isolation test still holds; the bridge's own version absorbed the change,
  as ADR-0006 §1 intended.
- **#33 has its answer.** An OpenAI-compatible driver drops `anthropic`
  blocks and fills in its own row above if its provider has a reasoning
  format worth round-tripping. (Amended: it has one worth *reading*; see
  Amendments.)
- **`ScriptedModel` and every fixture say `v: 2`.** A test that pins the
  version pins it once, through `VERSION`.

## Alternatives considered

**A driver knob only, no block** (the issue's option 2). Rejected: it fixes
the loop only on models that accept `disabled`, and Fable does not. It also
trades away the model's own reasoning between tool calls, which is where it
matters most.

**Keep `v` at 1 by tolerating unknown blocks in the reader.** Rejected: it
reverses the "exhaustive on purpose" decision, and the loop would carry a
block it has no arm for without the compiler saying so. The bump is what §8
is for; following it the first time is the point.

**Typed fields (`signature`, `redacted`) instead of an opaque `data`.**
Rejected: the bridge would then learn every provider's reasoning format, and
each provider change would be a bridge change. The signature is a sealed
envelope; the bridge carries envelopes.

**Refuse foreign-provider blocks.** Rejected as above: unreadable by
definition, and the provider's own behaviour is to drop. Consistency with
the provider beat consistency with the rule.

**A `model` tag instead of `provider`.** Rejected: the provider already
decides per model, unbilled and without error, and a driver that second-
guessed it would need the provider's compatibility table.

## Amendments

- **2026-09-25** — [#123](https://github.com/tau-rs/tau/issues/123): the
  OpenAI-compatible column of §2 gains its own-provider row. A reply's
  `reasoning` (Ollama) or `reasoning_content` (vLLM) is sealed as one
  `thinking` block, `provider: openai-compatible`, `data` the field's value
  verbatim, first in `content`; on the way out it is dropped, as every
  `thinking` block on that wire is. Read-only by construction: the wire has
  no slot to send it back, so the bytes on the wire do not change and no
  cassette is re-recorded. The bridge is untouched — an existing variant
  gained a producer — so `v` stays 2.
