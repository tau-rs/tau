# LLM-plugin cassette coverage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the LLM-plugin cassette test coverage gaps (Anthropic auth never verified; multi-turn, sampling params, and several error/streaming behaviors only unit-tested) using the existing in-process replayer plus a few thin assertion helpers — no replayer behavior change.

**Architecture:** Add assertion helpers to `RecordedRequest` in `tau-plugin-test-support`, then add asserting integration tests in each plugin. Request-shape tests (auth, sampling, multi-turn) **reuse existing happy-path cassettes** — because the replayer serves responses by order and ignores the request, you only need to drive `complete()`/`stream()` and assert on `server.received_requests()`. New cassette YAML is required only for new *response* behaviors (5xx, extra stop reasons, mid-stream error, multiple stream tool calls). A CI guard test enforces that no real API key ever lands in a cassette.

**Tech Stack:** Rust, `tokio`, `serde_json`, `serde_yaml`; the bespoke cassette replayer in `crates/tau-plugin-test-support/src/cassette.rs`; provider plugins under `crates/tau-plugins/{anthropic,openai,ollama}/`.

**Spec:** `docs/superpowers/specs/2026-05-31-cassette-coverage-design.md`

---

## ⚠️ Mandatory environment rules (read before any cargo command)

This workspace shares one cargo lock across 8 crates. Per the repo-root `CLAUDE.md` **CARGO RULES**, EVERY cargo command MUST be:

```
timeout <T> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p <crate>
```

- Always `-p <crate>` (never bare/workspace).
- Timeouts: test 300s, build/check 180s, clippy 240s, fmt 30s.
- Prefer `cargo nextest run -p <crate>` for tests (CI parity); use `cargo test -p <crate> --doc` only for doctests.
- Before launching, `pgrep -af cargo | grep -v grep`; if another build uses `target/agent-impl`, use `target/agent-impl-2`.

**Commits:** this repo's lefthook hooks can corrupt git identity and HEAD. Commit with explicit identity and skip hooks (changes are test-only; CI is the gate):

```
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "<msg>"
```

Crate `-p` names: `tau-plugin-test-support`, and the plugin lib targets are `-p anthropic`, `-p openai`, `-p ollama` (the crate package names).

---

## Reference: verified APIs (do not re-derive)

`RecordedRequest` (in `crates/tau-plugin-test-support/src/cassette.rs:29-41`) — headers are stored with **lowercased keys**, values trimmed:

```rust
pub struct RecordedRequest {
    pub method: String,
    pub uri: String,
    pub headers: std::collections::HashMap<String, String>, // lowercased keys
    pub body: String,
}
```

`CompletionRequest` sampling fields (`crates/tau-ports/src/llm.rs:69-99`): `temperature: Option<f32>`, `top_p: Option<f32>`, `seed: Option<u64>`, `stop_sequences: Vec<String>`, `max_tokens: Option<u32>`.

Message constructors (`tau-ports/src/llm.rs:138-162`): `LlmProviderMessage::user(Vec<ContentBlock>)`, `::assistant(Vec<ContentBlock>)`, `::tool_result(tool_use_id: String, content: Vec<ContentBlock>, is_error: bool)`. `ContentBlock::Text(String)` / `ContentBlock::ToolUse(ToolUse)`. `ToolUse::new(id: String, name: String, input: tau_domain::Value)`.

`CompletionResponse` (`tau-ports/src/llm.rs`): `text: String`, `tool_uses: Vec<ToolUse>`, `stop_reason: StopReason`, `usage: Option<TokenUsage>`.

`StopReason`: `EndTurn | MaxTokens | StopSequence | ToolUse | Error`.

`CompletionChunk`: `Text { delta: String }`, `ToolUse(ToolUse)`, `Finish { stop_reason: StopReason, usage: Option<TokenUsage> }`.

Per-plugin test helpers already exist in each `tests/common/mod.rs`: `sample_request()`, `extract_text()`, `test_config(base_url)`, `test_config_with_retry(base_url, max_attempts, base_delay_ms)`. Fake keys: anthropic `"sk-ant-test"` (+ `anthropic-version` default `"2023-06-01"`), openai `"sk-test"`, ollama no bearer token.

5xx mapping (all three plugins): `500..=599 => LlmError::Provider { message }`.

---

## File Structure

- Modify `crates/tau-plugin-test-support/Cargo.toml` — add `serde_json` dep.
- Modify `crates/tau-plugin-test-support/src/cassette.rs` — `impl RecordedRequest` helpers + their unit tests.
- Create `crates/tau-plugin-test-support/src/secret_scan.rs` + register in `src/lib.rs` — the cred-safety guard helper.
- Modify `crates/tau-plugins/anthropic/tests/complete.rs` — auth, sampling, multi-turn, 500, stop-reason tests.
- Create `crates/tau-plugins/anthropic/tests/cassettes/complete_500_server_error.yaml`, `complete_stop_reason_max_tokens.yaml`, `complete_stop_reason_stop_sequence.yaml`.
- Modify `crates/tau-plugins/openai/tests/complete.rs` — auth, sampling, multi-turn, 500 tests.
- Modify `crates/tau-plugins/openai/tests/streaming.rs` — multiple-tool-calls + mid-stream-error tests.
- Create `crates/tau-plugins/openai/tests/cassettes/complete_500_server_error.yaml`, `stream_multiple_tool_calls.yaml`, `stream_error_mid_stream.yaml`.
- Modify `crates/tau-plugins/ollama/tests/complete.rs` — auth-absence, sampling, multi-turn tests.
- Create one tiny guard test in each plugin: `crates/tau-plugins/{anthropic,openai,ollama}/tests/no_real_secrets.rs`.

---

