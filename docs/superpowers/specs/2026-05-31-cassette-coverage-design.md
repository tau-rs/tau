# LLM-plugin cassette test coverage — design

**Date:** 2026-05-31
**Status:** Draft (pending user review)
**Scope:** `crates/tau-plugin-test-support/` (thin helpers) + `crates/tau-plugins/{anthropic,openai,ollama}/tests/` (new assertions & cassettes). No production code changes.

> **Handoff note.** This spec is written to be executed by a fresh Claude Code
> session that does not have this conversation's context. It bundles the audit
> findings, the strategy, the exact API to add, and a prioritized, file-referenced
> work backlog. The implementing agent must follow the **CARGO RULES** in the
> repo-root `CLAUDE.md` for every cargo command (per-agent `CARGO_TARGET_DIR`,
> `-p <crate>`, `timeout`, `CARGO_INCREMENTAL=0`).

## Problem

tau's three LLM-provider plugins (`anthropic`, `openai`, `ollama`) are tested with
"cassette" tests: an in-process HTTP replayer
(`crates/tau-plugin-test-support/src/cassette.rs`) serves canned responses from
YAML files. An audit (2026-05-31) found the suites are **strong response-parsers
and error-mappers but weak request-validators**, with two concrete consequences:

1. **Provider auth is barely verified.** The replayer captures the incoming
   request (method, URI, headers, body) into `received_requests()`, but the
   Anthropic tests only ever read `.len()` (request *counts*), never `.headers`.
   Result: **the Anthropic plugin's API-key attachment is verified by no test
   anywhere.** If it stopped sending `x-api-key` tomorrow, every test would still
   pass. OpenAI and Ollama are saved only by a *separate* unit test in their
   `client.rs`; they have no cassette-layer auth assertion either.

2. **"Needed exchange data" is mostly unit-tested, not integration-tested.**
   Multi-turn conversations and sampling parameters (`temperature`/`top_p`/`stop`)
   are exercised in `request.rs` unit tests but never confirmed on the wire via a
   cassette test. Several error/streaming edges (5xx, extra stop reasons,
   multiple tool calls in a stream) lack integration cassettes.

### Root-cause analysis (and why we are NOT changing the replayer)

The replayer serves responses purely **in order** and ignores the cassette's
`request:` block. That sounds like the root cause, but it is not a *capability*
gap: `received_requests()` already exposes method, URI, headers, and body, and one
existing test already asserts on the sent body
(`crates/tau-plugins/anthropic/tests/complete.rs:33-39`, the system-prompt test).
**Every gap below closes with the replayer exactly as it is today.** A declarative
`expect:`-block matching engine was considered and rejected as weak ROI — it would
relocate assertions into YAML without unlocking any new coverage. See Non-goals.

## Credential-safety model (verified, and the invariant we add)

The replay path is structurally incapable of leaking a real credential:

- The key the plugin sends in tests is a **hardcoded fake constant**, stored as a
  `SecretString` and only un-wrapped when building the header:
  - Anthropic `"sk-ant-test"` — `crates/tau-plugins/anthropic/tests/common/mod.rs:42`
  - OpenAI `"sk-test-1234"` — `crates/tau-plugins/openai/src/client.rs:245`
  - Ollama `"hosted-token-xyz"` (plus a no-token case) — `crates/tau-plugins/ollama/src/client.rs:284` / `:262`
- The plugin posts to `http://127.0.0.1:<ephemeral>` (the in-process replayer).
  **No request ever leaves the process toward a real provider.**
- The `x-api-key: "<REDACTED>"` strings in committed cassette YAML sit in the
  `request:` block, which the replayer never reads — cosmetic, never compared,
  never a real key.
- A real API key is read **only** by the opt-in `live.rs` tests (`#[ignore]`,
  gated on an env var), which write nothing to disk. Those are untouched.

**Every new assertion compares a captured value against the known fake constant
(fake-vs-fake), so the work needs no real credential and stays fully offline.**

**New invariant (cred-safety guard):** because this work *adds* cassettes, ship a
single workspace test that scans `crates/tau-plugins/*/tests/**/*.yaml` and fails
if any header value looks like a *real* key (high-entropy `sk-ant-…` / `sk-…`
beyond the short known fakes) rather than a known fake. This converts "no real
creds in cassettes" from convention into a CI-enforced guarantee.

## Strategy: tests-only + thin helpers

No replayer behavior change. Add a few ergonomic assertion helpers, then close the
gaps with new asserting tests and cassettes. Phased so the genuine hole closes
first and each phase is independently shippable.

### Phase 0 — thin helpers (`tau-plugin-test-support`)

Add methods to `RecordedRequest` (chosen over methods on `CassetteServer` so they
compose with retry tests that index into the `received_requests()` vec):

```rust
impl RecordedRequest {
    /// Case-insensitive header lookup. Headers are stored lowercased
    /// (see cassette.rs parse_request), so callers may pass any casing.
    pub fn header(&self, name: &str) -> Option<&str>;

    /// Assert a header is present and exactly equals `expected`.
    /// Panics with a message including the actual headers on mismatch.
    pub fn assert_header(&self, name: &str, expected: &str);

    /// Parse the captured body as JSON. Panics with a clear message
    /// (including a body excerpt) if the body is not valid JSON.
    pub fn body_json(&self) -> serde_json::Value;

    /// Assert `expected` is a recursive subset of the sent JSON body:
    /// every key in `expected` must be present with an equal value, but
    /// the sent body may contain extra keys and any field ordering.
    pub fn assert_body_subset(&self, expected: serde_json::Value);
}
```

