//! Integration tests for `tau-runtime`'s host-side handshake driver.
//!
//! Drives [`tau_runtime_tokio::plugin_host::__internals::drive_handshake`]
//! against a [`FakeStdioPeer`] for each
//! [`HandshakeFailureReason`] variant plus the happy path.

use std::collections::BTreeMap;
use std::time::Duration;

use tau_domain::PortKind;
use tau_plugin_protocol::handshake::meta;
use tau_plugin_protocol::test_support::FakeStdioPeer;
use tau_plugin_protocol::{
    HandshakeRequest, HandshakeResponse, MethodSchema, TraceContext, PROTOCOL_VERSION,
};
use tau_runtime_tokio::error::{CoreRuntimeError, HandshakeFailureReason};
use tau_runtime_tokio::plugin_host::__internals::drive_handshake;
use tau_runtime_tokio::RuntimeError;

use assert_matches::assert_matches;

fn trace_context() -> TraceContext {
    TraceContext::new(
        "run-1".to_string(),
        "agent-1".to_string(),
        "span-1".to_string(),
    )
}

fn schemas_for(methods: &[&str]) -> BTreeMap<String, MethodSchema> {
    let mut out = BTreeMap::new();
    let empty = MethodSchema::new(serde_json::json!({}), serde_json::json!({}));
    for m in methods {
        out.insert((*m).to_string(), empty.clone());
    }
    out
}

#[tokio::test]
async fn happy_path_returns_validated_handshake_response() {
    let (mut peer, mut sut_reader, mut sut_writer) = FakeStdioPeer::new();

    let task = tokio::spawn(async move {
        drive_handshake(
            &mut sut_reader,
            &mut sut_writer,
            "echo-llm",
            PortKind::LlmBackend,
            &["llm.complete", "llm.stream"],
            serde_json::Value::Null,
            trace_context(),
            Duration::from_secs(5),
        )
        .await
    });

    let (msgid, request) = peer.expect_handshake().await;
    assert_eq!(msgid, 1);
    assert_eq!(request.protocol_version, PROTOCOL_VERSION);
    assert_eq!(request.port, PortKind::LlmBackend);

    let response = HandshakeResponse::new(
        PROTOCOL_VERSION.to_string(),
        PortKind::LlmBackend,
        "echo-llm".to_string(),
        "0.1.0".to_string(),
        vec!["llm.complete".to_string(), "llm.stream".to_string()],
        schemas_for(&["llm.complete", "llm.stream"]),
    );
    peer.send_handshake_response(msgid, response.clone())
        .await
        .unwrap();

    let result = task.await.unwrap();
    let actual = result.expect("handshake should succeed");
    assert_eq!(actual.plugin_name, "echo-llm");
    assert_eq!(actual.provides, PortKind::LlmBackend);
}

#[tokio::test]
async fn protocol_version_mismatch_surfaces_typed_reason() {
    let (mut peer, mut sut_reader, mut sut_writer) = FakeStdioPeer::new();

    let task = tokio::spawn(async move {
        drive_handshake(
            &mut sut_reader,
            &mut sut_writer,
            "echo-llm",
            PortKind::LlmBackend,
            &["llm.complete"],
            serde_json::Value::Null,
            trace_context(),
            Duration::from_secs(5),
        )
        .await
    });

    let (msgid, _req) = peer.expect_handshake().await;
    let response = HandshakeResponse::new(
        // Different protocol version than the host's PROTOCOL_VERSION.
        "999".to_string(),
        PortKind::LlmBackend,
        "echo-llm".to_string(),
        "0.1.0".to_string(),
        vec!["llm.complete".to_string()],
        schemas_for(&["llm.complete"]),
    );
    peer.send_handshake_response(msgid, response).await.unwrap();

    let result = task.await.unwrap();
    let err = result.expect_err("handshake should fail");
    assert_matches!(
        err,
        RuntimeError::Core(CoreRuntimeError::PluginHandshakeFailed { plugin, reason }) => {
            assert_eq!(plugin, "echo-llm");
            assert!(
                matches!(
                    reason,
                    HandshakeFailureReason::ProtocolVersionMismatch { .. }
                ),
                "expected ProtocolVersionMismatch, got {reason:?}"
            );
        }
    );
}

