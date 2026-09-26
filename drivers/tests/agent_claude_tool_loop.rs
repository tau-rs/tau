//! The `claude` driver as a tool in the `libtau` loop (ADR-0013 §9, §10):
//! `Toolbox::project` reads the driver's schema through the kernel, the
//! stub model answers one `tool_use` against it, the driver runs
//! `tau-fake-cli` replaying #130's `1-hello` behind the `ClaudeStub`
//! wrapper, and the model reads the worker envelope back as the
//! `tool_result` — exactly the driver's reply bytes (ADR-0006 §5).
//!
//! The shape is `sandbox_tool_loop.rs`'s, over `ClaudeDriver`.

#![cfg(all(feature = "agent-claude", unix))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "common/agent.rs"]
mod agent;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use libtau::{prompt, tool_loop, Toolbox};
use serde_json::json;
use tau_drivers::agent::claude::ClaudeDriver;
use tau_drivers::agent::envelope::Status;
use tau_drivers::agent::wire::{Reply, Stop};
use tau_kernel::abi::{Budget, Consumption, Corr, DimKey, DriverId, Name, Namespace};
use tau_kernel::bridge::{Content, ModelReply, ModelRequest, Role, StopReason, Usage, VERSION};
use tau_kernel::driver::{Driver, ToolSchema};
use tau_kernel::kernel::{AbortHandle, BoxFuture, Delivery, Kernel};
use tau_kernel::log::Log;
use tau_kernel::syscall::program;

use agent::ClaudeStub;

const PIN: &str = "claude-2.1.272";

/// What the `1-hello` session wrote as its `summary` (#130 run 1).
const SUMMARY: &str = "Created hello.txt with the content \"hello\" in the working directory.";

/// What the `1-hello` result line reported, and what the kernel test
/// `agent_claude_kernel.rs` pins the driver's `Consumption` at.
const CLI_TOKENS: u64 = 44_506;
const CLI_COST_MICROUSD: u64 = 259_148;

/// What the scripted model bills per call.
const MODEL_TOKENS: u64 = 15;

fn tokio_spawner(fut: BoxFuture<()>) -> AbortHandle {
    let task = tokio::spawn(fut);
    Box::new(move || task.abort())
}

/// A model that answers from a script and remembers what it was asked.
#[derive(Clone)]
struct ScriptedModel {
    replies: Arc<Mutex<VecDeque<ModelReply>>>,
    seen: Arc<Mutex<Vec<ModelRequest>>>,
}

impl Driver for ScriptedModel {
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        let decoded: ModelRequest = serde_json::from_slice(&request.payload).unwrap();
        self.seen.lock().unwrap().push(decoded);
        let reply = self.replies.lock().unwrap().pop_front().unwrap();
        let bytes = serde_json::to_vec(&reply).unwrap();
        Box::pin(async move {
            (
                bytes,
                Consumption::from_dims([(DimKey::Tokens, MODEL_TOKENS)]),
            )
        })
    }
}

fn reply(content: Vec<Content>, stop: StopReason) -> ModelReply {
    ModelReply {
        v: VERSION,
        model: Some("scripted".into()),
        content,
        stop,
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
        },
    }
}

/// The real driver, with a tap on the bytes it replies: what the model
/// must read back verbatim. Not a mock — every `send` reaches
/// `ClaudeDriver`, and through it the fake CLI.
#[derive(Clone)]
struct Tapped {
    driver: ClaudeDriver,
    replied: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Driver for Tapped {
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        let inner = self.driver.handle(request);
        let replied = Arc::clone(&self.replied);
        Box::pin(async move {
            let (bytes, consumption) = inner.await;
            replied.lock().unwrap().push(bytes.clone());
            (bytes, consumption)
        })
    }

    fn describe(&self) -> Option<ToolSchema> {
        self.driver.describe()
    }

    fn abandon(&self, corr: Corr) {
        self.driver.abandon(corr);
    }
}

