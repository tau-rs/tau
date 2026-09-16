//! The sandbox as a tool: `Toolbox::project` reads the driver's schema
//! through the kernel, the loop sends the model's `input` verbatim, and
//! the model reads the sandbox reply back as the `tool_result`.

#![cfg(all(feature = "sandbox", unix))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::sandbox::{config, driver, sh_id, tokio_spawner};
use libtau::{prompt, tool_loop, Toolbox};
use serde_json::json;
use tau_drivers::sandbox::wire::{Reply, Stop};
use tau_kernel::abi::{Budget, Consumption, DimKey, DriverId, Name, Namespace};
use tau_kernel::bridge::{Content, ModelReply, ModelRequest, Role, StopReason, Usage, VERSION};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::{BoxFuture, Delivery, Kernel};
use tau_kernel::log::Log;
use tau_kernel::syscall::program;

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
        Box::pin(async move { (bytes, Consumption::from_dims([(DimKey::Tokens, 15)])) })
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

#[tokio::test]
async fn one_call_through_the_loop_reads_stdout_back_in_the_tool_result() {
    let model = ScriptedModel {
        replies: Arc::new(Mutex::new(VecDeque::from([
            reply(
                vec![Content::ToolCall {
                    id: "call_1".into(),
                    name: "sh".into(),
                    input: json!({ "code": "echo hello from the sandbox" }),
                }],
                StopReason::ToolCall,
            ),
            reply(
                vec![Content::Text {
                    text: "It said hello.".into(),
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
    let sandbox = driver(config(5, Duration::from_secs(10)));
    let sh_cap = kernel
        .register_driver(sh_id(), sandbox.clone(), sandbox.ceiling())
        .unwrap();
    let ns = Namespace::from_caps([model_cap, sh_cap]);
    let out: Arc<Mutex<Option<ModelRequest>>> = Arc::default();
    let sink = Arc::clone(&out);
    let root = kernel
        .spawn_root(
            program(move |h| async move {
                let toolbox = Toolbox::project(&h, &[sh_cap]).expect("the sandbox is a tool");
                assert_eq!(toolbox.len(), 1);
                let mut request = prompt("Say hello through the shell.", 256);
                let mut notes = Vec::new();
                let last = tool_loop(&h, model_cap, &toolbox, &mut request, &mut notes)
                    .await
                    .expect("the loop ends on end_turn");
                assert_eq!(last.stop, StopReason::EndTurn);
                sink.lock().unwrap().replace(request);
                h.exit(b"")
            }),
            ns,
            Budget::from_dims([
                (DimKey::Tokens, 10_000),
                (DimKey::ComputeMs, 10_000),
                (DimKey::Calls, 10),
            ]),
        )
        .unwrap();
    kernel.drained().await.unwrap();
    kernel.shutdown();

    // The model saw the projected schema: the driver's, under the name the
    // harness registered.
    let seen = model.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    let tools = seen.first().unwrap().tools.clone();
    assert_eq!(tools.len(), 1);
    let def = tools.first().unwrap();
    assert_eq!(def.name.as_str(), "sh");
    assert!(def.description.contains("5 s CPU"), "{}", def.description);
    assert_eq!(def.input_schema["additionalProperties"], json!(false));

    // The tool result is the sandbox reply, verbatim, and it says hello.
    let transcript = out.lock().unwrap().take().unwrap();
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
    assert!(!is_error);
    let sandbox_reply: Reply = serde_json::from_str(content).unwrap();
    assert_eq!(sandbox_reply.stop, Stop::Exit(0));
    assert_eq!(sandbox_reply.stdout, "hello from the sandbox\n");

    let rec = kernel.state().agent(root).unwrap().clone();
    assert_eq!(
        rec.spent.get(&DimKey::ComputeMs),
        Some(&sandbox_reply.usage.compute_ms()),
        "the run's CPU was settled on the agent"
    );
}