## Phase 0 — assertion helpers

### Task 1: `RecordedRequest` assertion helpers

**Files:**
- Modify: `crates/tau-plugin-test-support/Cargo.toml`
- Modify: `crates/tau-plugin-test-support/src/cassette.rs`

- [ ] **Step 1: Add `serde_json` dependency**

In `crates/tau-plugin-test-support/Cargo.toml`, under `[dependencies]` (after the `serde_yaml` line):

```toml
serde_json  = { workspace = true }
```

- [ ] **Step 2: Write the failing unit tests**

Append to the existing `mod self_tests` block at the bottom of `crates/tau-plugin-test-support/src/cassette.rs` (inside the `#[cfg(test)] mod self_tests { ... }`):

```rust
    #[test]
    fn header_lookup_is_case_insensitive() {
        let mut req = RecordedRequest::default();
        req.headers.insert("x-api-key".into(), "sk-ant-test".into());
        assert_eq!(req.header("X-API-Key"), Some("sk-ant-test"));
        assert_eq!(req.header("x-api-key"), Some("sk-ant-test"));
        assert_eq!(req.header("missing"), None);
    }

    #[test]
    fn assert_header_passes_on_exact_match() {
        let mut req = RecordedRequest::default();
        req.headers.insert("authorization".into(), "Bearer sk-test".into());
        req.assert_header("Authorization", "Bearer sk-test"); // must not panic
    }

    #[test]
    #[should_panic(expected = "header")]
    fn assert_header_panics_on_mismatch() {
        let req = RecordedRequest::default();
        req.assert_header("x-api-key", "sk-ant-test");
    }

    #[test]
    fn body_subset_matches_ignoring_extra_keys_and_order() {
        let mut req = RecordedRequest::default();
        req.body = r#"{"model":"m","stream":false,"extra":1}"#.into();
        req.assert_body_subset(serde_json::json!({ "stream": false, "model": "m" }));
    }

    #[test]
    #[should_panic(expected = "subset")]
    fn body_subset_panics_when_value_differs() {
        let mut req = RecordedRequest::default();
        req.body = r#"{"model":"m"}"#.into();
        req.assert_body_subset(serde_json::json!({ "model": "other" }));
    }

    #[test]
    fn body_subset_matches_nested_arrays_in_order() {
        let mut req = RecordedRequest::default();
        req.body = r#"{"messages":[{"role":"user"},{"role":"assistant"}]}"#.into();
        req.assert_body_subset(serde_json::json!({
            "messages": [{ "role": "user" }, { "role": "assistant" }]
        }));
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-plugin-test-support --lib`
Expected: FAIL to compile — `no method named header`/`assert_header`/`assert_body_subset`.

- [ ] **Step 4: Implement the helpers**

Add this `impl` block to `crates/tau-plugin-test-support/src/cassette.rs`, immediately after the `RecordedRequest` struct definition (after its closing `}` near line 41):

