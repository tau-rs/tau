//! Live smoke tests against api.openai.com.
//!
//! Setup:
//!   export OPENAI_API_KEY=sk-proj-...
//!   TAU_OPENAI_LIVE_TESTS=1 cargo test -p openai --test live -- \
//!     --ignored --nocapture
//!
//! Cost: ~$0.001 per smoke run on gpt-4o-mini.
//!
//! These tests are #[ignore]-by-default. CI does not run them.
//! Maintainer-triggered to detect OpenAI API drift between cassette
//! re-records.

mod common;

use futures_util::StreamExt;
use openai_plugin_lib::{config::OpenAIConfig, plugin::OpenAIPlugin};
use tau_plugin_sdk::Configure;
use tau_ports::{CompletionChunk, CompletionRequest, ContentBlock, LlmBackend, LlmProviderMessage};

/// Build an `OpenAIConfig` for live tests. Reads `OPENAI_API_KEY`
/// from the environment; panics if missing.
fn live_config() -> OpenAIConfig {
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required for live tests");
    let mut cfg = OpenAIConfig::default();
    cfg.api_key = Some(api_key.into());
    cfg
}

fn live_model() -> String {
    std::env::var("TAU_OPENAI_LIVE_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into())
}

fn live_request() -> CompletionRequest {
    let mut req = CompletionRequest::new(live_model());
    req.messages
        .push(LlmProviderMessage::user(vec![ContentBlock::Text(
            "say hi in exactly 3 words".into(),
        )]));
    req.max_tokens = Some(20);
    req
}

#[tokio::test]
#[ignore = "live: requires TAU_OPENAI_LIVE_TESTS=1 + OPENAI_API_KEY"]
async fn live_complete_smoke() {
    if std::env::var("TAU_OPENAI_LIVE_TESTS").is_err() {
        eprintln!("skipping: TAU_OPENAI_LIVE_TESTS not set");
        return;
    }
    let plugin = OpenAIPlugin::from_config(live_config()).unwrap();
    let resp = plugin.complete(live_request()).await.unwrap();
    assert!(!resp.text.is_empty(), "live response had empty text");
    eprintln!("live response text: {:?}", resp.text);
    eprintln!("live usage: {:?}", resp.usage);
}

#[tokio::test]
#[ignore = "live: requires TAU_OPENAI_LIVE_TESTS=1 + OPENAI_API_KEY"]
async fn live_stream_smoke() {
    if std::env::var("TAU_OPENAI_LIVE_TESTS").is_err() {
        eprintln!("skipping: TAU_OPENAI_LIVE_TESTS not set");
        return;
    }
    let plugin = OpenAIPlugin::from_config(live_config()).unwrap();
    let mut stream = plugin.stream(live_request()).await.unwrap();
    let mut text_chunks = 0;
    let mut got_finish = false;
    let mut accumulated_text = String::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(CompletionChunk::Text { delta }) => {
                text_chunks += 1;
                accumulated_text.push_str(&delta);
            }
            Ok(CompletionChunk::ToolUse(_)) => {
                // Unlikely on a "say hi" prompt without tools.
            }
            Ok(CompletionChunk::Finish { stop_reason, usage }) => {
                got_finish = true;
                eprintln!("live stream stop_reason: {stop_reason:?}, usage: {usage:?}");
            }
            Ok(other) => {
                eprintln!("unexpected chunk variant: {other:?}");
            }
            Err(e) => panic!("stream error: {e:?}"),
        }
    }
    assert!(text_chunks > 0, "expected at least one text chunk");
    assert!(got_finish, "expected a Finish chunk");
    assert!(!accumulated_text.is_empty(), "accumulated text was empty");
    eprintln!("live accumulated text: {accumulated_text:?}");
}
