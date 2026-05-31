//! Integration tests for the host-side IPC adapters in
//! [`tau_runtime::plugin_host::__internals`].
//!
//! These tests pair an [`IpcLlmBackend`] / [`IpcTool`] / [`IpcStorage`]
//! with a [`FakeStdioPeer`] (test-driven plugin side)
//! through two duplex streams, build a [`PluginProcess`] via the
//! `new_for_test` constructor (no real subprocess), and exercise one
//! RPC per adapter to verify:
//!
//! 1. Wire shape (frame method + params bytes) matches the SDK side.
//! 2. Response decoding produces the expected typed result.
//! 3. Wire-level error envelopes surface as
//!    `{Llm,Tool,Storage}Error::Internal { message }`.
//!
//! The DynLlmBackend / DynTool / DynStorage shim traits
//! return `BoxFuture<'a, _>` futures that are deliberately *not*
//! `Send`-bounded (see `crate::builder` module-level docs), so the
//! call/peer two-task pattern uses [`tokio::join!`] rather than
//! [`tokio::spawn`].
//!
//! Streaming + capability filter coverage live in companion test
//! files.
//!
//! Note: `IpcSandbox` was removed in the v0.1 Sandbox port refinement.
//! The new `Sandbox::wrap_spawn` takes `&mut Command` (in-process only)
//! and cannot be proxied over IPC.

#![cfg(feature = "test-support")]

use std::sync::Arc;
use std::time::Duration;

use tau_domain::Value;
use tau_plugin_protocol::test_support::FakeStdioPeer;
use tau_plugin_protocol::{FramedReader, FramedWriter, FramerOptions};
use tau_ports::fixtures::{make_completion_response, make_tool_result, make_tool_spec};
use tau_ports::{CompletionChunk, CompletionRequest, Key, Namespace, StopReason, ToolContent};
use tau_runtime::builder::{DynLlmBackend, DynStorage, DynTool};
use tau_runtime::plugin_host::__internals::{
    DynAsyncWriter, IpcLlmBackend, IpcStorage, IpcTool, PluginProcess,
};
use tokio::io::DuplexStream;

/// Build a [`PluginProcess`] paired with a [`FakeStdioPeer`] via two
/// duplex streams. The returned tuple is `(process, peer)` — the peer
/// drives the test side of the wire while `process` is what the IPC
/// adapter dispatches through.
///
/// We can't reuse [`FakeStdioPeer::new`] directly because its
/// SUT-side writer is `FramedWriter<DuplexStream>` and
/// [`PluginProcess`] expects `FramedWriter<DynAsyncWriter>` — the
/// framer doesn't expose its inner so we build the duplex pair
/// ourselves and box the SUT-side write half. The mirror image of
/// what [`FakeStdioPeer::new`] does internally.
fn paired_process(plugin_name: &str) -> (Arc<PluginProcess>, FakeStdioPeer) {
    let (peer_read_half, sut_write_half) = tokio::io::duplex(64 * 1024);
    let (sut_read_half, peer_write_half) = tokio::io::duplex(64 * 1024);
    let peer = FakeStdioPeer {
        reader: FramedReader::new(peer_read_half, FramerOptions::default()),
        writer: FramedWriter::new(peer_write_half),
    };
    let sut_reader: FramedReader<DuplexStream> =
        FramedReader::new(sut_read_half, FramerOptions::default());
    let sut_writer: FramedWriter<DynAsyncWriter> =
        FramedWriter::new(Box::new(sut_write_half) as DynAsyncWriter);

    let process = PluginProcess::new_for_test(
        plugin_name.to_string(),
        sut_reader,
        sut_writer,
        Duration::from_secs(2),
    );
    (process, peer)
}

#[tokio::test]
async fn ipc_llm_backend_complete_roundtrip_via_fake_peer() {
    let (process, mut peer) = paired_process("echo-llm");
    let backend = IpcLlmBackend::new("echo-llm".to_string(), process);

    let req = CompletionRequest::new("echo-llm".to_string());
    let req_for_assert = req.clone();

    // Drive both sides concurrently. The `BoxFuture<'a, _>` returned
    // by `DynLlmBackend::complete` is not `Send`-bounded, so a
    // `tokio::spawn` would fail; `tokio::join!` polls both futures
    // on the current task instead.
    let call_fut = backend.complete(req);
    let peer_fut = async {
        let (msgid, params_bytes) = peer.expect_request("llm.complete").await;
        assert_eq!(msgid, 2, "first post-handshake msgid is 2");
        let parsed: Vec<CompletionRequest> =
            rmp_serde::from_slice(&params_bytes).expect("params decode");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].model, req_for_assert.model);
        let canned =
            make_completion_response("hello".into(), Vec::new(), StopReason::EndTurn, None);
        peer.send_response(msgid, &canned).await.unwrap();
    };
    let (call_result, ()) = tokio::join!(call_fut, peer_fut);
    let result = call_result.expect("complete should succeed");
    assert_eq!(result.text, "hello");
    assert!(result.tool_uses.is_empty());
}

