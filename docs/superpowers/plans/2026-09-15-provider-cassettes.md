# Provider Cassettes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Real Anthropic/OpenAI/Ollama exchanges, recorded once with credentials stripped, replayed by the existing stub on every `just check`; a capability probe per listed model; `just live` to re-record.

**Architecture:** One scenario table per provider runs in two modes. Record mode points the driver at the stub in *forward* mode, which relays to the real provider and reports each exchange; the allowlist recorder writes `drivers/tests/cassettes/<target>/<scenario>.json`. Replay mode scripts the stub with the cassette's responses and asserts the contract expectation plus request equality. A guard test scans every cassette for secret shapes. No change to the driver crate's public surface.

**Tech Stack:** Rust integration tests in `drivers/tests/`, tokio, reqwest (already a crate dependency, reachable from tests), serde_json, nextest `quick` profile, `just`.

**Spec:** `docs/superpowers/specs/2026-09-15-provider-cassettes-design.md`

## Global Constraints

- Workspace lints: `unwrap`/`expect`/`panic`/indexing denied in `src/`; tests may `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` as every existing test file does.
- `HashMap`/`HashSet` denied: use `BTreeMap`/`BTreeSet`.
- Every test lives in `tests/`; quick profile kills a test at 5 s.
- Replay and guard tests run without network. Record tests are `#[ignore]` and gated on `TAU_RECORD=1`.
- Header allowlist, verbatim: `content-type`, `anthropic-version`, `anthropic-beta`. Known transport headers ignored, not recorded: `host`, `content-length`, `accept`, `user-agent`, `accept-encoding`, `connection`. Any other request header → record refuses to write.
- Cassette dir: `drivers/tests/cassettes/{anthropic,openai,ollama}/`.
- Full-matrix models: `claude-haiku-4-5-20251001`, `gpt-4.1-mini`, `qwen3:1.7b`; thinking scenarios on `claude-opus-5`.
- Prices (µUSD/token) for consumption arithmetic and the cost cap: Fable 5.x 10/50; Opus 4.x/5 5/25; Sonnet 5 2/10; Sonnet 4.x 3/15; Haiku 4.5 1/5; OpenAI placeholders 1/4 (the assertion is arithmetic, not a bill); Ollama 0/0.
- Conventional commits, one per task, never push credentials. Keys are read at run time from `security find-generic-password -s <NAME> -w` by `just live`; tests read `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` from env only.

---

## File map

| File | Responsibility |
|---|---|
| `drivers/tests/common/cassette.rs` (new) | `Cassette`, `Exchange`, `RecordedRequest`, `RecordedResponse`; allowlist redaction; load/save; `dir()`; secret scan |
| `drivers/tests/common/mod.rs` (modify) | `Answer::Forward` relay mode; `start_forwarding(base_url)` returning the stub plus an exchange receiver |
| `drivers/tests/common/scenario.rs` (new) | `Target`, `Expect`, `Scenario`, `Step`; `replay()` and `record()` runners |
| `drivers/tests/cassette_guard.rs` (new) | §5 guard over the directory; plant test |
| `drivers/tests/cassettes_anthropic.rs` (new) | Anthropic full matrix: replay tests + `#[ignore] record` |
| `drivers/tests/cassettes_openai.rs` (new) | OpenAI + Ollama full matrix |
| `drivers/tests/probes.rs` (new) | capability probes for every listed model, both providers; `MODELS.md` renderer; inventory drift |
| `drivers/tests/cassettes/**` (new) | the recorded JSON; `MODELS.md` |
| `justfile` (modify) | `live` recipe |

---

### Task 1: Cassette format, allowlist redaction, secret scan

**Files:**
- Create: `drivers/tests/common/cassette.rs`
- Modify: `drivers/tests/common/mod.rs` (add `pub(crate) mod cassette;` at top, after the `#![allow]`)
- Create: `drivers/tests/cassette_guard.rs`
- Create: `drivers/tests/cassettes/.gitkeep`

**Interfaces:**
- Produces:
  ```rust
  pub(crate) const ALLOWED_HEADERS: [&str; 3] = ["content-type", "anthropic-version", "anthropic-beta"];
  pub(crate) const TRANSPORT_HEADERS: [&str; 6] = ["host","content-length","accept","user-agent","accept-encoding","connection"];
  #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
  pub(crate) struct Cassette { pub v: u16, pub recorded_at: String, pub target: String, pub model: Option<String>, pub exchanges: Vec<Exchange> }
  pub(crate) struct Exchange { pub request: RecordedRequest, pub response: RecordedResponse }
  pub(crate) struct RecordedRequest { pub method: String, pub path: String, pub headers: BTreeMap<String,String>, pub body: Value }
  pub(crate) struct RecordedResponse { pub status: u16, pub body: Value }
  pub(crate) fn redact(captured: &Captured) -> Result<RecordedRequest, String>;   // Err(header name) on a non-allowlisted, non-transport header
  pub(crate) fn dir() -> PathBuf;                                                  // <crate>/tests/cassettes
  pub(crate) fn path(target: &str, scenario: &str) -> PathBuf;                     // dir()/target/scenario.json
  pub(crate) fn load(target: &str, scenario: &str) -> Option<Cassette>;
  pub(crate) fn save(c: &Cassette, scenario: &str);
  pub(crate) fn scan_for_secrets(text: &str) -> Option<String>;                    // Some(pattern name) on a hit
  pub(crate) fn all_files() -> Vec<PathBuf>;                                        // every *.json under dir(), sorted
  ```

- [ ] **Step 1: Write the failing guard tests**

`drivers/tests/cassette_guard.rs`:
```rust
//! Every committed cassette is free of secret shapes and of request headers
//! outside the allowlist, and names the directory it sits in.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::cassette::{self, Cassette, ALLOWED_HEADERS};

#[test]
fn no_cassette_carries_a_secret_shape_or_a_foreign_header() {
    for file in cassette::all_files() {
        let text = std::fs::read_to_string(&file).unwrap();
        if let Some(pattern) = cassette::scan_for_secrets(&text) {
            panic!("{}: matches secret pattern {pattern}", file.display());
        }
        let c: Cassette = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        assert_eq!(c.v, 1, "{}: unknown cassette version", file.display());
        let dir_name = file.parent().unwrap().file_name().unwrap().to_str().unwrap();
        assert_eq!(c.target, dir_name, "{}: target does not match its directory", file.display());
        for ex in &c.exchanges {
            for name in ex.request.headers.keys() {
                assert!(ALLOWED_HEADERS.contains(&name.as_str()), "{}: header {name} is not allowlisted", file.display());
            }
        }
    }
}

#[test]
fn the_scan_catches_every_planted_shape() {
    for planted in [
        "x-api-key: sk-ant-api03-abcdefghijklmnopqrstuvwxyz",
        "sk-proj-1234567890abcdefghijklmnop",
        "Authorization: Bearer abc.def.ghi",
        "\"sk-abcdefghijklmnopqrstuvwxyz0123\"",
    ] {
        assert!(cassette::scan_for_secrets(planted).is_some(), "{planted} slipped through");
    }
    assert!(cassette::scan_for_secrets("{\"id\":\"msg_01ABC\",\"skip\":\"sk-\"}").is_none(), "a bare sk- prefix is not a key");
}

#[test]
fn redaction_keeps_only_the_allowlist_and_refuses_strangers() {
    let ok = common::Captured {
        head: "POST /v1/messages HTTP/1.1\r\nhost: x\r\ncontent-type: application/json\r\nx-api-key: sk-ant-secret\r\nanthropic-version: 2023-06-01\r\ncontent-length: 2\r\naccept: */*\r\n\r\n".into(),
        body: b"{}".to_vec(),
    };
    let r = cassette::redact(&ok).unwrap();
    assert_eq!(r.method, "POST");
    assert_eq!(r.path, "/v1/messages");
    assert_eq!(r.headers.keys().cloned().collect::<Vec<_>>(), ["anthropic-version", "content-type"]);
    assert!(!serde_json::to_string(&r).unwrap().contains("secret"));

    let stranger = common::Captured {
        head: "POST /v1/messages HTTP/1.1\r\ncontent-type: application/json\r\nx-org-id: org_123\r\n\r\n".into(),
        body: b"{}".to_vec(),
    };
    assert_eq!(cassette::redact(&stranger).unwrap_err(), "x-org-id");
}
```