```rust
impl RecordedRequest {
    /// Case-insensitive header lookup. Headers are stored with
    /// lowercased keys (see `parse_request`), so any casing works.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_lowercase()).map(String::as_str)
    }

    /// Assert a header is present and exactly equals `expected`.
    pub fn assert_header(&self, name: &str, expected: &str) {
        match self.header(name) {
            Some(actual) => assert_eq!(
                actual, expected,
                "header `{name}` mismatch: expected `{expected}`, got `{actual}`"
            ),
            None => panic!(
                "expected header `{name}` to be present; headers were: {:?}",
                self.headers
            ),
        }
    }

    /// Parse the captured request body as JSON.
    pub fn body_json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| {
            let excerpt: String = self.body.chars().take(200).collect();
            panic!("request body was not valid JSON ({e}); body started: {excerpt}")
        })
    }

    /// Assert `expected` is a recursive subset of the sent JSON body:
    /// every key in an `expected` object must be present with a
    /// recursively-matching value (extra keys allowed); arrays match
    /// element-wise in order; scalars match exactly.
    pub fn assert_body_subset(&self, expected: serde_json::Value) {
        let actual = self.body_json();
        if let Err(path) = json_subset(&expected, &actual) {
            panic!(
                "request body is not a superset of expected at `{path}`.\n\
                 expected subset: {expected}\nactual body: {actual}"
            );
        }
    }
}

/// Returns `Ok(())` if `expected` is a recursive subset of `actual`,
/// else `Err(path)` naming the first mismatching JSON path.
fn json_subset(expected: &serde_json::Value, actual: &serde_json::Value) -> Result<(), String> {
    use serde_json::Value;
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for (k, ev) in e {
                match a.get(k) {
                    Some(av) => json_subset(ev, av).map_err(|p| format!("{k}.{p}"))?,
                    None => return Err(k.clone()),
                }
            }
            Ok(())
        }
        (Value::Array(e), Value::Array(a)) => {
            if e.len() != a.len() {
                return Err(format!("[len {} != {}]", e.len(), a.len()));
            }
            for (i, (ev, av)) in e.iter().zip(a.iter()).enumerate() {
                json_subset(ev, av).map_err(|p| format!("[{i}].{p}"))?;
            }
            Ok(())
        }
        _ => {
            if expected == actual {
                Ok(())
            } else {
                Err(String::new())
            }
        }
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-plugin-test-support --lib`
Expected: PASS (all `self_tests`, including the new six).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-plugin-test-support/Cargo.toml crates/tau-plugin-test-support/src/cassette.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "test(plugin-test-support): RecordedRequest assertion helpers"
```

---

## Phase 1 — close the genuine hole (Anthropic auth)

### Task 2: Anthropic auth header assertion

**Files:**
- Test: `crates/tau-plugins/anthropic/tests/complete.rs` (add a test fn)

- [ ] **Step 1: Write the failing test**

Append to `crates/tau-plugins/anthropic/tests/complete.rs`:

```rust
#[tokio::test]
async fn complete_sends_auth_headers() {
    let server = cassette::replay("tests/cassettes/complete_happy_path.yaml").await;
    let plugin = AnthropicPlugin::from_config(common::test_config(server.uri().into())).unwrap();
    let _ = plugin.complete(common::sample_request()).await.unwrap();

    let sent = &server.received_requests()[0];
    sent.assert_header("x-api-key", "sk-ant-test");
    sent.assert_header("anthropic-version", "2023-06-01");
}
```

- [ ] **Step 2: Run it to verify it fails (then passes once Task 1 is merged)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p anthropic complete_sends_auth_headers`
Expected: if Task 1 is already merged, this should PASS immediately (the plugin already sends the headers — this test proves it). If it FAILS, the failure pinpoints a real regression. Confirm the assertion is actually exercised by temporarily changing `"sk-ant-test"` to `"wrong"` and observing a failure, then revert.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-plugins/anthropic/tests/complete.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "test(anthropic): assert x-api-key + anthropic-version are sent"
```

---

## Phase 2a — auth parity (reuse happy-path cassettes)

### Task 3: OpenAI auth header assertion

**Files:**
- Test: `crates/tau-plugins/openai/tests/complete.rs`

- [ ] **Step 1: Write the test**

Append to `crates/tau-plugins/openai/tests/complete.rs` (it already has `mod common; use common::cassette;` and imports `OpenAIPlugin`; mirror the file's existing test setup):

```rust
#[tokio::test]
async fn complete_sends_bearer_auth() {
    let server = cassette::replay("tests/cassettes/complete_happy_path.yaml").await;
    let plugin = OpenAIPlugin::from_config(common::test_config(server.uri().into())).unwrap();
    let _ = plugin.complete(common::sample_request()).await.unwrap();

    let sent = &server.received_requests()[0];
    sent.assert_header("authorization", "Bearer sk-test");
}
```

If the plugin type / import path differs, copy it from the top of the existing `complete.rs` in that crate. The cassette `tests/cassettes/complete_happy_path.yaml` already exists.

- [ ] **Step 2: Run it**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p openai complete_sends_bearer_auth`
Expected: PASS. Verify it bites by temporarily breaking the expected value, then revert.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-plugins/openai/tests/complete.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "test(openai): assert Authorization: Bearer is sent"
```

### Task 4: Ollama auth-absence assertion

**Files:**
- Test: `crates/tau-plugins/ollama/tests/complete.rs`

- [ ] **Step 1: Write the test**

The ollama `test_config` sets no bearer token (local deployment), so the plugin must send NO `Authorization` header. Append to `crates/tau-plugins/ollama/tests/complete.rs`:

```rust
#[tokio::test]
async fn complete_sends_no_auth_header_when_local() {
    let server = cassette::replay("tests/cassettes/complete_happy_path.yaml").await;
    let plugin = OllamaPlugin::from_config(common::test_config(server.uri().into())).unwrap();
    let _ = plugin.complete(common::sample_request()).await.unwrap();

    let sent = &server.received_requests()[0];
    assert!(
        sent.header("authorization").is_none(),
        "local Ollama must not send an Authorization header; got: {:?}",
        sent.header("authorization"),
    );
}
```

Copy the `OllamaPlugin` import / plugin type from the top of the existing `complete.rs` if it differs.

- [ ] **Step 2: Run it**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p ollama complete_sends_no_auth_header_when_local`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-plugins/ollama/tests/complete.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "test(ollama): assert no Authorization header for local deployments"
```

---

## Phase 2b — sampling params on the wire (reuse happy-path cassettes)

### Task 5: Anthropic sampling params reach the wire

**Files:**
- Test: `crates/tau-plugins/anthropic/tests/complete.rs`

- [ ] **Step 1: Write the test**

Append to `crates/tau-plugins/anthropic/tests/complete.rs`:

```rust
#[tokio::test]
async fn complete_sends_sampling_params_in_body() {
    let server = cassette::replay("tests/cassettes/complete_happy_path.yaml").await;
    let plugin = AnthropicPlugin::from_config(common::test_config(server.uri().into())).unwrap();
    let mut req = common::sample_request();
    req.temperature = Some(0.7);
    req.top_p = Some(0.9);
    req.stop_sequences = vec!["\n\n".into()];
    let _ = plugin.complete(req).await.unwrap();

    let body = server.received_requests()[0].body_json();
    let temp = body.get("temperature").unwrap().as_f64().unwrap();
    assert!((temp - 0.7).abs() < 1e-6, "temperature was {temp}");
    let top_p = body.get("top_p").unwrap().as_f64().unwrap();
    assert!((top_p - 0.9).abs() < 1e-6, "top_p was {top_p}");
    assert_eq!(
        body.get("stop_sequences").unwrap().as_array().unwrap(),
        &vec![serde_json::json!("\n\n")],
    );
}
```

- [ ] **Step 2: Run it**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p anthropic complete_sends_sampling_params_in_body`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-plugins/anthropic/tests/complete.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "test(anthropic): assert sampling params reach the request body"
```

### Task 6: OpenAI sampling params reach the wire (top-level)

**Files:**
- Test: `crates/tau-plugins/openai/tests/complete.rs`

- [ ] **Step 1: Write the test**

OpenAI puts sampling params at the top level (NOT under an `options` object). Append to `crates/tau-plugins/openai/tests/complete.rs`:

```rust
#[tokio::test]
async fn complete_sends_sampling_params_top_level() {
    let server = cassette::replay("tests/cassettes/complete_happy_path.yaml").await;
    let plugin = OpenAIPlugin::from_config(common::test_config(server.uri().into())).unwrap();
    let mut req = common::sample_request();
    req.max_tokens = Some(100);
    req.temperature = Some(0.7);
    req.top_p = Some(0.9);
    req.seed = Some(42);
    req.stop_sequences = vec!["END".into()];
    let _ = plugin.complete(req).await.unwrap();

    let body = server.received_requests()[0].body_json();
    assert!(body.get("options").is_none(), "openai must not nest under options");
    assert_eq!(body["max_tokens"], 100);
    assert_eq!(body["temperature"], f64::from(0.7f32));
    assert_eq!(body["top_p"], f64::from(0.9f32));
    assert_eq!(body["seed"], 42);
    assert_eq!(body["stop"], serde_json::json!(["END"]));
}
```

- [ ] **Step 2: Run it**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p openai complete_sends_sampling_params_top_level`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-plugins/openai/tests/complete.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "test(openai): assert sampling params reach the request body"
```

### Task 7: Ollama sampling params reach the wire (under `options`, Ollama names)

**Files:**
- Test: `crates/tau-plugins/ollama/tests/complete.rs`

- [ ] **Step 1: Write the test**

Ollama nests sampling params under `options` and renames `max_tokens` → `num_predict`. Append to `crates/tau-plugins/ollama/tests/complete.rs`:

```rust
#[tokio::test]
async fn complete_sends_sampling_params_under_options() {
    let server = cassette::replay("tests/cassettes/complete_happy_path.yaml").await;
    let plugin = OllamaPlugin::from_config(common::test_config(server.uri().into())).unwrap();
    let mut req = common::sample_request();
    req.max_tokens = Some(100);
    req.temperature = Some(0.7);
    req.top_p = Some(0.9);
    req.seed = Some(42);
    req.stop_sequences = vec!["END".into()];
    let _ = plugin.complete(req).await.unwrap();

    let body = server.received_requests()[0].body_json();
    let opts = body["options"].as_object().unwrap();
    assert_eq!(opts["num_predict"], 100); // NOT max_tokens
    assert_eq!(opts["temperature"], f64::from(0.7f32));
    assert_eq!(opts["top_p"], f64::from(0.9f32));
    assert_eq!(opts["seed"], 42);
    assert_eq!(opts["stop"], serde_json::json!(["END"]));
}
```

- [ ] **Step 2: Run it**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p ollama complete_sends_sampling_params_under_options`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-plugins/ollama/tests/complete.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "test(ollama): assert sampling params reach options sub-object"
```

---

## Phase 2c — multi-turn round-trip (reuse happy-path cassettes)

Each test builds a user → assistant(text + tool_use) → tool_result conversation and asserts the *sent* body carries it in the provider's wire shape. Imports needed at the top of each `complete.rs`: add `use tau_ports::{ContentBlock, LlmProviderMessage, ToolUse};` and `use tau_domain::Value;` if not already imported.

### Task 8: Anthropic multi-turn round-trip

**Files:**
- Test: `crates/tau-plugins/anthropic/tests/complete.rs`

- [ ] **Step 1: Write the test**

```rust
#[tokio::test]
async fn complete_sends_multi_turn_history() {
    use tau_domain::Value;
    use tau_ports::{ContentBlock, LlmProviderMessage, ToolUse};

    let server = cassette::replay("tests/cassettes/complete_happy_path.yaml").await;
    let plugin = AnthropicPlugin::from_config(common::test_config(server.uri().into())).unwrap();
    let mut req = common::sample_request(); // seeds messages[0] = user "say hi"
    req.messages.push(LlmProviderMessage::assistant(vec![
        ContentBlock::Text("calling tool".into()),
        ContentBlock::ToolUse(ToolUse::new(
            "tu_01".into(),
            "echo".into(),
            Value::Object(Default::default()),
        )),
    ]));
    req.messages.push(LlmProviderMessage::tool_result(
        "tu_01".into(),
        vec![ContentBlock::Text("result".into())],
        false,
    ));
    let _ = plugin.complete(req).await.unwrap();

    let body = server.received_requests()[0].body_json();
    let messages = body["messages"].as_array().unwrap();
    // [0]=user, [1]=assistant(text+tool_use), [2]=tool_result (Anthropic shapes as user role).
    let assistant = &messages[1];
    assert_eq!(assistant["role"], "assistant");
    let content = assistant["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[1]["type"], "tool_use");
    assert_eq!(content[1]["id"], "tu_01");
    assert_eq!(content[1]["name"], "echo");
    let tool_result = &messages[2];
    assert_eq!(tool_result["role"], "user");
    assert_eq!(tool_result["content"][0]["type"], "tool_result");
    assert_eq!(tool_result["content"][0]["tool_use_id"], "tu_01");
}
```

- [ ] **Step 2: Run it**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p anthropic complete_sends_multi_turn_history`
Expected: PASS. If a field path mismatches, inspect the printed body and align to the actual wire shape (cross-check `crates/tau-plugins/anthropic/src/request.rs` tests at lines 308-353).

- [ ] **Step 3: Commit**

```bash
git add crates/tau-plugins/anthropic/tests/complete.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "test(anthropic): assert multi-turn history reaches the wire"
```

### Task 9: OpenAI multi-turn round-trip

**Files:**
- Test: `crates/tau-plugins/openai/tests/complete.rs`

- [ ] **Step 1: Write the test**

OpenAI flattens assistant text to a string + `tool_calls` array, and a tool result becomes a `role:"tool"` message keyed by `tool_call_id`:

```rust
#[tokio::test]
async fn complete_sends_multi_turn_history() {
    use tau_domain::Value;
    use tau_ports::{ContentBlock, LlmProviderMessage, ToolUse};

    let server = cassette::replay("tests/cassettes/complete_happy_path.yaml").await;
    let plugin = OpenAIPlugin::from_config(common::test_config(server.uri().into())).unwrap();
    let mut req = common::sample_request();
    req.messages.push(LlmProviderMessage::assistant(vec![
        ContentBlock::Text("ok let me ".into()),
        ContentBlock::ToolUse(ToolUse::new(
            "call_abc123".into(),
            "echo".into(),
            Value::Object(Default::default()),
        )),
    ]));
    req.messages.push(LlmProviderMessage::tool_result(
        "call_abc123".into(),
        vec![ContentBlock::Text("42".into())],
        false,
    ));
    let _ = plugin.complete(req).await.unwrap();

    let body = server.received_requests()[0].body_json();
    let messages = body["messages"].as_array().unwrap();
    let assistant = &messages[1];
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["content"], "ok let me ");
    let calls = assistant["tool_calls"].as_array().unwrap();
    assert_eq!(calls[0]["id"], "call_abc123");
    assert_eq!(calls[0]["type"], "function");
    assert_eq!(calls[0]["function"]["name"], "echo");
    let tool_msg = &messages[2];
    assert_eq!(tool_msg["role"], "tool");
    assert_eq!(tool_msg["tool_call_id"], "call_abc123");
    assert_eq!(tool_msg["content"], "42");
}
```

- [ ] **Step 2: Run it**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p openai complete_sends_multi_turn_history`
Expected: PASS. Cross-check shape against `crates/tau-plugins/openai/src/request.rs` tests (lines 280-344) if a path mismatches.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-plugins/openai/tests/complete.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "test(openai): assert multi-turn history reaches the wire"
```

### Task 10: Ollama multi-turn round-trip

**Files:**
- Test: `crates/tau-plugins/ollama/tests/complete.rs`

- [ ] **Step 1: Write the test**

Ollama emits a `tool_calls` array on the assistant message and a `role:"tool"` message WITHOUT a `tool_use_id` (it pairs by order):

```rust
#[tokio::test]
async fn complete_sends_multi_turn_history() {
    use tau_domain::Value;
    use tau_ports::{ContentBlock, LlmProviderMessage, ToolUse};

    let server = cassette::replay("tests/cassettes/complete_happy_path.yaml").await;
    let plugin = OllamaPlugin::from_config(common::test_config(server.uri().into())).unwrap();
    let mut req = common::sample_request();
    req.messages.push(LlmProviderMessage::assistant(vec![
        ContentBlock::Text("ok let me ".into()),
        ContentBlock::ToolUse(ToolUse::new(
            "ollama-tool-0".into(),
            "echo".into(),
            Value::Object(Default::default()),
        )),
    ]));
    req.messages.push(LlmProviderMessage::tool_result(
        "ollama-tool-0".into(),
        vec![ContentBlock::Text("42".into())],
        false,
    ));
    let _ = plugin.complete(req).await.unwrap();

    let body = server.received_requests()[0].body_json();
    let messages = body["messages"].as_array().unwrap();
    let assistant = &messages[1];
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["content"], "ok let me ");
    assert_eq!(assistant["tool_calls"].as_array().unwrap()[0]["function"]["name"], "echo");
    let tool_msg = &messages[2];
    assert_eq!(tool_msg["role"], "tool");
    assert_eq!(tool_msg["content"], "42");
    assert!(tool_msg.get("tool_use_id").is_none(), "ollama pairs by order, not id");
}
```

- [ ] **Step 2: Run it**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p ollama complete_sends_multi_turn_history`
Expected: PASS. Cross-check shape against `crates/tau-plugins/ollama/src/request.rs` tests (lines 262-301) if needed.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-plugins/ollama/tests/complete.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "test(ollama): assert multi-turn history reaches the wire"
```

---

## Phase 2d — response-behavior gaps (NEW cassettes)

### Task 11: Anthropic 500 server error → `Provider`

**Files:**
- Create: `crates/tau-plugins/anthropic/tests/cassettes/complete_500_server_error.yaml`
- Test: `crates/tau-plugins/anthropic/tests/complete.rs`

- [ ] **Step 1: Create the cassette**

`crates/tau-plugins/anthropic/tests/cassettes/complete_500_server_error.yaml`:

```yaml
- request:
    method: POST
    uri: /v1/messages
    headers:
      x-api-key: "<REDACTED>"
      anthropic-version: "2023-06-01"
    body: |-
      placeholder
  response:
    status: 500
    headers:
      content-type: application/json
    body: |-
      {"type":"error","error":{"type":"api_error","message":"internal server error"}}
```

- [ ] **Step 2: Write the failing test**

Append to `crates/tau-plugins/anthropic/tests/complete.rs` (run with `max_attempts = 1` so the single 500 is the final result):

```rust
#[tokio::test]
async fn complete_500_maps_to_provider_error() {
    let server = cassette::replay("tests/cassettes/complete_500_server_error.yaml").await;
    let plugin =
        AnthropicPlugin::from_config(common::test_config_with_retry(server.uri().into(), 1, 0))
            .unwrap();
    let err = plugin.complete(common::sample_request()).await.unwrap_err();
    let LlmError::Provider { ref message } = err else {
        panic!("expected Provider, got {err:?}");
    };
    assert!(message.contains("server error"), "unexpected message: {message}");
}
```

- [ ] **Step 3: Run it**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p anthropic complete_500_maps_to_provider_error`
Expected: PASS. (If the variant differs, inspect `tau_ports::LlmError` and the mapping at `crates/tau-plugins/anthropic/src/error.rs:86`.)

- [ ] **Step 4: Commit**

```bash
git add crates/tau-plugins/anthropic/tests/cassettes/complete_500_server_error.yaml crates/tau-plugins/anthropic/tests/complete.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "test(anthropic): 500 maps to retryable Provider error"
```

### Task 12: Anthropic stop reasons MaxTokens + StopSequence

**Files:**
- Create: `crates/tau-plugins/anthropic/tests/cassettes/complete_stop_reason_max_tokens.yaml`
- Create: `crates/tau-plugins/anthropic/tests/cassettes/complete_stop_reason_stop_sequence.yaml`
- Test: `crates/tau-plugins/anthropic/tests/complete.rs`

- [ ] **Step 1: Create both cassettes**

`complete_stop_reason_max_tokens.yaml` (same shape as happy path; `stop_reason` changed):

```yaml
- request:
    method: POST
    uri: /v1/messages
    headers:
      x-api-key: "<REDACTED>"
      anthropic-version: "2023-06-01"
    body: |-
      placeholder
  response:
    status: 200
    headers:
      content-type: application/json
    body: |-
      {"id":"msg_01ABC","type":"message","role":"assistant","content":[{"type":"text","text":"truncated"}],"model":"claude-3-5-haiku-latest","stop_reason":"max_tokens","usage":{"input_tokens":12,"output_tokens":20}}
```

`complete_stop_reason_stop_sequence.yaml` (identical except the final `stop_reason`):

```yaml
- request:
    method: POST
    uri: /v1/messages
    headers:
      x-api-key: "<REDACTED>"
      anthropic-version: "2023-06-01"
    body: |-
      placeholder
  response:
    status: 200
    headers:
      content-type: application/json
    body: |-
      {"id":"msg_01ABC","type":"message","role":"assistant","content":[{"type":"text","text":"stopped"}],"model":"claude-3-5-haiku-latest","stop_reason":"stop_sequence","usage":{"input_tokens":12,"output_tokens":5}}
```

- [ ] **Step 2: Write the failing tests**

Append to `crates/tau-plugins/anthropic/tests/complete.rs` (the `StopReason` import: add `use tau_ports::StopReason;` to the existing `use tau_ports::{...}` line):

```rust
#[tokio::test]
async fn complete_maps_max_tokens_stop_reason() {
    let server = cassette::replay("tests/cassettes/complete_stop_reason_max_tokens.yaml").await;
    let plugin = AnthropicPlugin::from_config(common::test_config(server.uri().into())).unwrap();
    let resp = plugin.complete(common::sample_request()).await.unwrap();
    assert_eq!(resp.stop_reason, tau_ports::StopReason::MaxTokens);
}

#[tokio::test]
async fn complete_maps_stop_sequence_stop_reason() {
    let server = cassette::replay("tests/cassettes/complete_stop_reason_stop_sequence.yaml").await;
    let plugin = AnthropicPlugin::from_config(common::test_config(server.uri().into())).unwrap();
    let resp = plugin.complete(common::sample_request()).await.unwrap();
    assert_eq!(resp.stop_reason, tau_ports::StopReason::StopSequence);
}
```

- [ ] **Step 3: Run them**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p anthropic stop_reason`
Expected: PASS (both).

- [ ] **Step 4: Commit**

```bash
git add crates/tau-plugins/anthropic/tests/cassettes/complete_stop_reason_max_tokens.yaml crates/tau-plugins/anthropic/tests/cassettes/complete_stop_reason_stop_sequence.yaml crates/tau-plugins/anthropic/tests/complete.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "test(anthropic): cover MaxTokens + StopSequence stop reasons"
```

### Task 13: OpenAI 500 server error → `Provider`

**Files:**
- Create: `crates/tau-plugins/openai/tests/cassettes/complete_500_server_error.yaml`
- Test: `crates/tau-plugins/openai/tests/complete.rs`

- [ ] **Step 1: Create the cassette**

`crates/tau-plugins/openai/tests/cassettes/complete_500_server_error.yaml`:

```yaml
- request:
    method: POST
    uri: /v1/chat/completions
    headers:
      authorization: "<REDACTED>"
    body: |-
      placeholder
  response:
    status: 500
    headers:
      content-type: application/json
    body: |-
      {"error":{"message":"internal server error","type":"server_error"}}
```

- [ ] **Step 2: Write the failing test**

Append to `crates/tau-plugins/openai/tests/complete.rs`:

```rust
#[tokio::test]
async fn complete_500_maps_to_provider_error() {
    let server = cassette::replay("tests/cassettes/complete_500_server_error.yaml").await;
    let plugin =
        OpenAIPlugin::from_config(common::test_config_with_retry(server.uri().into(), 1, 0))
            .unwrap();
    let err = plugin.complete(common::sample_request()).await.unwrap_err();
    assert!(
        matches!(err, LlmError::Provider { .. }),
        "expected Provider, got {err:?}",
    );
}
```

Ensure `use tau_ports::LlmError;` is present at the top of the file (copy from the existing imports).

- [ ] **Step 3: Run it**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p openai complete_500_maps_to_provider_error`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/tau-plugins/openai/tests/cassettes/complete_500_server_error.yaml crates/tau-plugins/openai/tests/complete.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "test(openai): 500 maps to retryable Provider error"
```

### Task 14: OpenAI multiple tool calls in one stream

**Files:**
- Create: `crates/tau-plugins/openai/tests/cassettes/stream_multiple_tool_calls.yaml`
- Test: `crates/tau-plugins/openai/tests/streaming.rs`

- [ ] **Step 1: Create the cassette**

Model this on the existing `crates/tau-plugins/openai/tests/cassettes/stream_with_tool_use.yaml` (open it first to match the exact SSE framing the parser expects), but emit TWO tool calls at indices 0 and 1. `crates/tau-plugins/openai/tests/cassettes/stream_multiple_tool_calls.yaml`:

```yaml
- request:
    method: POST
    uri: /v1/chat/completions
    headers:
      authorization: "<REDACTED>"
    body: |-
      placeholder
  response:
    status: 200
    headers:
      content-type: text/event-stream
    body: |-
      data: {"choices":[{"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"alpha","arguments":""}}]},"index":0}]}

      data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]},"index":0}]}

      data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"beta","arguments":"{}"}}]},"index":0}]}

      data: {"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}],"usage":{"prompt_tokens":5,"completion_tokens":4,"total_tokens":9}}

      data: [DONE]
```

- [ ] **Step 2: Write the failing test**

Append to `crates/tau-plugins/openai/tests/streaming.rs` (mirror the drain-the-stream pattern already used there; `use futures::StreamExt;`/`tokio_stream` as the existing tests do):

```rust
#[tokio::test]
async fn stream_emits_two_tool_use_chunks() {
    let server = cassette::replay("tests/cassettes/stream_multiple_tool_calls.yaml").await;
    let plugin = OpenAIPlugin::from_config(common::test_config(server.uri().into())).unwrap();
    let mut stream = plugin.stream(common::sample_request()).await.unwrap();

    let mut tool_names = Vec::new();
    let mut saw_finish = false;
    while let Some(item) = stream.next().await {
        match item.unwrap() {
            tau_ports::CompletionChunk::ToolUse(tu) => tool_names.push(tu.name),
            tau_ports::CompletionChunk::Finish { stop_reason, .. } => {
                assert_eq!(stop_reason, tau_ports::StopReason::ToolUse);
                saw_finish = true;
            }
            tau_ports::CompletionChunk::Text { .. } => {}
        }
    }
    assert_eq!(tool_names, vec!["alpha".to_string(), "beta".to_string()]);
    assert!(saw_finish, "expected a Finish chunk");
}
```

If the streaming iteration idiom differs (e.g. a custom `next_chunk()`), copy it verbatim from the existing tests at the top of `streaming.rs`.

- [ ] **Step 3: Run it**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p openai stream_emits_two_tool_use_chunks`
Expected: PASS. If the SSE framing is rejected, diff your cassette against `stream_with_tool_use.yaml` for exact whitespace/blank-line framing.

- [ ] **Step 4: Commit**

```bash
git add crates/tau-plugins/openai/tests/cassettes/stream_multiple_tool_calls.yaml crates/tau-plugins/openai/tests/streaming.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "test(openai): cover multiple tool calls in one stream"
```

### Task 15: OpenAI mid-stream error event

**Files:**
- Create: `crates/tau-plugins/openai/tests/cassettes/stream_error_mid_stream.yaml`
- Test: `crates/tau-plugins/openai/tests/streaming.rs`

- [ ] **Step 1: Create the cassette**

`crates/tau-plugins/openai/tests/cassettes/stream_error_mid_stream.yaml` — emit one text delta, then an error event (model `stream_with_tool_use.yaml`'s framing):

```yaml
- request:
    method: POST
    uri: /v1/chat/completions
    headers:
      authorization: "<REDACTED>"
    body: |-
      placeholder
  response:
    status: 200
    headers:
      content-type: text/event-stream
    body: |-
      data: {"choices":[{"delta":{"role":"assistant","content":"Hel"},"index":0}]}

      data: {"error":{"message":"overloaded","type":"server_error"}}

      data: [DONE]
```

- [ ] **Step 2: Write the failing test**

```rust
#[tokio::test]
async fn stream_surfaces_mid_stream_error() {
    let server = cassette::replay("tests/cassettes/stream_error_mid_stream.yaml").await;
    let plugin = OpenAIPlugin::from_config(common::test_config(server.uri().into())).unwrap();
    let mut stream = plugin.stream(common::sample_request()).await.unwrap();

    let mut saw_error = false;
    while let Some(item) = stream.next().await {
        if item.is_err() {
            saw_error = true;
        }
    }
    assert!(saw_error, "expected an error item from the stream");
}
```

- [ ] **Step 3: Run it**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p openai stream_surfaces_mid_stream_error`
Expected: PASS. If the OpenAI stream parser ignores an unrecognized `error` event rather than surfacing it, that is a real finding — note it and adjust the assertion to document the actual behavior (and flag for follow-up), rather than forcing a false green.

- [ ] **Step 4: Commit**

```bash
git add crates/tau-plugins/openai/tests/cassettes/stream_error_mid_stream.yaml crates/tau-plugins/openai/tests/streaming.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "test(openai): cover mid-stream error event"
```

---

## Phase guard — cred-safety scan

### Task 16: enforce "no real secrets in cassettes" via CI

**Files:**
- Create: `crates/tau-plugin-test-support/src/secret_scan.rs`
- Modify: `crates/tau-plugin-test-support/src/lib.rs`
- Create: `crates/tau-plugins/anthropic/tests/no_real_secrets.rs`
- Create: `crates/tau-plugins/openai/tests/no_real_secrets.rs`
- Create: `crates/tau-plugins/ollama/tests/no_real_secrets.rs`

- [ ] **Step 1: Write the scanner + its unit tests**

Create `crates/tau-plugin-test-support/src/secret_scan.rs`:

```rust
//! Guard helper: fail tests if a cassette YAML appears to embed a real
//! API key (rather than one of the short, known test fakes).

use std::path::Path;

/// Tokens we intentionally use in cassettes/tests. Anything matching the
/// "long secret" heuristic that is NOT in this set is treated as a leak.
const ALLOWED: &[&str] = &["sk-ant-test", "sk-test", "sk-test-1234", "hosted-token-xyz"];

/// Returns true if `token` looks like a real provider key.
fn looks_like_real_secret(token: &str) -> bool {
    let t = token.trim().trim_matches('"');
    if ALLOWED.contains(&t) {
        return false;
    }
    let is_keyish = |s: &str| s.len() >= 16 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if let Some(rest) = t.strip_prefix("sk-ant-") {
        return is_keyish(rest);
    }
    if let Some(rest) = t.strip_prefix("sk-") {
        return is_keyish(rest);
    }
    false
}

/// Recursively scan `dir` for `*.yaml`/`*.yml` files and panic if any
/// line contains a token that looks like a real secret.
pub fn assert_no_real_secrets_in_dir(dir: &Path) {
    let mut offenders = Vec::new();
    scan(dir, &mut offenders);
    assert!(
        offenders.is_empty(),
        "possible real secret(s) found in cassettes:\n{}",
        offenders.join("\n"),
    );
}

fn scan(dir: &Path, offenders: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan(&path, offenders);
        } else if matches!(path.extension().and_then(|e| e.to_str()), Some("yaml" | "yml")) {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (i, line) in text.lines().enumerate() {
                for token in line.split(|c: char| c.is_whitespace() || matches!(c, ':' | ',' | '"' | '\'')) {
                    if looks_like_real_secret(token) {
                        offenders.push(format!("{}:{}: {token}", path.display(), i + 1));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::looks_like_real_secret;

    #[test]
    fn flags_real_looking_keys() {
        assert!(looks_like_real_secret("sk-ant-api03-AbCdEfGhIjKlMnOpQrStUv"));
        assert!(looks_like_real_secret("sk-proj-AbCdEfGhIjKlMnOpQrStUvWxYz"));
    }

    #[test]
    fn allows_known_fakes_and_non_keys() {
        assert!(!looks_like_real_secret("sk-ant-test"));
        assert!(!looks_like_real_secret("sk-test"));
        assert!(!looks_like_real_secret("hosted-token-xyz"));
        assert!(!looks_like_real_secret("<REDACTED>"));
        assert!(!looks_like_real_secret("claude-3-5-haiku-latest"));
        assert!(!looks_like_real_secret("2023-06-01"));
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/tau-plugin-test-support/src/lib.rs`, add after `pub mod cassette;`:

```rust
pub mod secret_scan;
```

- [ ] **Step 3: Run the scanner unit tests to verify they pass**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-plugin-test-support --lib secret_scan`
Expected: PASS.

- [ ] **Step 4: Add the per-plugin guard tests**

Create identical-shaped files (change nothing but the crate they live in). `crates/tau-plugins/anthropic/tests/no_real_secrets.rs`:

```rust
//! Guard: no cassette in this crate may embed a real API key.

#[test]
fn cassettes_contain_no_real_secrets() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    tau_plugin_test_support::secret_scan::assert_no_real_secrets_in_dir(&dir);
}
```

Create `crates/tau-plugins/openai/tests/no_real_secrets.rs` and `crates/tau-plugins/ollama/tests/no_real_secrets.rs` with the **same** content (the `env!("CARGO_MANIFEST_DIR")` resolves per-crate).

Confirm each plugin already has `tau-plugin-test-support` as a `[dev-dependencies]` entry (it does — the cassette tests use it). If a crate references it only via the re-export, no change is needed.

- [ ] **Step 5: Run the guard tests**

Run for each crate:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p anthropic cassettes_contain_no_real_secrets
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p openai cassettes_contain_no_real_secrets
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p ollama cassettes_contain_no_real_secrets
```
Expected: PASS (all existing cassettes use fakes/`<REDACTED>`).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-plugin-test-support/src/secret_scan.rs crates/tau-plugin-test-support/src/lib.rs crates/tau-plugins/anthropic/tests/no_real_secrets.rs crates/tau-plugins/openai/tests/no_real_secrets.rs crates/tau-plugins/ollama/tests/no_real_secrets.rs
git -c user.name="titouanlebocq" -c user.email="lebocq.titouan@gmail.com" \
  commit --no-verify -m "test(plugins): CI guard rejecting real secrets in cassettes"