#[tokio::test]
async fn one_delegated_task_through_the_loop_reads_the_envelope_back_in_the_tool_result() {
    let dir = agent::Temp::new("tool-loop-hello");
    let script = agent::replay_script(dir.path(), PIN, "1-hello");
    let stub = ClaudeStub::new(dir.path());
    let config = stub.config(&script, dir.path());
    let claude = Tapped {
        driver: ClaudeDriver::new(config).unwrap(),
        replied: Arc::default(),
    };
    let model = ScriptedModel {
        replies: Arc::new(Mutex::new(VecDeque::from([
            reply(
                vec![Content::ToolCall {
                    id: "call_1".into(),
                    name: "claude".into(),
                    input: json!({ "op": "run", "task": "Write hello.txt containing hello." }),
                }],
                StopReason::ToolCall,
            ),
            reply(
                vec![Content::Text {
                    text: format!("The session reported: {SUMMARY}"),
                }],
                StopReason::EndTurn,
            ),
        ]))),
        seen: Arc::default(),
    };

    let kernel = Kernel::boot(Log::in_memory(), tokio_spawner);
    let model_cap = kernel
        .register_driver(
            DriverId::new(Name::new("model").unwrap()),
            model.clone(),
            Budget::from_dims([(DimKey::Tokens, 1_000)]),
        )
        .unwrap();
    let claude_cap = kernel
        .register_driver(
            DriverId::new(Name::new("claude").unwrap()),
            claude.clone(),
            claude.driver.ceiling(),
        )
        .unwrap();
    let ns = Namespace::from_caps([model_cap, claude_cap]);
    let out: Arc<Mutex<Option<(ModelRequest, ModelReply)>>> = Arc::default();
    let sink = Arc::clone(&out);
    let root = kernel
        .spawn_root(
            program(move |h| async move {
                let toolbox = Toolbox::project(&h, &[claude_cap]).expect("the driver is a tool");
                assert_eq!(toolbox.len(), 1);
                let mut request = prompt("Have a worker write hello.txt.", 256);
                let mut notes = Vec::new();
                let last = tool_loop(&h, model_cap, &toolbox, &mut request, &mut notes)
                    .await
                    .expect("the loop ends on end_turn");
                assert_eq!(last.stop, StopReason::EndTurn);
                sink.lock().unwrap().replace((request, last));
                h.exit(b"")
            }),
            ns,
            // Enough to reserve the driver's ceiling: 400k tokens and $2.
            Budget::from_dims([
                (DimKey::Tokens, 1_000_000),
                (DimKey::CostMicroUsd, 5_000_000),
                (DimKey::Calls, 10),
            ]),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();

    // ADR-0013 §9: the model saw the driver's schema under the name the
    // harness registered, with the bound in the sentence.
    let seen = model.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "one tool exchange, then the answer");
    let tools = seen[0].tools.clone();
    assert_eq!(tools.len(), 1);
    let def = &tools[0];
    assert_eq!(def.name.as_str(), "claude");
    assert!(
        def.description
            .contains("spend up to $2 (400k tokens, 40 turns)"),
        "{}",
        def.description
    );
    assert!(
        def.description.contains("Pass `session`"),
        "the claude adapter offers `resume`: {}",
        def.description
    );
    let branches = def.input_schema["oneOf"].as_array().unwrap();
    assert_eq!(branches.len(), 2, "run and resume");
    assert!(
        branches
            .iter()
            .all(|b| b["additionalProperties"] == json!(false)),
        "each branch closes itself (#200)"
    );

    // ADR-0006 §5: the tool result is exactly the bytes the driver replied.
    let (transcript, last) = out.lock().unwrap().take().unwrap();
    let last_user = transcript
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .unwrap();
    let Some(Content::ToolResult {
        call_id,
        content,
        is_error,
        ..
    }) = last_user.content.first()
    else {
        panic!("{:?}", last_user.content)
    };
    assert_eq!(call_id, "call_1");
    assert!(!is_error, "{content}");
    let replied = claude.replied.lock().unwrap().clone();
    assert_eq!(replied.len(), 1, "one delegated task");
    assert_eq!(content.as_bytes(), replied[0].as_slice());

    // The second call carried them: what the model read is what the
    // driver wrote, and it is the envelope.
    let carried = seen[1]
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .unwrap();
    assert_eq!(carried.content.first(), Some(&last_user.content[0]));
    let agent_reply: Reply = serde_json::from_str(content).unwrap();
    assert_eq!(agent_reply.stop, Stop::Done);
    let envelope = agent_reply.envelope.expect("the session wrote a report");
    assert_eq!(envelope.status, Status::Ok);
    assert_eq!(envelope.summary, SUMMARY);
    assert_eq!(
        agent_reply.session.as_deref(),
        Some("affe155e-9ed1-4477-95f0-c17d40bd9a89"),
        "what a `resume` would take"
    );
    assert_eq!(
        agent_reply.usage.cost_microusd,
        Some(CLI_COST_MICROUSD),
        "the reply's usage is what the CLI stated"
    );

    // The model's answer quoted the summary.
    let Some(Content::Text { text }) = last.content.first() else {
        panic!("{:?}", last.content)
    };
    assert!(text.contains(SUMMARY), "{text}");

    // ADR-0013 §6: the requester's record settled at what the CLI reported
    // (plus the model's own two calls), the reservation at the ceiling
    // released, one `call` per send, no overdraft.
    let record = kernel.state().agent(root).unwrap().clone();
    assert!(record.reserved.is_empty(), "settled at the reply");
    assert_eq!(
        record.spent.get(&DimKey::Tokens),
        Some(&(CLI_TOKENS + 2 * MODEL_TOKENS))
    );
    assert_eq!(
        record.spent.get(&DimKey::CostMicroUsd),
        Some(&CLI_COST_MICROUSD)
    );
    assert_eq!(
        record.spent.get(&DimKey::Calls),
        Some(&3),
        "two model calls and one delegated task"
    );
    assert!(record.overdraft.is_empty(), "under the ceiling");
    assert_eq!(
        stub.probes(),
        1,
        "the login probe at construction, and none for a send that ran"
    );
}