Subset semantics (must be unit-tested in `tau-plugin-test-support`):
- Objects: every key in `expected` present in actual with a recursively-matching
  value; extra keys in actual are allowed.
- Arrays: same length, element-wise recursive match (order significant — message
  order matters for LLM requests).
- Scalars: exact equality.

Usage:
```rust
let sent = &server.received_requests()[0];
sent.assert_header("x-api-key", "sk-ant-test");
sent.assert_body_subset(serde_json::json!({ "model": "claude-x", "stream": false }));
```

Estimated surface: ~40 LOC + matcher unit tests. No cassette-format change.

### Phase 1 — close the genuine hole (P0)

- **Anthropic auth assertion.** In an existing happy-path cassette test, assert the
  sent request carries `x-api-key == "sk-ant-test"` and
  `anthropic-version == "2023-06-01"`. (`anthropic-version` is sent at
  `crates/tau-plugins/anthropic/src/client.rs:70`.)

### Phase 2 — backfill the "needed exchange data" gaps

Priority **P1** (the exchange data explicitly in scope), applied to all three
providers unless noted:

- **Multi-turn round-trip.** A cassette test driving
  user → assistant(tool_use) → tool_result, asserting (via `assert_body_subset`)
  that the *sent* body carries the full message history in the correct per-provider
  wire shape. Today only unit-tested in each plugin's `request.rs`.
- **Sampling params on the wire.** Assert `temperature` / `top_p` / `stop`
  (and `max_tokens` → `num_predict` for Ollama) appear in the sent body. Today only
  unit-tested.
- **Auth parity.** Add the cassette-layer header assert to OpenAI
  (`authorization: Bearer sk-test-1234`) and Ollama (present-when-set via the
  hosted-token config; absent-when-local), so all three verify auth identically at
  the integration layer.

Priority **P2** (error & streaming edges):

- **5xx integration cassette** for Anthropic + OpenAI (a 500/503 response asserted
  to map to the retryable `Provider` error). Ollama already covers 503-retry via
  `complete_503_model_loading_then_success.yaml`.
- **Anthropic stop reasons** `MaxTokens` and `StopSequence` (cassette + assert);
  only `EndTurn` and `ToolUse` are covered today.
- **OpenAI** multiple tool calls in one stream, and a mid-stream error event
  (only stream truncation is covered today).

### Phase guard — cred-safety scan

Ship the YAML key-scan guard test described in the credential-safety section.
Order is flexible, but it should land before/with the first phase that adds new
cassettes so new files are covered immediately.

## Coverage backlog (reference matrix)

`✅` cassette+assertion today · `⚠️` unit-test only (not integration) · `❌` absent.
Target column is the post-implementation state.

| Scenario | Anthropic | OpenAI | Ollama | Target |
|---|---|---|---|---|
| Auth header attached (asserted at cassette layer) | ❌ | ⚠️ unit | ⚠️ unit | ✅ all |
| Multi-turn round-trip (assistant + tool_result on wire) | ⚠️ | ⚠️ | ⚠️ | ✅ all |
| Sampling params reach the wire | ⚠️ | ⚠️ | ⚠️ | ✅ all |
| 5xx server error (integration) | ❌ | ❌ | ✅ | ✅ all |
| Stop reasons MaxTokens / StopSequence | ❌ | ⚠️ | ✅ | ✅ all |
| Multiple tool calls in one stream | ❌ | ❌ | ⚠️ | ✅ O |
| Mid-stream error event | ✅ | ❌ | ✅ | ✅ all |
| No real key in any cassette YAML | convention | convention | convention | ✅ CI-enforced |

## Testing approach

- All new assertions compare captured values against the hardcoded fake constants;
  fully offline, deterministic, no network. Reuse each plugin's existing
  `tests/common` config builders (e.g. `common::test_config(base_url)`).
- New cassette YAML follows the existing files in each plugin's `tests/cassettes/`.
- Run per the CARGO RULES, e.g.
  `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p anthropic`.
- The `live.rs` opt-in drift tests remain the quarterly real-API check; untouched.

## Non-goals

- **No replayer behavior change / no `expect:`-block matching engine.** Decided:
  weak ROI; `received_requests()` already exposes everything needed.
- **No `request:`-block-authoritative migration** (would force rewriting ~45
  existing cassettes that use `body: placeholder` / `<REDACTED>`).
- **No auto-record mode** (`TAU_RECORD_CASSETTES`) — a separate future project; the
  `rerecord-anthropic-cassettes.sh` stub stays as-is.
- **No production-code changes** to any plugin. This is a test-coverage effort only.

## Key file references

| Purpose | Path |
|---|---|
| Replayer + `RecordedRequest` (Phase 0 target) | `crates/tau-plugin-test-support/src/cassette.rs` |
| Existing sent-body assertion to mirror | `crates/tau-plugins/anthropic/tests/complete.rs:33-39` |
| Anthropic auth header send site | `crates/tau-plugins/anthropic/src/client.rs:69-70` |
| Anthropic fake key | `crates/tau-plugins/anthropic/tests/common/mod.rs:42` |
| OpenAI auth unit test (parity reference) | `crates/tau-plugins/openai/src/client.rs:268-272` |
| Ollama auth unit tests (present/absent) | `crates/tau-plugins/ollama/src/client.rs:262,282-299` |
| Cassette directories (per plugin) | `crates/tau-plugins/{anthropic,openai,ollama}/tests/cassettes/` |