#[tokio::test]
async fn provides_mismatch_surfaces_typed_reason() {
    let (mut peer, mut sut_reader, mut sut_writer) = FakeStdioPeer::new();

    let task = tokio::spawn(async move {
        drive_handshake(
            &mut sut_reader,
            &mut sut_writer,
            "echo-llm",
            PortKind::LlmBackend,
            &["llm.complete"],
            serde_json::Value::Null,
            trace_context(),
            Duration::from_secs(5),
        )
        .await
    });

    let (msgid, _req) = peer.expect_handshake().await;
    let response = HandshakeResponse::new(
        PROTOCOL_VERSION.to_string(),
        // Plugin advertises Tool, host expected LlmBackend.
        PortKind::Tool,
        "echo-llm".to_string(),
        "0.1.0".to_string(),
        vec!["llm.complete".to_string()],
        schemas_for(&["llm.complete"]),
    );
    peer.send_handshake_response(msgid, response).await.unwrap();

    let result = task.await.unwrap();
    let err = result.expect_err("handshake should fail");
    assert_matches!(
        err,
        RuntimeError::Core(CoreRuntimeError::PluginHandshakeFailed {
            reason: HandshakeFailureReason::ProvidesMismatch {
                manifest,
                plugin_advertised,
            },
            ..
        }) => {
            assert_eq!(manifest, PortKind::LlmBackend);
            assert_eq!(plugin_advertised, PortKind::Tool);
        }
    );
}

#[tokio::test]
async fn missing_required_method_surfaces_typed_reason() {
    let (mut peer, mut sut_reader, mut sut_writer) = FakeStdioPeer::new();

    let task = tokio::spawn(async move {
        drive_handshake(
            &mut sut_reader,
            &mut sut_writer,
            "echo-llm",
            PortKind::LlmBackend,
            &["llm.complete", "llm.stream"],
            serde_json::Value::Null,
            trace_context(),
            Duration::from_secs(5),
        )
        .await
    });

    let (msgid, _req) = peer.expect_handshake().await;
    let response = HandshakeResponse::new(
        PROTOCOL_VERSION.to_string(),
        PortKind::LlmBackend,
        "echo-llm".to_string(),
        "0.1.0".to_string(),
        // Missing `llm.stream`.
        vec!["llm.complete".to_string()],
        schemas_for(&["llm.complete"]),
    );
    peer.send_handshake_response(msgid, response).await.unwrap();

    let result = task.await.unwrap();
    let err = result.expect_err("handshake should fail");
    assert_matches!(
        err,
        RuntimeError::Core(CoreRuntimeError::PluginHandshakeFailed {
            reason: HandshakeFailureReason::MissingRequiredMethod { method },
            ..
        }) => {
            assert_eq!(method, "llm.stream");
        }
    );
}

#[tokio::test]
async fn timeout_surfaces_typed_reason() {
    let (mut peer, mut sut_reader, mut sut_writer) = FakeStdioPeer::new();

    let task = tokio::spawn(async move {
        drive_handshake(
            &mut sut_reader,
            &mut sut_writer,
            "echo-llm",
            PortKind::LlmBackend,
            &["llm.complete"],
            serde_json::Value::Null,
            trace_context(),
            // Tiny timeout: the peer never responds, so the timeout
            // fires before any frame arrives.
            Duration::from_millis(50),
        )
        .await
    });

    // Drain the request so the peer's transport stays alive until the
    // task observes its timeout. Otherwise dropping `peer` on test
    // exit would EOF the SUT and the timeout race becomes ambiguous.
    let (_msgid, _req) = peer.expect_handshake().await;

    let result = task.await.unwrap();
    let err = result.expect_err("handshake should fail");
    assert_matches!(
        err,
        RuntimeError::Core(CoreRuntimeError::PluginHandshakeFailed { reason, .. }) => {
            assert!(
                matches!(reason, HandshakeFailureReason::Timeout),
                "expected Timeout, got {reason:?}"
            );
        }
    );
    // Keep peer alive until after the assertion.
    drop(peer);
}