Note: `x-api-key` and `authorization` are *dropped silently* by `redact`, never copied and never an error: they are the two headers the drivers are known to send and the reason the allowlist exists. Add `pub(crate) const DROPPED_AUTH_HEADERS: [&str; 2] = ["x-api-key", "authorization"];`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p tau-drivers --all-features --test cassette_guard`
Expected: compile error, `common::cassette` not found.

- [ ] **Step 3: Implement `common/cassette.rs`**

```rust
//! The cassette: one real exchange sequence with a provider, credentials
//! never read, committed and replayed by the stub.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Captured;

pub(crate) const ALLOWED_HEADERS: [&str; 3] = ["content-type", "anthropic-version", "anthropic-beta"];
pub(crate) const TRANSPORT_HEADERS: [&str; 6] =
    ["host", "content-length", "accept", "user-agent", "accept-encoding", "connection"];
pub(crate) const DROPPED_AUTH_HEADERS: [&str; 2] = ["x-api-key", "authorization"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Cassette {
    pub(crate) v: u16,
    pub(crate) recorded_at: String,
    pub(crate) target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    pub(crate) exchanges: Vec<Exchange>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Exchange {
    pub(crate) request: RecordedRequest,
    pub(crate) response: RecordedResponse,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct RecordedRequest {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) body: Value,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct RecordedResponse {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

/// Copies the request by allowlist. `Err(name)` names the first header that
/// is neither allowlisted, transport, nor a known auth header.
pub(crate) fn redact(captured: &Captured) -> Result<RecordedRequest, String> {
    let mut lines = captured.head.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or("").to_owned();
    let path = parts.next().unwrap_or("").to_owned();
    let mut headers = BTreeMap::new();
    for line in lines {
        let Some((k, v)) = line.split_once(':') else { continue };
        let name = k.trim().to_ascii_lowercase();
        if ALLOWED_HEADERS.contains(&name.as_str()) {
            headers.insert(name, v.trim().to_owned());
        } else if TRANSPORT_HEADERS.contains(&name.as_str()) || DROPPED_AUTH_HEADERS.contains(&name.as_str()) {
            continue;
        } else {
            return Err(name);
        }
    }
    let body = serde_json::from_slice(&captured.body).unwrap_or(Value::Null);
    Ok(RecordedRequest { method, path, headers, body })
}

pub(crate) fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("cassettes")
}

pub(crate) fn path(target: &str, scenario: &str) -> PathBuf {
    dir().join(target).join(format!("{scenario}.json"))
}

pub(crate) fn load(target: &str, scenario: &str) -> Option<Cassette> {
    let text = std::fs::read_to_string(path(target, scenario)).ok()?;
    Some(serde_json::from_str(&text).expect("cassette parses"))
}

pub(crate) fn save(c: &Cassette, scenario: &str) {
    let p = path(&c.target, scenario);
    std::fs::create_dir_all(p.parent().expect("has parent")).expect("mkdir");
    let text = serde_json::to_string_pretty(c).expect("serializes");
    if let Some(pattern) = scan_for_secrets(&text) {
        panic!("refusing to write {}: matches {pattern}", p.display());
    }
    std::fs::write(&p, text + "\n").expect("write cassette");
}

/// `Some(name)` if `text` contains a key-shaped string.
pub(crate) fn scan_for_secrets(text: &str) -> Option<String> {
    if text.contains("sk-ant-") { return Some("sk-ant-".into()); }
    if text.contains("sk-proj-") { return Some("sk-proj-".into()); }
    if text.contains("Bearer ") { return Some("Bearer ".into()); }
    // `sk-` followed by 20+ key characters.
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(pos) = text.get(i..).and_then(|t| t.find("sk-")) {
        let start = i + pos + 3;
        let run = bytes.get(start..).map_or(0, |rest| {
            rest.iter().take_while(|b| b.is_ascii_alphanumeric() || **b == b'_' || **b == b'-').count()
        });
        if run >= 20 { return Some("sk-<20+ chars>".into()); }
        i = start;
    }
    None
}

pub(crate) fn all_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(targets) = std::fs::read_dir(dir()) else { return out };
    for target in targets.flatten() {
        let Ok(files) = std::fs::read_dir(target.path()) else { continue };
        for f in files.flatten() {
            if f.path().extension().is_some_and(|e| e == "json") {
                out.push(f.path());
            }
        }
    }
    out.sort();
    out
}
```

Make `Captured`'s fields constructible from the guard test: they are already `pub(crate)`; the struct itself is `pub(crate)`. Add `pub(crate) mod cassette;` to `common/mod.rs` right below the `#![allow(...)]` line.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo nextest run -p tau-drivers --all-features --test cassette_guard`
Expected: 3 passed (directory empty except `.gitkeep`, so the first test iterates nothing).

- [ ] **Step 5: Commit**

```bash
git add drivers/tests/common/cassette.rs drivers/tests/common/mod.rs drivers/tests/cassette_guard.rs drivers/tests/cassettes/.gitkeep
git commit -m "test(drivers): cassette format, allowlist redaction, secret guard (#100)"
```

---

### Task 2: Stub forward mode

**Files:**
- Modify: `drivers/tests/common/mod.rs` (`Answer` enum, `read_request`, new `start_forwarding`)
- Test: add to `drivers/tests/cassette_guard.rs` a forwarding test (keeps the number of test binaries down; rename the file's doc comment to "guard and stub plumbing")

**Interfaces:**
- Produces:
  ```rust
  pub(crate) struct Relayed { pub(crate) request: Captured, pub(crate) status: u16, pub(crate) body: Vec<u8> }
  pub(crate) async fn start_forwarding(base_url: String) -> (Stub, mpsc::UnboundedReceiver<Relayed>);
  ```
  `Answer::Forward { base_url: String, relayed: mpsc::UnboundedSender<Relayed> }`.

- [ ] **Step 1: Write the failing test**

Append to `cassette_guard.rs`:
```rust
#[tokio::test]
async fn forward_mode_relays_headers_body_status_and_reports_the_exchange() {
    // The "provider": a plain stub answering 418 with a fixed body.
    let mut upstream = common::start(common::Answer::Json(418, r#"{"ok":true}"#.into())).await;
    let (relay, mut relayed) = common::start_forwarding(upstream.base_url.clone()).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/v1/messages", relay.base_url))
        .header("content-type", "application/json")
        .header("x-api-key", "sk-ant-not-a-real-key")
        .body(r#"{"hello":"world"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 418);
    assert_eq!(resp.text().await.unwrap(), r#"{"ok":true}"#);

    let seen_upstream = upstream.captured.recv().await.unwrap();
    assert_eq!(seen_upstream.path(), "/v1/messages");
    assert_eq!(seen_upstream.header("x-api-key"), Some("sk-ant-not-a-real-key"));
    assert_eq!(seen_upstream.json(), serde_json::json!({"hello":"world"}));

    let r = relayed.recv().await.unwrap();
    assert_eq!(r.status, 418);
    assert_eq!(r.body, br#"{"ok":true}"#);
    assert_eq!(r.request.path(), "/v1/messages");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p tau-drivers --all-features --test cassette_guard forward_mode`
Expected: compile error, `start_forwarding` not found.

- [ ] **Step 3: Implement**

In `common/mod.rs`:

```rust
/// One request the forward-mode stub relayed, with what came back.
#[derive(Debug)]
pub(crate) struct Relayed {
    pub(crate) request: Captured,
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
}
```
Add a variant to `Answer`:
```rust
    /// Relay the request to `base_url` over real HTTPS, answer the client
    /// with what came back, and report the pair on `relayed`.
    Forward {
        base_url: String,
        relayed: mpsc::UnboundedSender<Relayed>,
    },
```
Add the constructor:
```rust
/// A stub that relays every request to `base_url`. Record mode.
pub(crate) async fn start_forwarding(base_url: String) -> (Stub, mpsc::UnboundedReceiver<Relayed>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let stub = serve(Arc::new(vec![(None, Answer::Forward { base_url, relayed: tx })])).await;
    (stub, rx)
}
```
In `read_request`, the `match answer` gains an arm (before `Answer::Script(_) => unreachable!`):
```rust
        Answer::Forward { base_url, relayed } => {
            let url = format!("{}{}", base_url.trim_end_matches('/'), captured.path());
            let mut req = reqwest::Client::new().post(url);
            for line in captured.head.lines().skip(1) {
                let Some((k, v)) = line.split_once(':') else { continue };
                let name = k.trim().to_ascii_lowercase();
                if matches!(name.as_str(), "host" | "content-length" | "connection" | "accept-encoding") {
                    continue;
                }
                req = req.header(name, v.trim());
            }
            let (status, body) = match req.body(captured.body.clone()).send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let body = resp.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
                    (status, body)
                }
                Err(e) => (
                    502,
                    format!(r#"{{"type":"error","error":{{"type":"relay","message":"{e}"}}}}"#).into_bytes(),
                ),
            };
            let _ = relayed.send(Relayed { request: captured.clone(), status, body: body.clone() });
            let response = format!(
                "HTTP/1.1 {status} Relayed\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.write_all(&body).await;
            let _ = stream.shutdown().await;
        }
```
`captured` is moved into `tx.send(captured)` earlier in the function; change that line to `let _ = tx.send(captured.clone());` (`Captured` is `Clone`). The `Answer::Json` arm's `reason` match needs no change; the relay writes its own status line.

The drivers strip nothing on the way out, so an upstream `content-encoding: gzip` would break the driver's JSON parse. `reqwest` without the `gzip` feature does not advertise `accept-encoding`, and we drop the client's if any, so bodies arrive identity-encoded.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo nextest run -p tau-drivers --all-features --test cassette_guard`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add drivers/tests/common/mod.rs drivers/tests/cassette_guard.rs
git commit -m "test(drivers): stub forward mode for cassette recording (#100)"
```

---

### Task 3: Scenario runner (replay and record)

**Files:**
- Create: `drivers/tests/common/scenario.rs`
- Modify: `drivers/tests/common/mod.rs` (add `pub(crate) mod scenario;`)
- Test: `drivers/tests/cassette_guard.rs` (runner test against a synthetic cassette)

**Interfaces:**
- Consumes: `cassette::{Cassette, Exchange, load, save, redact}`, `common::{start, script, start_forwarding, Stub}`.
- Produces:
  ```rust
  #[derive(Clone, Copy, PartialEq, Eq, Debug)]
  pub(crate) enum Target { Anthropic, OpenAi, Ollama }
  impl Target { pub(crate) fn dir_name(self) -> &'static str; pub(crate) fn live_base_url(self) -> String; }

  pub(crate) enum Expect {
      Stop(StopReason),                       // stop == this, usage.in>0, usage.out>0, consumed Tokens == in+out
      ToolCall { name: &'static str, min: usize },  // stop ToolCall; >= min ToolCall blocks all named `name`
      ProviderError,                          // stop Error(kind Provider); consumed has no Tokens
      Policy,                                 // either a non-error stop or a Provider error; never Unsupported/Transport/OverCeiling
  }
  pub(crate) fn check(expect: &Expect, reply: &ModelReply, consumed: &Consumption);

  /// Builds the request for step `i` from the replies so far.
  pub(crate) type Build = Box<dyn Fn(&[ModelReply]) -> ModelRequest + Send + Sync>;
  pub(crate) struct Step { pub build: Build, pub expect: Expect }
  pub(crate) struct Scenario { pub name: &'static str, pub target: Target, pub model: &'static str, pub steps: Vec<Step> }

  /// `make(base_url) -> Box<dyn Driver>`: the caller picks the config; base_url is the stub's.
  pub(crate) type Make = Box<dyn Fn(&str) -> Box<dyn Driver> + Send + Sync>;

  pub(crate) async fn replay(s: &Scenario, make: Make);   // panics with the scenario name on any mismatch
  pub(crate) async fn record(s: &Scenario, make: Make) -> Consumption;  // writes the cassette; returns summed consumption
  pub(crate) fn record_enabled() -> bool;                 // TAU_RECORD == "1"
  pub(crate) async fn call(driver: &dyn Driver, corr: u64, req: &ModelRequest) -> (ModelReply, Consumption);
  ```

- [ ] **Step 1: Write the failing test**

Append to `cassette_guard.rs`:
```rust
use common::scenario::{self, Expect, Scenario, Step, Target};
use tau_drivers::model::anthropic::{AnthropicConfig, AnthropicDriver, ApiKey};
use tau_kernel::bridge::{Content, Message, ModelRequest, Role, StopReason, VERSION};

fn hello() -> ModelRequest {
    ModelRequest {
        v: VERSION,
        system: None,
        messages: vec![Message { role: Role::User, content: vec![Content::Text { text: "hi".into() }] }],
        tools: vec![],
        max_tokens: 64,
        sampling: None,
    }
}

#[tokio::test]
async fn replay_runs_a_synthetic_cassette_and_checks_request_equality() {
    // Build a cassette by hand from the ADR fixture the contract suite already trusts.
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/anthropic/response-tool-use.json")).unwrap();
    let make = |base: &str| -> Box<dyn tau_kernel::driver::Driver> {
        let mut cfg = AnthropicConfig::new("claude-haiku-4-5-20251001", ApiKey::new("sk-test"), 8_000, 64, 1, 5);
        cfg.base_url = base.to_owned();
        Box::new(AnthropicDriver::new(cfg).unwrap())
    };
    // What the driver will send for `hello()` — capture it once against a plain stub.
    let mut probe = common::start(common::Answer::Json(200, fixture.to_string())).await;
    let d = make(&probe.base_url);
    let _ = scenario::call(d.as_ref(), 1, &hello()).await;
    let sent = probe.captured.recv().await.unwrap();

    let c = common::cassette::Cassette {
        v: 1,
        recorded_at: "2026-09-15".into(),
        target: "synthetic".into(),
        model: Some("claude-haiku-4-5-20251001".into()),
        exchanges: vec![common::cassette::Exchange {
            request: common::cassette::redact(&sent).unwrap(),
            response: common::cassette::RecordedResponse { status: 200, body: fixture },
        }],
    };
    common::cassette::save(&c, "synthetic_tool_use");

    let s = Scenario {
        name: "synthetic_tool_use",
        target: Target::Anthropic,
        model: "claude-haiku-4-5-20251001",
        steps: vec![Step { build: Box::new(|_| hello()), expect: Expect::ToolCall { name: "store", min: 1 } }],
    };
    // Point replay at the synthetic directory by overriding the target dir name.
    scenario::replay_from(&s, "synthetic", Box::new(make)).await;

    std::fs::remove_dir_all(common::cassette::dir().join("synthetic")).unwrap();
}
```
The fixture's tool is named `store` (see `drivers/tests/fixtures/anthropic/response-tool-use.json`); confirm the name when writing the test and adjust the literal if it differs. `replay_from(s, dir_name, make)` is `replay` with the directory name overridden; `replay` calls `replay_from(s, s.target.dir_name(), make)`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p tau-drivers --all-features --test cassette_guard replay_runs`
Expected: compile error, `common::scenario` not found.

- [ ] **Step 3: Implement `common/scenario.rs`**

```rust
//! One scenario table, two modes: replay the cassette through the stub, or
//! record it through the relay against the real provider.

use std::future::Future;
use std::pin::Pin;

use serde_json::Value;
use tau_kernel::abi::{AgentId, Corr, DimKey};
use tau_kernel::bridge::{Content, ErrorKind, ModelReply, ModelRequest, StopReason};
use tau_kernel::driver::{Consumption, Driver};
use tau_kernel::kernel::Delivery;

use super::cassette::{self, Cassette, Exchange, RecordedResponse};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Target { Anthropic, OpenAi, Ollama }

impl Target {
    pub(crate) fn dir_name(self) -> &'static str {
        match self { Self::Anthropic => "anthropic", Self::OpenAi => "openai", Self::Ollama => "ollama" }
    }
    pub(crate) fn live_base_url(self) -> String {
        match self {
            Self::Anthropic => "https://api.anthropic.com".into(),
            Self::OpenAi => "https://api.openai.com".into(),
            Self::Ollama => std::env::var("TAU_OLLAMA_BASE_URL").unwrap_or_else(|_| "http://localhost:11434".into()),
        }
    }
}

pub(crate) enum Expect {
    Stop(StopReason),
    ToolCall { name: &'static str, min: usize },
    ProviderError,
    Policy,
}

pub(crate) fn check(expect: &Expect, reply: &ModelReply, consumed: &Consumption) {
    let billed = || {
        assert!(reply.usage.input_tokens > 0 && reply.usage.output_tokens > 0, "usage is real: {:?}", reply.usage);
        assert_eq!(consumed.get(&DimKey::Tokens), Some(reply.usage.input_tokens + reply.usage.output_tokens));
    };
    match expect {
        Expect::Stop(stop) => { assert_eq!(&reply.stop, stop, "{reply:#?}"); billed(); }
        Expect::ToolCall { name, min } => {
            assert_eq!(reply.stop, StopReason::ToolCall, "{reply:#?}");
            let calls: Vec<_> = reply.content.iter().filter(|c| matches!(c, Content::ToolCall { .. })).collect();
            assert!(calls.len() >= *min, "wanted >= {min} tool calls, got {}: {reply:#?}", calls.len());
            for c in calls {
                if let Content::ToolCall { name: n, .. } = c { assert_eq!(n, name); }
            }
            billed();
        }
        Expect::ProviderError => {
            assert!(matches!(&reply.stop, StopReason::Error(e) if e.kind == ErrorKind::Provider), "{reply:#?}");
            assert_eq!(consumed.get(&DimKey::Tokens), None, "a rejection bills nothing");
        }
        Expect::Policy => match &reply.stop {
            StopReason::Error(e) => assert_eq!(e.kind, ErrorKind::Provider, "{reply:#?}"),
            _ => billed(),
        },
    }
}

pub(crate) type Build = Box<dyn Fn(&[ModelReply]) -> ModelRequest + Send + Sync>;
pub(crate) struct Step { pub(crate) build: Build, pub(crate) expect: Expect }
pub(crate) struct Scenario {
    pub(crate) name: &'static str,
    pub(crate) target: Target,
    pub(crate) model: &'static str,
    pub(crate) steps: Vec<Step>,
}
pub(crate) type Make = Box<dyn Fn(&str) -> Box<dyn Driver> + Send + Sync>;

pub(crate) async fn call(driver: &dyn Driver, corr: u64, req: &ModelRequest) -> (ModelReply, Consumption) {
    let (bytes, consumed) = driver
        .handle(Delivery { corr: Corr::new(corr), from: AgentId::new(1), payload: serde_json::to_vec(req).unwrap() })
        .await;
    (serde_json::from_slice(&bytes).unwrap(), consumed)
}

pub(crate) fn record_enabled() -> bool {
    std::env::var("TAU_RECORD").as_deref() == Ok("1")
}

async fn run_steps(s: &Scenario, driver: &dyn Driver) -> Vec<ModelReply> {
    let mut replies = Vec::new();
    for (i, step) in s.steps.iter().enumerate() {
        let req = (step.build)(&replies);
        let (reply, consumed) = call(driver, i as u64 + 1, &req).await;
        check(&step.expect, &reply, &consumed);
        replies.push(reply);
    }
    replies
}

pub(crate) async fn replay(s: &Scenario, make: Make) {
    replay_from(s, s.target.dir_name(), make).await;
}

pub(crate) async fn replay_from(s: &Scenario, dir_name: &str, make: Make) {
    let c = cassette::load(dir_name, s.name)
        .unwrap_or_else(|| panic!("{}/{}: no cassette; run `just live record`", dir_name, s.name));
    let answers = c.exchanges.iter().map(|e| (e.response.status, e.response.body.to_string()));
    let mut stub = super::start(super::script(answers)).await;
    let driver = make(&stub.base_url);
    let _ = run_steps(s, driver.as_ref()).await;
    for (i, ex) in c.exchanges.iter().enumerate() {
        let sent = stub.captured.try_recv().unwrap_or_else(|_| panic!("{}: fewer requests than recorded (missing #{i})", s.name));
        assert_eq!(sent.path(), ex.request.path, "{}: exchange #{i} path", s.name);
        assert_eq!(sent.json(), ex.request.body, "{}: exchange #{i} body differs from the recording", s.name);
    }
    assert!(stub.captured.try_recv().is_err(), "{}: more requests than recorded", s.name);
}

pub(crate) async fn record(s: &Scenario, make: Make) -> Consumption {
    let (stub, mut relayed) = super::start_forwarding(s.target.live_base_url()).await;
    let driver = make(&stub.base_url);
    let mut total = Consumption::default();
    let mut replies = Vec::new();
    for (i, step) in s.steps.iter().enumerate() {
        let req = (step.build)(&replies);
        let (reply, consumed) = call(driver.as_ref(), i as u64 + 1, &req).await;
        check(&step.expect, &reply, &consumed);
        total = total.saturating_add(&consumed);
        replies.push(reply);
    }
    let mut exchanges = Vec::new();
    while let Ok(r) = relayed.try_recv() {
        let request = cassette::redact(&r.request)
            .unwrap_or_else(|h| panic!("{}: refusing to record header {h}", s.name));
        let body: Value = serde_json::from_slice(&r.body)
            .unwrap_or_else(|e| panic!("{}: provider body is not JSON: {e}", s.name));
        exchanges.push(Exchange { request, response: RecordedResponse { status: r.status, body } });
    }
    assert!(!exchanges.is_empty(), "{}: nothing was relayed", s.name);
    let model = replies.iter().rev().find_map(|r| r.model.clone()).or_else(|| Some(s.model.to_owned()));
    let c = Cassette { v: 1, recorded_at: today(), target: s.target.dir_name().to_owned(), model, exchanges };
    cassette::save(&c, s.name);
    eprintln!("recorded {}/{}", s.target.dir_name(), s.name);
    total
}

fn today() -> String {
    // Date only, from the system clock via `std::process` to stay clear of
    // the `SystemTime::now` lint: the recorder runs by hand, never in the reducer.
    let out = std::process::Command::new("date").arg("+%Y-%m-%d").output().expect("date");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}
```
If `Consumption` has no `saturating_add`/`Default`, look in `kernel/src/driver.rs` for the merge helper the kernel uses to settle consumption and use that; if none exists, sum the two dims by hand with `Consumption::from_dims([(DimKey::Tokens, a + b), (DimKey::CostMicroUsd, c + d)])`.

The `clippy.toml` `SystemTime::now` ban applies to the whole workspace including tests; hence the `date` subprocess.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo nextest run -p tau-drivers --all-features --test cassette_guard`
Expected: 5 passed. The synthetic directory is removed at the end of the test; confirm `git status` shows no stray `tests/cassettes/synthetic/`.

- [ ] **Step 5: Commit**

```bash
git add drivers/tests/common/scenario.rs drivers/tests/common/mod.rs drivers/tests/cassette_guard.rs
git commit -m "test(drivers): scenario runner — replay from cassette, record through the relay (#100)"
```

---

### Task 4: Anthropic full matrix

**Files:**
- Create: `drivers/tests/cassettes_anthropic.rs`

**Interfaces:**
- Consumes: `scenario::{Scenario, Step, Expect, Target, replay, record, record_enabled, Make}`.
- Produces: `fn scenarios() -> Vec<Scenario>` (names listed below), `fn make(model, thinking, sampling, estimate) -> Make`, request builders `text(prompt, max)`, `with_calc(req)`, `calc_result(prev, text)`.

Scenario names and shapes (each a `Scenario`):

| name | model | steps | expect |
|---|---|---|---|
| `text_end_turn` | haiku | text("Reply with the single word: pong.", 64) | Stop(EndTurn) |
| `tool_call` | haiku | with_calc(text("What is 17*23? Use the calculator tool.", 256)) | ToolCall{calculator,1} |
| `tool_result_round_trip` | haiku | step 1 as `tool_call`; step 2 = calc_result(prev, "391") | step 2: Stop(EndTurn) |
| `parallel_tool_calls` | haiku | with_calc(text("Compute 2+2 and 3+3 as two separate calculator calls in one turn.", 512)) | ToolCall{calculator, min 2} |
| `max_tokens_stop` | haiku | text("Write a 500-word essay about rivers.", 16) | Stop(MaxTokens) |
| `stop_sequence_stop` | haiku | text("Count from 1 to 10, one number per line.", 128) with sampling.stop_sequences=["5"] | Stop(StopSequence) |
| `sampling_accepted` | haiku, SamplingMode::Accepted | text("pong?", 32) with temperature 0.2 | Stop(EndTurn) |
| `thinking_on_replayed_second_turn` | opus-5 | step 1: with_calc(text("What is 17*23? Use the calculator tool.", 2048)); step 2: calc_result(prev,"391") including every `Content::Thinking` block from prev in the assistant turn | step 1 ToolCall; step 2 Stop(EndTurn) |
| `thinking_disabled` | opus-5, ThinkingMode::Disabled | text("pong?", 64) | Stop(EndTurn) |
| `count_tokens_estimate` | haiku, InputEstimate::CountTokens | text("pong?", 32) | Stop(EndTurn); cassette has 2 exchanges |
| `bad_request_400` | haiku | messages: [] , max 32 | ProviderError |
| `bad_key_401` | haiku, ApiKey::new("sk-ant-invalid") | text("pong?", 32) | ProviderError |
| `unknown_model_404` | model "claude-does-not-exist" | text("pong?", 32) | ProviderError |

For `bad_key_401` the key literal `sk-ant-invalid` is 14 chars after `sk-`, below the guard's 20-char run and not containing `sk-ant-`... it does contain `sk-ant-`. Use `ApiKey::new("invalid-key")` instead; the header is never recorded anyway.

- [ ] **Step 1: Write the file with the replay tests (they fail: no cassettes yet)**

```rust
//! The Anthropic driver against recorded provider exchanges: the full
//! ADR-0006 matrix on Haiku 4.5, thinking on Opus 5, replayed by the stub.
//! `TAU_RECORD=1` re-records through the relay.

#![cfg(feature = "anthropic")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::scenario::{self, Expect, Make, Scenario, Step, Target};
use serde_json::json;
use tau_drivers::model::anthropic::{
    AnthropicConfig, AnthropicDriver, ApiKey, InputEstimate, SamplingMode, ThinkingMode, API_KEY_ENV,
};
use tau_kernel::abi::Name;
use tau_kernel::bridge::{Content, Message, ModelReply, ModelRequest, Role, Sampling, StopReason, ToolDef, VERSION};
use tau_kernel::driver::Driver;

const HAIKU: &str = "claude-haiku-4-5-20251001";
const OPUS: &str = "claude-opus-5";

fn key() -> ApiKey {
    if scenario::record_enabled() { ApiKey::from_env(API_KEY_ENV).expect("ANTHROPIC_API_KEY") } else { ApiKey::new("replay") }
}

fn price(model: &str) -> (u64, u64) {
    if model.starts_with("claude-opus") { (5, 25) } else { (1, 5) }
}

fn make_with(model: &'static str, key: ApiKey, thinking: ThinkingMode, sampling: SamplingMode, estimate: InputEstimate) -> Make {
    Box::new(move |base: &str| {
        let (i, o) = price(model);
        let mut cfg = AnthropicConfig::new(model, key.clone(), 16_000, 4_096, i, o);
        cfg.base_url = base.to_owned();
        cfg.thinking = thinking;
        cfg.sampling = sampling;
        cfg.estimate = estimate;
        Box::new(AnthropicDriver::new(cfg).unwrap()) as Box<dyn Driver>
    })
}

fn make(model: &'static str) -> Make {
    make_with(model, key(), ThinkingMode::default(), SamplingMode::default(), InputEstimate::default())
}

fn text(prompt: &str, max_tokens: u32) -> ModelRequest {
    ModelRequest {
        v: VERSION,
        system: Some("You are terse.".into()),
        messages: vec![Message { role: Role::User, content: vec![Content::Text { text: prompt.into() }] }],
        tools: vec![],
        max_tokens,
        sampling: None,
    }
}

fn calculator() -> ToolDef {
    ToolDef {
        name: Name::new("calculator").unwrap(),
        description: "Evaluates an arithmetic expression.".into(),
        input_schema: json!({"type":"object","properties":{"expression":{"type":"string"}},"required":["expression"],"additionalProperties":false}),
    }
}

fn with_calc(mut req: ModelRequest) -> ModelRequest {
    req.tools = vec![calculator()];
    req
}

/// The second turn: the assistant's reply (thinking + tool calls, verbatim)
/// then one `tool_result` per call.
fn calc_result(first: &ModelRequest, prev: &ModelReply, result: &str) -> ModelRequest {
    let mut req = first.clone();
    req.messages.push(Message { role: Role::Assistant, content: prev.content.clone() });
    let results = prev.content.iter().filter_map(|c| match c {
        Content::ToolCall { id, .. } => Some(Content::ToolResult { call_id: id.clone(), content: result.into(), is_error: false, error_kind: None }),
        _ => None,
    }).collect();
    req.messages.push(Message { role: Role::User, content: results });
    req
}

fn one(name: &'static str, model: &'static str, build: impl Fn() -> ModelRequest + Send + Sync + 'static, expect: Expect) -> Scenario {
    Scenario { name, target: Target::Anthropic, model, steps: vec![Step { build: Box::new(move |_| build()), expect }] }
}

fn round_trip(name: &'static str, model: &'static str, max_tokens: u32) -> Scenario {
    let first = move || with_calc(text("What is 17*23? Use the calculator tool.", max_tokens));
    Scenario {
        name, target: Target::Anthropic, model,
        steps: vec![
            Step { build: Box::new(move |_| first()), expect: Expect::ToolCall { name: "calculator", min: 1 } },
            Step { build: Box::new(move |prev| calc_result(&first(), prev.last().unwrap(), "391")), expect: Expect::Stop(StopReason::EndTurn) },
        ],
    }
}

fn scenarios() -> Vec<(Scenario, Make)> {
    let mut stop5 = text("Count from 1 to 10, one number per line.", 128);
    stop5.sampling = Some(Sampling { stop_sequences: vec!["5".into()], ..Sampling::default() });
    let mut warm = text("pong?", 32);
    warm.sampling = Some(Sampling { temperature: Some(0.2), ..Sampling::default() });
    let empty = ModelRequest { messages: vec![], ..text("", 32) };

    vec![
        (one("text_end_turn", HAIKU, || text("Reply with the single word: pong.", 64), Expect::Stop(StopReason::EndTurn)), make(HAIKU)),
        (one("tool_call", HAIKU, || with_calc(text("What is 17*23? Use the calculator tool.", 256)), Expect::ToolCall { name: "calculator", min: 1 }), make(HAIKU)),
        (round_trip("tool_result_round_trip", HAIKU, 256), make(HAIKU)),
        (one("parallel_tool_calls", HAIKU, || with_calc(text("Compute 2+2 and 3+3 as two separate calculator calls in one turn.", 512)), Expect::ToolCall { name: "calculator", min: 2 }), make(HAIKU)),
        (one("max_tokens_stop", HAIKU, || text("Write a 500-word essay about rivers.", 16), Expect::Stop(StopReason::MaxTokens)), make(HAIKU)),
        (one("stop_sequence_stop", HAIKU, move || stop5.clone(), Expect::Stop(StopReason::StopSequence)), make(HAIKU)),
        (one("sampling_accepted", HAIKU, move || warm.clone(), Expect::Stop(StopReason::EndTurn)), make_with(HAIKU, key(), ThinkingMode::default(), SamplingMode::Accepted, InputEstimate::default())),
        (round_trip("thinking_on_replayed_second_turn", OPUS, 2048), make(OPUS)),
        (one("thinking_disabled", OPUS, || text("pong?", 64), Expect::Stop(StopReason::EndTurn)), make_with(OPUS, key(), ThinkingMode::Disabled, SamplingMode::default(), InputEstimate::default())),
        (one("count_tokens_estimate", HAIKU, || text("pong?", 32), Expect::Stop(StopReason::EndTurn)), make_with(HAIKU, key(), ThinkingMode::default(), SamplingMode::default(), InputEstimate::CountTokens)),
        (one("bad_request_400", HAIKU, move || empty.clone(), Expect::ProviderError), make(HAIKU)),
        (one("bad_key_401", HAIKU, || text("pong?", 32), Expect::ProviderError), make_with(HAIKU, ApiKey::new("invalid-key"), ThinkingMode::default(), SamplingMode::default(), InputEstimate::default())),
        (one("unknown_model_404", "claude-does-not-exist", || text("pong?", 32), Expect::ProviderError), make("claude-does-not-exist")),
    ]
}

fn find(name: &str) -> (Scenario, Make) {
    scenarios().into_iter().find(|(s, _)| s.name == name).unwrap_or_else(|| panic!("no scenario {name}"))
}

macro_rules! replay_tests {
    ($($name:ident),* $(,)?) => { $(
        #[tokio::test]
        async fn $name() { let (s, make) = find(stringify!($name)); scenario::replay(&s, make).await; }
    )* };
}

replay_tests! {
    text_end_turn, tool_call, tool_result_round_trip, parallel_tool_calls, max_tokens_stop,
    stop_sequence_stop, sampling_accepted, thinking_on_replayed_second_turn, thinking_disabled,
    count_tokens_estimate, bad_request_400, bad_key_401, unknown_model_404,
}

#[tokio::test]
#[ignore = "TAU_RECORD=1 and ANTHROPIC_API_KEY; costs money"]
async fn record_all() {
    if !scenario::record_enabled() { eprintln!("TAU_RECORD is not 1; skipping"); return; }
    let mut total = 0;
    for (s, make) in scenarios() {
        let c = scenario::record(&s, make).await;
        total += c.get(&tau_kernel::abi::DimKey::CostMicroUsd).unwrap_or(0);
    }
    eprintln!("anthropic matrix recorded; {total} µUSD");
}
```
The `Sampling` type derives `Default` (checked: `#[derive(Clone, Debug, Default, ...)]` in `kernel/src/bridge/request.rs`). `ModelRequest` also derives `Default`? Check; if not, build `empty` by setting `.messages = vec![]` on a `text("", 32)` value.

The `thinking_on_replayed_second_turn` assertion that the second request carries the sealed blocks is implicit: replay asserts the second request body equals the recorded one, and the recorded one is what Opus 5 accepted with the blocks in place. Add an explicit belt in `calc_result`'s caller: after `record`/`replay`, nothing more is needed because `check` on step 2 would have failed on a 400 at record time.

- [ ] **Step 2: Run to verify replay fails for the right reason**

Run: `cargo nextest run -p tau-drivers --all-features --test cassettes_anthropic text_end_turn`
Expected: FAIL with `anthropic/text_end_turn: no cassette; run `just live record``.

- [ ] **Step 3: Record**

Run:
```bash
TAU_RECORD=1 ANTHROPIC_API_KEY=$(security find-generic-password -s ANTHROPIC_API_KEY -w) \
  cargo test -p tau-drivers --all-features --test cassettes_anthropic -- --ignored --nocapture record_all
```
Expected: `recorded anthropic/<name>` × 13, then the µUSD total (expect a few thousand µUSD). If a scenario fails its expectation (e.g. Haiku answers `parallel_tool_calls` with one call), adjust the *prompt* and re-run; do not loosen the expectation. If `stop_sequence_stop` stops at `end_turn` because the model writes "5." on one line, use stop sequence `"\n5"`.

- [ ] **Step 4: Verify replay passes and the guard is green**

Run: `cargo nextest run -p tau-drivers --all-features --test cassettes_anthropic --test cassette_guard`
Expected: 13 replay + 5 guard tests pass. Inspect one cassette by eye: `cat drivers/tests/cassettes/anthropic/tool_call.json` shows only `content-type` and `anthropic-version` headers.

- [ ] **Step 5: Commit**

```bash
git add drivers/tests/cassettes_anthropic.rs drivers/tests/cassettes/anthropic/
git commit -m "test(drivers): Anthropic full-matrix cassettes, recorded on Haiku 4.5 and Opus 5 (#100)"
```

---

### Task 5: OpenAI and Ollama full matrix

**Files:**
- Create: `drivers/tests/cassettes_openai.rs`

**Interfaces:**
- Consumes: same as Task 4. `OpenAiConfig::new(model, Option<ApiKey>, input_bound, max_max_tokens, in_price, out_price)`; `cfg.output_cap = OutputCap::MaxCompletionTokens`.
- Produces: `scenarios(target: Target) -> Vec<(Scenario, Make)>`.

Scenario table (OpenAI on `gpt-4.1-mini`; Ollama on `qwen3:1.7b`, key `None`, prices 0/0; `–` = not for Ollama):

| name | steps | expect | Ollama |
|---|---|---|---|
| `text_end_turn` | text("Reply with the single word: pong.", 64) | Stop(EndTurn) | ✓ |
| `tool_call` | with_calc(text("What is 17*23? Use the calculator tool.", 256)) | ToolCall{calculator,1} | ✓ |
| `tool_result_round_trip` | as Task 4 | step 2 Stop(EndTurn) | ✓ |
| `parallel_tool_calls` | as Task 4 | ToolCall{calculator, min 2} | – |
| `max_tokens_stop` | text("Write a 500-word essay about rivers.", 16) | Stop(MaxTokens) | ✓ |
| `stop_sequence_stop` | as Task 4 | Stop(StopSequence) | – |
| `sampling_accepted` | temperature 0.2 | Stop(EndTurn) | ✓ |
| `max_completion_tokens_cap` | OutputCap::MaxCompletionTokens; text("pong?", 32) | Stop(EndTurn) | – |
| `bad_request_400` | messages [] | ProviderError | ✓ |
| `bad_key_401` | key `Some("invalid-key")` | ProviderError | – |
| `unknown_model_404` | model "gpt-does-not-exist" / "does-not-exist:latest" | ProviderError | ✓ |

- [ ] **Step 1: Write the file**

Mirror Task 4's structure exactly (copy `text`, `calculator`, `with_calc`, `calc_result`, `one`, `round_trip`, the `replay_tests!` macro), with:

```rust
use tau_drivers::model::openai::{ApiKey, OpenAiConfig, OpenAiDriver, OutputCap, API_KEY_ENV};

const MINI: &str = "gpt-4.1-mini";
const QWEN: &str = "qwen3:1.7b";

fn key(target: Target) -> Option<ApiKey> {
    match target {
        Target::Ollama => None,
        _ if scenario::record_enabled() => Some(ApiKey::from_env(API_KEY_ENV).expect("OPENAI_API_KEY")),
        _ => Some(ApiKey::new("replay")),
    }
}

fn make_with(target: Target, model: &'static str, key: Option<ApiKey>, cap: OutputCap) -> Make {
    Box::new(move |base: &str| {
        let (i, o) = if target == Target::Ollama { (0, 0) } else { (1, 4) };
        let mut cfg = OpenAiConfig::new(model, key.clone(), 16_000, 4_096, i, o);
        cfg.base_url = base.to_owned();
        cfg.output_cap = cap;
        Box::new(OpenAiDriver::new(cfg).unwrap()) as Box<dyn Driver>
    })
}
```
`one`/`round_trip` take `target: Target` as a parameter here. `scenarios(target)` returns the OpenAI list for `Target::OpenAi` and the ✓ subset for `Target::Ollama`, with `unknown_model_404` using `"does-not-exist:latest"` on Ollama. Replay tests: two macros invocations, `openai_*` and `ollama_*` prefixes, each calling `find(target, name)`. Two record tests: `record_openai` and `record_ollama` (both `#[ignore]`; `record_ollama` also skips with a message when `reqwest::get(format!("{}/api/tags", Target::Ollama.live_base_url()))` fails).

For Ollama, `check`'s billed assertion needs `CostMicroUsd` to be absent or zero: `Expect` only asserts `Tokens`, so prices 0/0 are fine.

- [ ] **Step 2: Run to verify replay fails for the right reason**

Run: `cargo nextest run -p tau-drivers --all-features --test cassettes_openai openai_text_end_turn`
Expected: FAIL with `openai/text_end_turn: no cassette`.

- [ ] **Step 3: Record OpenAI, then Ollama**

```bash
TAU_RECORD=1 OPENAI_API_KEY=$(security find-generic-password -s OPENAI_API_KEY -w) \
  cargo test -p tau-drivers --all-features --test cassettes_openai -- --ignored --nocapture record_openai
TAU_RECORD=1 cargo test -p tau-drivers --all-features --test cassettes_openai -- --ignored --nocapture record_ollama
```
Expected: 11 + 7 `recorded` lines. If qwen3:1.7b will not produce a tool call for `tool_call`, try the prompt "Use the calculator tool to compute 17*23." once; if it still refuses, drop `tool_call` and `tool_result_round_trip` from the Ollama subset and say so in the commit body.

- [ ] **Step 4: Verify replay and guard**

Run: `cargo nextest run -p tau-drivers --all-features --test cassettes_openai --test cassette_guard`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add drivers/tests/cassettes_openai.rs drivers/tests/cassettes/openai/ drivers/tests/cassettes/ollama/
git commit -m "test(drivers): OpenAI and Ollama full-matrix cassettes (#100)"
```

---

### Task 6: Capability probes for every listed model, `MODELS.md`, inventory drift, cost cap

**Files:**
- Create: `drivers/tests/probes.rs`
- Create (recorded): `drivers/tests/cassettes/{anthropic,openai}/probe_*.json`, `drivers/tests/cassettes/MODELS.md`

**Interfaces:**
- Consumes: Tasks 3–5 helpers (`text`, `with_calc` duplicated here; this file stands alone), `cassette::{all_files, load, Cassette}`, `scenario::{record, replay_from, Expect::Policy}`.
- Produces:
  ```rust
  #[derive(Clone, Copy)] enum Probe { Text, ToolCall, SamplingPresent, ThinkingDisabled, MaxCompletionTokens }
  fn probe_name(p: Probe, model: &str) -> String;   // "probe_<kind>__<model with ':' and '/' → '-'>"
  fn scenario_for(target: Target, model: String, p: Probe) -> (Scenario, Make);
  fn render_models_md(cassettes: &[Cassette], retired: &[String], unprobed: &[String]) -> String;
  async fn list_models(target: Target, key: &str) -> Vec<String>;   // GET /v1/models, ids; OpenAI filtered to chat-capable, dated dupes and *-chat-latest collapsed
  fn price(target: Target, model: &str) -> (u64, u64);
  ```
  `Scenario.name` is `&'static str`; leak the probe name with `Box::leak(name.into_boxed_str())` (test code, bounded count).

Probe requests:
- `Text`: `text("Reply with the single word: pong.", 64)` → `Expect::Policy`
- `ToolCall`: `with_calc(text("What is 17*23? Use the calculator tool.", 512))` → `Expect::Policy`
- `SamplingPresent`: `text("pong?", 32)` with `temperature: 0.2`, config `SamplingMode::Accepted` (Anthropic) → `Expect::Policy`
- `ThinkingDisabled` (Anthropic only): `text("pong?", 64)`, config `ThinkingMode::Disabled` → `Expect::Policy`
- `MaxCompletionTokens` (OpenAI only): `text("pong?", 32)`, config `OutputCap::MaxCompletionTokens` → `Expect::Policy`

`max_max_tokens` for probes: 4_096 (o-series and gpt-5 reasoning models need headroom; `max_tokens` is clamped, not the request's ask).

OpenAI model filter for `list_models`: keep ids matching `^(gpt-|o\d)`; drop any containing `audio|realtime|image|tts|transcribe|search|embedding|moderation|instruct|codex|live`; drop ids that end in `-YYYY-MM-DD` when the undated alias is also present; drop `*-chat-latest`. Sort so ids containing `pro` come last. Anthropic: all ids, sorted so `haiku` < `sonnet` < `opus` < `fable` (cheapest first).

`MODELS.md` shape:
```markdown
# Provider model capability table

Rendered from `drivers/tests/cassettes/*/probe_*.json` by `probes::render_models_md`; do not edit by hand.

## anthropic

| model | text | tool call | sampling present | thinking disabled |
|---|---|---|---|---|
| claude-haiku-4-5-20251001 | ok | ok | ok | ok |
| claude-opus-5 | ok | ok | 400 temperature… | ok |

## openai

| model | text | tool call | sampling present | max_completion_tokens |
|---|---|---|---|---|
| gpt-4.1-mini | ok | ok | ok | ok |

## inventory (at last record)

- retired (cassette, no longer listed): none
- unprobed (listed, no cassette): none
```
A cell is `ok` when the recorded response status is 2xx, else `<status> <first 40 chars of error message>`, else `—` when no cassette exists for that probe.

- [ ] **Step 1: Write the failing replay/render tests**

```rust
#[tokio::test]
async fn every_probe_cassette_replays_against_the_driver_that_recorded_it() {
    let mut n = 0;
    for file in cassette::all_files() {
        let stem = file.file_stem().unwrap().to_str().unwrap();
        let Some(rest) = stem.strip_prefix("probe_") else { continue };
        let (kind, model_slug) = rest.split_once("__").unwrap();
        let c: Cassette = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let target = match c.target.as_str() { "anthropic" => Target::Anthropic, "openai" => Target::OpenAi, t => panic!("{t}") };
        let model = c.model.clone().unwrap_or_else(|| model_slug.to_owned());
        let (s, make) = scenario_for(target, model, Probe::parse(kind));
        scenario::replay_from(&s, target.dir_name(), make).await;
        n += 1;
    }
    eprintln!("{n} probe cassettes replayed");
}

#[test]
fn models_md_is_what_the_cassettes_render_to() {
    let cassettes: Vec<Cassette> = cassette::all_files().iter()
        .filter(|f| f.file_stem().unwrap().to_str().unwrap().starts_with("probe_"))
        .map(|f| serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap())
        .collect();
    let on_disk = std::fs::read_to_string(cassette::dir().join("MODELS.md")).unwrap_or_default();
    let (retired, unprobed) = inventory_from(&on_disk);   // parses the two bullet lines back; render is otherwise pure
    assert_eq!(on_disk, render_models_md(&cassettes, &retired, &unprobed), "MODELS.md is stale; run `just live record`");
}
```
Replay for a probe uses the *recorded* model id (dated for OpenAI) so the request body matches: `scenario_for` puts `model` into the config. The cassette's `model` field was set from the reply's `model` at record time; for a 4xx probe the reply has no model, so `record` falls back to `s.model`, the alias. Both replay identically because the driver sends whatever the config says and the cassette holds exactly that.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p tau-drivers --all-features --test probes`
Expected: compile errors (`Probe`, `scenario_for`, `render_models_md`, `inventory_from` undefined).

- [ ] **Step 3: Implement `probes.rs` record path**

```rust
#[tokio::test]
#[ignore = "TAU_RECORD=1 and provider keys; costs money"]
async fn record_probes() {
    if !scenario::record_enabled() { eprintln!("TAU_RECORD is not 1; skipping"); return; }
    let cap: u64 = std::env::var("TAU_RECORD_CAP_MICROUSD").ok().and_then(|s| s.parse().ok()).unwrap_or(3_000_000);
    let only = std::env::var("TAU_RECORD_TARGET").ok();   // "anthropic" | "openai" | unset = both
    let mut spent = 0u64;
    let mut retired = Vec::new();
    let mut unprobed = Vec::new();
    for target in [Target::Anthropic, Target::OpenAi] {
        if only.as_deref().is_some_and(|o| o != target.dir_name()) { continue; }
        let key = std::env::var(match target { Target::Anthropic => "ANTHROPIC_API_KEY", _ => "OPENAI_API_KEY" }).expect("key");
        let listed = list_models(target, &key).await;
        let have: BTreeSet<String> = cassette::all_files().iter()
            .filter_map(|f| { let c: Cassette = serde_json::from_str(&std::fs::read_to_string(f).ok()?).ok()?; (c.target == target.dir_name()).then_some(c.model?) })
            .collect();
        retired.extend(have.iter().filter(|m| !listed.contains(*m)).cloned());
        'models: for model in &listed {
            for probe in Probe::for_target(target) {
                if spent >= cap {
                    eprintln!("cap {cap} µUSD reached before {model}; stopping");
                    unprobed.extend(listed.iter().skip_while(|m| m != &model).cloned());
                    break 'models;
                }
                let (s, make) = scenario_for(target, model.clone(), probe);
                let c = scenario::record(&s, make).await;
                spent += c.get(&DimKey::CostMicroUsd).unwrap_or(0);
            }
        }
    }
    let cassettes = /* reload all probe_* cassettes as in the render test */;
    std::fs::write(cassette::dir().join("MODELS.md"), render_models_md(&cassettes, &retired, &unprobed)).unwrap();
    eprintln!("probes recorded; {spent} µUSD; retired={retired:?} unprobed={unprobed:?}");
}
```
`list_models` uses `reqwest::Client::new().get(format!("{}/v1/models", target.live_base_url()))` with header `x-api-key` + `anthropic-version: 2023-06-01` for Anthropic (also `?limit=100`), `authorization: Bearer` for OpenAI; parse `data[].id`.

`price(target, model)`: Anthropic by prefix: `claude-fable` → (10, 50); `claude-opus` → (5, 25); `claude-sonnet-5` → (2, 10); `claude-sonnet` → (3, 15); `claude-haiku` → (1, 5). OpenAI: (1, 4) placeholder for every model, except ids containing `pro` → (20, 100) so the cap bites early on them.

The `Expect::Policy` check accepts a Provider error, so a 400 for `SamplingPresent` on Opus 5 is recorded as evidence, not treated as failure. A 401 on every model would also "pass" Policy; guard against a bad key by asserting, after the first target's first probe, that its cassette's status is not 401 (`panic!("key rejected")`).

- [ ] **Step 4: Record**

```bash
TAU_RECORD=1 \
ANTHROPIC_API_KEY=$(security find-generic-password -s ANTHROPIC_API_KEY -w) \
OPENAI_API_KEY=$(security find-generic-password -s OPENAI_API_KEY -w) \
  cargo test -p tau-drivers --all-features --test probes -- --ignored --nocapture record_probes
```
Expected: about 44 Anthropic + about 90 OpenAI `recorded` lines, a µUSD total under the cap, `MODELS.md` written. Check the Anthropic console balance afterwards; if the cap stopped the run early, re-run with `TAU_RECORD_TARGET=anthropic` after adding funds, or accept the `unprobed` list.

- [ ] **Step 5: Verify replay, render, guard, quick profile timing**

Run: `cargo nextest run -p tau-drivers --all-features --profile quick`
Expected: all pass, `every_probe_cassette_replays…` well under 5 s (about 130 socket round-trips locally is ~1 s). If it is over ~1.5 s, split by target into two tests.

- [ ] **Step 6: Commit**

```bash
git add drivers/tests/probes.rs drivers/tests/cassettes/
git commit -m "test(drivers): capability probes on every listed model, MODELS.md, inventory drift (#100)"
```

---

### Task 7: `just live`

**Files:**
- Modify: `justfile` (append after `coverage`)
- Modify: `CLAUDE.md` Commands list (one line)

- [ ] **Step 1: Add the recipe**

```just
# Provider cassettes. `just live` replays; `just live record [anthropic|openai|ollama|probes]`
# re-records through the relay with keys from the macOS Keychain
# (`security find-generic-password`; store with `security add-generic-password -U -a "$USER"
# -s ANTHROPIC_API_KEY -w "$(pbpaste)"`). Costs money; see drivers/tests/cassettes/MODELS.md.
live mode="replay" target="all":
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "{{mode}}" = "replay" ]; then
      exec cargo nextest run -p tau-drivers --all-features --profile quick \
        --test cassette_guard --test cassettes_anthropic --test cassettes_openai --test probes
    fi
    [ "{{mode}}" = "record" ] || { echo "mode is replay or record"; exit 2; }
    export TAU_RECORD=1
    key() { security find-generic-password -s "$1" -w 2>/dev/null || { echo "no Keychain entry $1" >&2; exit 2; }; }
    case "{{target}}" in
      all|anthropic) ANTHROPIC_API_KEY="$(key ANTHROPIC_API_KEY)" cargo test -p tau-drivers --all-features --test cassettes_anthropic -- --ignored --nocapture record_all ;;&
      all|openai)    OPENAI_API_KEY="$(key OPENAI_API_KEY)" cargo test -p tau-drivers --all-features --test cassettes_openai -- --ignored --nocapture record_openai ;;&
      all|ollama)    cargo test -p tau-drivers --all-features --test cassettes_openai -- --ignored --nocapture record_ollama ;;&
      all|probes)    ANTHROPIC_API_KEY="$(key ANTHROPIC_API_KEY)" OPENAI_API_KEY="$(key OPENAI_API_KEY)" cargo test -p tau-drivers --all-features --test probes -- --ignored --nocapture record_probes ;;
      *) echo "target is all|anthropic|openai|ollama|probes"; exit 2 ;;
    esac
    echo; echo "re-recorded; review with: git diff --stat drivers/tests/cassettes"
