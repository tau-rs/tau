//! Live smoke tests against api.anthropic.com.
//!
//! Run with:
//!   TAU_ANTHROPIC_LIVE_TESTS=1 ANTHROPIC_API_KEY=sk-ant-... \
//!     cargo test -p anthropic --test live -- --ignored --nocapture
//!
//! Costs: ~$0.001 per smoke run on `claude-3-5-haiku-latest`.
//!
//! These tests are #[ignore]-by-default. CI does not run them.
//! Maintainer-triggered, ~quarterly cadence to detect Anthropic API
//! drift between cassette re-records.

mod common;

use anthropic_plugin_lib::{config::AnthropicConfig, plugin::AnthropicPlugin};
use futures_util::StreamExt;
use tau_plugin_sdk::Configure;
use tau_ports::{CompletionChunk, CompletionRequest, ContentBlock, LlmBackend, LlmProviderMessage};

/// Build an `AnthropicConfig` for live tests. Reads
/// `ANTHROPIC_API_KEY` from the environment; panics if missing.
fn live_config() -> AnthropicConfig {
    let api_key =
        std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY required for live tests");
    // `AnthropicConfig` is `#[non_exhaustive]`: from outside the crate
    // we can't use struct-literal construction. Build via Default and
    // mutate.
    let mut cfg = AnthropicConfig::default();
    cfg.api_key = Some(api_key.into());
    cfg.base_url = "https://api.anthropic.com".into();
    cfg
}

fn live_request() -> CompletionRequest {
    let mut req = CompletionRequest::new("claude-3-5-haiku-latest".into());
    req.messages
        .push(LlmProviderMessage::user(vec![ContentBlock::Text(
            "say hi in exactly 3 words".into(),
        )]));
    req.max_tokens = Some(20);
    req
}

#[tokio::test]
#[ignore = "live: requires TAU_ANTHROPIC_LIVE_TESTS=1 and ANTHROPIC_API_KEY"]
async fn live_complete_smoke() {
    if std::env::var("TAU_ANTHROPIC_LIVE_TESTS").is_err() {
        eprintln!("skipping: TAU_ANTHROPIC_LIVE_TESTS not set");
        return;
    }
    let plugin = AnthropicPlugin::from_config(live_config()).unwrap();
    let resp = plugin.complete(live_request()).await.unwrap();
    assert!(!resp.text.is_empty(), "live response had empty text");
    eprintln!("live response text: {:?}", resp.text);
    eprintln!("live usage: {:?}", resp.usage);
}

#[tokio::test]
#[ignore = "live: requires TAU_ANTHROPIC_LIVE_TESTS=1 and ANTHROPIC_API_KEY"]
async fn live_stream_smoke() {
    if std::env::var("TAU_ANTHROPIC_LIVE_TESTS").is_err() {
        eprintln!("skipping: TAU_ANTHROPIC_LIVE_TESTS not set");
        return;
    }
    let plugin = AnthropicPlugin::from_config(live_config()).unwrap();
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
                // Unlikely on a "say hi" prompt without tools, but
                // tolerate the possibility.
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