#[tokio::test]
async fn ipc_llm_backend_complete_maps_error_envelope_to_internal() {
    let (process, mut peer) = paired_process("echo-llm");
    let backend = IpcLlmBackend::new("echo-llm".to_string(), process);

    let req = CompletionRequest::new("echo-llm".to_string());
    let call_fut = backend.complete(req);
    let peer_fut = async {
        let (msgid, _params) = peer.expect_request("llm.complete").await;
        peer.send_response_error(msgid, -32603, "boom")
            .await
            .unwrap();
    };
    let (call_result, ()) = tokio::join!(call_fut, peer_fut);
    let err = call_result.expect_err("expected error");
    let msg = format!("{err}");
    assert!(msg.contains("internal"), "got: {msg}");
    assert!(msg.contains("boom"), "got: {msg}");
}

#[tokio::test]
async fn ipc_llm_backend_stream_assembles_chunks_and_terminates() {
    use futures_util::StreamExt;

    let (process, mut peer) = paired_process("echo-llm");
    let backend = IpcLlmBackend::new("echo-llm".to_string(), process);

    let req = CompletionRequest::new("echo-llm".to_string());
    let req_for_assert = req.clone();

    // Drive both sides concurrently. The streaming path's returned
    // `CompletionStream` is `Send` (per `tau_ports`'s typedef) but the
    // outer `stream()` future is `BoxFuture<'a, _>` not bound `Send`,
    // so `tokio::spawn` would still fail; mirror the complete test's
    // `tokio::join!` pattern.
    let call_fut = async {
        let mut stream = backend
            .stream(req)
            .await
            .expect("stream() returns the assembled stream");
        let mut chunks = Vec::new();
        while let Some(item) = stream.next().await {
            chunks.push(item);
        }
        chunks
    };
    let peer_fut = async {
        let (msgid, params_bytes) = peer.expect_request("llm.stream").await;
        assert_eq!(msgid, 2, "first post-handshake msgid is 2");
        let parsed: Vec<CompletionRequest> =
            rmp_serde::from_slice(&params_bytes).expect("params decode");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].model, req_for_assert.model);

        // Three streamed chunks, then a clean terminating response.
        peer.send_stream_chunk(
            msgid,
            CompletionChunk::Text {
                delta: "alpha".to_string(),
            },
        )
        .await
        .unwrap();
        peer.send_stream_chunk(
            msgid,
            CompletionChunk::Text {
                delta: "beta".to_string(),
            },
        )
        .await
        .unwrap();
        peer.send_stream_chunk(
            msgid,
            CompletionChunk::Text {
                delta: "gamma".to_string(),
            },
        )
        .await
        .unwrap();
        // Empty `()` summary — the host doesn't decode it, only
        // observes the response arrives. Wire shape per spec §4.6 is
        // `{ stop_reason, usage }` but the assembler is summary-shape
        // agnostic.
        peer.send_response(msgid, &()).await.unwrap();
    };

    let (chunks, ()) = tokio::join!(call_fut, peer_fut);
    assert_eq!(chunks.len(), 3, "expected 3 chunks");
    for chunk in &chunks {
        assert!(chunk.is_ok(), "expected Ok chunk, got {chunk:?}");
    }
}

#[tokio::test]
async fn ipc_llm_backend_stream_propagates_mid_stream_error() {
    use futures_util::StreamExt;

    let (process, mut peer) = paired_process("echo-llm");
    let backend = IpcLlmBackend::new("echo-llm".to_string(), process);

    let req = CompletionRequest::new("echo-llm".to_string());

    let call_fut = async {
        let mut stream = backend.stream(req).await.expect("stream() ok");
        let mut items = Vec::new();
        while let Some(item) = stream.next().await {
            items.push(item);
        }
        items
    };
    let peer_fut = async {
        let (msgid, _params_bytes) = peer.expect_request("llm.stream").await;
        // One chunk, then an error envelope as the terminating
        // response. The assembler should yield Ok(chunk) then Err(_)
        // and terminate.
        peer.send_stream_chunk(
            msgid,
            CompletionChunk::Text {
                delta: "partial".to_string(),
            },
        )
        .await
        .unwrap();
        peer.send_response_error(msgid, -32603, "rate limited")
            .await
            .unwrap();
    };

    let (items, ()) = tokio::join!(call_fut, peer_fut);
    assert_eq!(items.len(), 2, "expected one chunk + one error");
    assert!(items[0].is_ok());
    let err = items[1]
        .as_ref()
        .expect_err("second item must be Err for the mid-stream failure");
    let msg = format!("{err}");
    assert!(msg.contains("rate limited"), "got: {msg}");
}