```

---

## Final verification (before opening the PR)

- [ ] **Run each touched crate's full test suite:**

```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-plugin-test-support
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p anthropic
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p openai
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p ollama
```
Expected: all green.

- [ ] **fmt + clippy on touched crates:**

```
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-plugin-test-support -p anthropic -p openai -p ollama -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-plugin-test-support --all-targets
```
(Repeat clippy `-p anthropic/-p openai/-p ollama`.) Expected: clean.

- [ ] **Open the PR** to `main` (this branch `feat/cassette-coverage` is based on `main`). Per repo push rules, use `scripts/agent-push.sh` (or `lefthook run pre-push && git push --no-verify`); never bare `git push` from an agent runtime. Branch protection is `strict` — if `main` advances, `gh pr update-branch <PR#>`.

---

## Notes for the implementer

- **Why request-shape tests reuse happy-path cassettes:** the replayer (`cassette.rs`) serves responses by arrival order and never inspects the request, so to test what the plugin *sends* you just drive `complete()`/`stream()` against any valid cassette and assert on `server.received_requests()`. Only new *response* behaviors need new YAML.
- **Credential safety:** every assertion compares captured values to the hardcoded fakes (`sk-ant-test`, `sk-test`, none-for-ollama). No real key is ever used; all traffic is loopback to the in-process replayer. Task 16 makes this a CI-enforced invariant.
- **If an existing test fn name collides** with one of the new names, suffix the new one (`_v2`) — but first check it isn't already covering the gap.
- **Do not** modify the replayer's matching behavior or any plugin production code; this is a test-only effort (see spec Non-goals).
