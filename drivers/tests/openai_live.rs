//! One real call, ignored by default. Against OpenAI:
//!
//! ```sh
//! TAU_OPENAI_LIVE=1 OPENAI_API_KEY=... \
//!     cargo test -p tau-drivers --test openai_live -- --ignored
//! ```
//!
//! or against a local vLLM (no key needed):
//!
//! ```sh
//! TAU_OPENAI_LIVE=1 TAU_OPENAI_BASE_URL=http://localhost:8000 \
//!     TAU_OPENAI_MODEL=Qwen/Qwen3-8B \
//!     cargo test -p tau-drivers --test openai_live -- --ignored
//! ```
//!
//! It costs a few hundred tokens and asserts only what the contract
//! promises: a reply that is not an error, a real usage, and a consumption
//! priced from it. Prices here are placeholders: the assertion is that the
//! arithmetic holds, not that the number is a bill.

#![cfg(feature = "openai")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use tau_drivers::model::openai::{ApiKey, OpenAiConfig, OpenAiDriver, API_KEY_ENV};
use tau_kernel::abi::Name;
use tau_kernel::abi::{AgentId, Corr, DimKey};
use tau_kernel::bridge::{
    Content, Message, ModelReply, ModelRequest, Role, StopReason, ToolDef, VERSION,
};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::Delivery;

#[tokio::test]
#[ignore = "needs TAU_OPENAI_LIVE=1 and OPENAI_API_KEY or TAU_OPENAI_BASE_URL; may cost money"]
async fn one_real_call_round_trips() {
    if std::env::var("TAU_OPENAI_LIVE").as_deref() != Ok("1") {
        eprintln!("TAU_OPENAI_LIVE is not 1; skipping");
        return;
    }
    let model = std::env::var("TAU_OPENAI_MODEL").unwrap_or_else(|_| "gpt-4.1-mini".to_owned());
    let key = ApiKey::from_env(API_KEY_ENV).ok();
    let mut config = OpenAiConfig::new(model, key, 8_000, 256, 1, 4);
    if let Ok(base_url) = std::env::var("TAU_OPENAI_BASE_URL") {
        config.base_url = base_url;
    }
    let driver = OpenAiDriver::new(config).unwrap();

    let request = ModelRequest {
        v: VERSION,
        system: Some("Answer in one short sentence.".into()),
        messages: vec![Message {
            role: Role::User,
            content: vec![Content::Text {
                text: "What is 2 + 2? Use the calculator tool.".into(),
            }],
        }],
        tools: vec![ToolDef {
            name: Name::new("calculator").unwrap(),
            description: "Evaluates an arithmetic expression.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"expression": {"type": "string"}},
                "required": ["expression"],
                "additionalProperties": false
            }),
        }],
        max_tokens: 4_096,
        sampling: None,
    };
    let (bytes, consumed) = driver
        .handle(Delivery {
            corr: Corr::new(1),
            from: AgentId::new(1),
            payload: serde_json::to_vec(&request).unwrap(),
        })
        .await;
    let reply: ModelReply = serde_json::from_slice(&bytes).unwrap();
    eprintln!("{reply:#?}\n{consumed:?}");

    assert!(
        !matches!(reply.stop, StopReason::Error(_)),
        "{:?}",
        reply.stop
    );
    assert!(reply.model.is_some());
    assert!(reply.usage.input_tokens > 0 && reply.usage.output_tokens > 0);
    assert_eq!(
        consumed.get(&DimKey::Tokens),
        Some(reply.usage.input_tokens + reply.usage.output_tokens)
    );
    assert_eq!(
        consumed.get(&DimKey::CostMicroUsd),
        Some(reply.usage.input_tokens + reply.usage.output_tokens * 4)
    );
}