#[tokio::test]
async fn eof_before_response_surfaces_malformed() {
    let (mut peer, mut sut_reader, mut sut_writer) = FakeStdioPeer::new();

    let task = tokio::spawn(async move {
        drive_handshake(
            &mut sut_reader,
            &mut sut_writer,
            "echo-llm",
            PortKind::LlmBackend,
            &["llm.complete"],
            serde_json::Value::Null,
            trace_context(),
            Duration::from_secs(5),
        )
        .await
    });

    // Read the handshake request, then drop the peer to close both
    // halves of the duplex transport: the SUT sees EOF on its next
    // frame read.
    let (_msgid, _req) = peer.expect_handshake().await;
    drop(peer);

    let result = task.await.unwrap();
    let err = result.expect_err("handshake should fail");
    assert_matches!(
        err,
        RuntimeError::Core(CoreRuntimeError::PluginHandshakeFailed {
            reason: HandshakeFailureReason::Malformed { detail },
            ..
        }) => {
            assert!(
                detail.contains("EOF") || detail.contains("eof"),
                "expected EOF detail, got: {detail}"
            );
        }
    );
}

#[tokio::test]
async fn plugin_error_envelope_surfaces_malformed() {
    // Plugin replies with a populated `error` envelope rather than a
    // structured `result`. The host treats this as a structurally
    // invalid handshake (plugins must respond positively to the
    // handshake; failures route via the typed reasons, not envelopes).
    let (mut peer, mut sut_reader, mut sut_writer) = FakeStdioPeer::new();

    let task = tokio::spawn(async move {
        drive_handshake(
            &mut sut_reader,
            &mut sut_writer,
            "echo-llm",
            PortKind::LlmBackend,
            &["llm.complete"],
            serde_json::Value::Null,
            trace_context(),
            Duration::from_secs(5),
        )
        .await
    });

    let (msgid, _req) = peer.expect_handshake().await;
    peer.send_response_error(msgid, -32600, "invalid request")
        .await
        .unwrap();

    let result = task.await.unwrap();
    let err = result.expect_err("handshake should fail");
    assert_matches!(
        err,
        RuntimeError::Core(CoreRuntimeError::PluginHandshakeFailed {
            reason: HandshakeFailureReason::Malformed { detail },
            ..
        }) => {
            assert!(
                detail.contains("error envelope"),
                "expected error-envelope detail, got: {detail}"
            );
        }
    );
}

#[tokio::test]
async fn handshake_request_contains_supplied_config_and_trace_context() {
    let (mut peer, mut sut_reader, mut sut_writer) = FakeStdioPeer::new();
    let config = serde_json::json!({"api_key": "sk-...", "model": "test-1"});
    let config_for_task = config.clone();

    let task = tokio::spawn(async move {
        drive_handshake(
            &mut sut_reader,
            &mut sut_writer,
            "echo-llm",
            PortKind::LlmBackend,
            &["llm.complete"],
            config_for_task,
            TraceContext::new("R".into(), "A".into(), "S".into()),
            Duration::from_secs(5),
        )
        .await
    });

    let (msgid, request): (u32, HandshakeRequest) = peer.expect_handshake().await;
    assert_eq!(request.protocol_version, PROTOCOL_VERSION);
    assert_eq!(request.port, PortKind::LlmBackend);
    assert_eq!(request.config, config);
    assert_eq!(request.trace_context.run_id, "R");
    assert_eq!(request.trace_context.agent_id, "A");
    assert_eq!(request.trace_context.root_span_id, "S");

    // Reply so the task terminates.
    let response = HandshakeResponse::new(
        PROTOCOL_VERSION.to_string(),
        PortKind::LlmBackend,
        "echo-llm".to_string(),
        "0.1.0".to_string(),
        vec!["llm.complete".to_string()],
        schemas_for(&["llm.complete"]),
    );
    peer.send_handshake_response(msgid, response).await.unwrap();

    let _ = task.await.unwrap().expect("handshake should succeed");
    // Use the meta module re-export to keep the import alive.
    let _ = meta::HANDSHAKE_METHOD;
}