```
`;;&` (bash 4+) falls through to test the next pattern; macOS `/usr/bin/env bash` may be 3.2. If `bash --version` on the runner is 3.x, replace the `case` with four `if [[ "$t" = all || "$t" = anthropic ]]; then …; fi` blocks.

- [ ] **Step 2: Verify**

Run: `just live` → replay suites green. Run: `just live record ollama` → re-records the 7 Ollama cassettes; `git diff --stat drivers/tests/cassettes/ollama` shows changes only in `recorded_at`, response ids, `created`, and text content. Then `git checkout drivers/tests/cassettes/ollama` to keep the committed set.

- [ ] **Step 3: CLAUDE.md line**

Under `## Commands` add: ``- `just live [record [target]]` — provider cassettes: replay (default) or re-record via Keychain keys. See `drivers/tests/cassettes/MODELS.md`.``

- [ ] **Step 4: Commit**

```bash
git add justfile CLAUDE.md
git commit -m "build(just): live recipe — replay and re-record provider cassettes (#100)"
```

---

### Task 8: Gate, PR, spec sync

- [ ] **Step 1: Full gate**

Run: `just check` then `just test`.
Expected: green. Clippy on tests: the new files carry the same `#![allow]` as the existing suites; `BTreeMap`/`BTreeSet` only.