#[tokio::test]
async fn ipc_tool_invoke_roundtrip_via_fake_peer() {
    let (process, mut peer) = paired_process("echo-tool");
    let spec = make_tool_spec(
        "echo".into(),
        "echo".into(),
        Value::Object(Default::default()),
    );
    let tool = IpcTool::new("echo".to_string(), spec.clone(), Vec::new(), process);

    assert_eq!(tool.name(), "echo");
    assert_eq!(tool.schema().name, "echo");
    assert!(tool.capabilities().is_empty());

    // init/teardown are no-ops on the IPC side — they don't roundtrip.
    let session_ctx = tau_ports::SessionContext::new(
        tau_domain::AgentInstanceId::new(),
        uuid::Uuid::new_v4(),
        None,
    );
    tool.init(session_ctx).await.expect("init no-op");

    let arg = Value::String("hi".into());
    let arg_for_assert = arg.clone();
    let mut session = ();
    let call_fut = tool.invoke(&mut session, arg);
    let peer_fut = async {
        let (msgid, params_bytes) = peer.expect_request("tool.call").await;
        let (_ctx, decoded_args): (tau_ports::SessionContext, Value) =
            rmp_serde::from_slice(&params_bytes).expect("params decode (SessionContext, Value)");
        assert_eq!(decoded_args, arg_for_assert);

        let result = make_tool_result(
            vec![ToolContent::Text {
                text: "echoed".into(),
            }],
            false,
        );
        peer.send_response(msgid, &result).await.unwrap();
    };
    let (call_result, ()) = tokio::join!(call_fut, peer_fut);
    let outcome = call_result.expect("invoke should succeed");
    assert!(!outcome.is_error);
    assert_eq!(outcome.content.len(), 1);

    tool.teardown(()).await.expect("teardown no-op");
}

#[tokio::test]
async fn ipc_storage_get_put_delete_list_roundtrip_via_fake_peer() {
    let (process, mut peer) = paired_process("memstore");
    let storage = IpcStorage::new("memstore".to_string(), process);

    assert_eq!(DynStorage::name(&storage), "memstore");

    let ns = Namespace::try_new("test").unwrap();
    let key = Key::try_new("k1").unwrap();

    // get
    let get_fut = storage.get(&ns, &key);
    let peer_fut = async {
        let (msgid, _params) = peer.expect_request("storage.get").await;
        peer.send_response(msgid, &Some::<Vec<u8>>(b"value".to_vec()))
            .await
            .unwrap();
    };
    let (got, ()) = tokio::join!(get_fut, peer_fut);
    let got = got.expect("get ok");
    assert_eq!(got, Some(b"value".to_vec()));

    // put
    let put_fut = storage.put(&ns, &key, b"data");
    let peer_fut = async {
        let (msgid, _params) = peer.expect_request("storage.put").await;
        peer.send_response(msgid, &()).await.unwrap();
    };
    let (put_result, ()) = tokio::join!(put_fut, peer_fut);
    put_result.expect("put ok");

    // delete
    let del_fut = storage.delete(&ns, &key);
    let peer_fut = async {
        let (msgid, _params) = peer.expect_request("storage.delete").await;
        peer.send_response(msgid, &true).await.unwrap();
    };
    let (deleted, ()) = tokio::join!(del_fut, peer_fut);
    let deleted = deleted.expect("delete ok");
    assert!(deleted);

    // list
    let list_fut = storage.list(&ns, "k");
    let peer_fut = async {
        let (msgid, _params) = peer.expect_request("storage.list").await;
        let keys = vec![Key::try_new("k1").unwrap(), Key::try_new("k2").unwrap()];
        peer.send_response(msgid, &keys).await.unwrap();
    };
    let (listed, ()) = tokio::join!(list_fut, peer_fut);
    let listed = listed.expect("list ok");
    assert_eq!(listed.len(), 2);
}