- [ ] **Step 2: Sync the spec's §2 and §4 with what shipped** (exchanges array; no masking in replay). Already done in the spec commit accompanying this plan; re-read and correct any drift (e.g. Ollama subset if `tool_call` was dropped).

- [ ] **Step 3: Update PR #103 body checkboxes, mark ready, babysit to merge per CLAUDE.md**

```bash
gh pr ready 103
gh pr view 103 --json mergeStateStatus
```

---

## Self-review

- **Spec coverage:** §1 two modes → Task 3; §2 format → Task 1 (exchanges array); §3 allowlist → Task 1; §4 → replay compares exact bodies, no masking (spec corrected); §5 guard → Task 1; §6 tiers → Tasks 4–6; §7 inventory drift → Task 6; §8 `just live` → Task 7; §9 cost guard → Task 6. Not-recordable list stays in existing suites (no task; nothing to do).
- **Placeholders:** the `/* reload all probe_* cassettes … */` in Task 6 Step 3 refers to the exact expression in Step 1's `models_md_is_what_the_cassettes_render_to`; copy it. `inventory_from` parses the two `- retired …` / `- unprobed …` lines (split on `: `, then `, `; `none` → empty).
- **Type consistency:** `Make = Box<dyn Fn(&str) -> Box<dyn Driver> + Send + Sync>` everywhere; `Expect::ToolCall { name, min }` in Tasks 3–5; `scenario::replay_from(&Scenario, &str, Make)` used by Tasks 3 and 6; `Consumption::get(&DimKey) -> Option<u64>` as in the existing live tests.
