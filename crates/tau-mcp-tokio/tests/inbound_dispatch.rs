//! Tests for the inbound-dispatch task.
//!
//! Uses a custom Transport impl over `tokio::sync::mpsc` channels so
//! the test fully controls inbound feed + outbound assert. Avoids the
//! cassette transport, which is read-only.

use std::pin::Pin;
use std::sync::Arc;

use tau_mcp::host::handlers::DefaultDenyHandlers;
use tau_mcp::protocol::jsonrpc::{
    JsonRpcMessage, JsonRpcRequest, JsonRpcResponse, RequestId, JSONRPC_VERSION,
};
use tau_mcp::transport::Transport;
use tau_mcp::McpError;
use tau_mcp_tokio::host_lifecycle::{spawn_inbound_dispatch, INBOUND_REFUSED_ERROR_CODE};
use tokio::sync::{mpsc, Mutex};

/// Bidirectional mpsc-backed Transport for tests.
struct MpscTransport {
    inbound: Mutex<mpsc::UnboundedReceiver<JsonRpcMessage>>,
    outbound: mpsc::UnboundedSender<JsonRpcMessage>,
}

impl Transport for MpscTransport {
    fn send_message<'a>(
        &'a self,
        msg: &'a JsonRpcMessage,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), McpError>> + Send + 'a>> {
        let msg = msg.clone();
        let tx = self.outbound.clone();
        Box::pin(async move {
            tx.send(msg)
                .map_err(|_| McpError::Transport("outbound channel closed".into()))?;
            Ok(())
        })
    }

    fn next_message<'a>(
        &'a self,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<Option<JsonRpcMessage>, McpError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut rx = self.inbound.lock().await;
            Ok(rx.recv().await)
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_deny_sampling_yields_jsonrpc_error_with_code_neg_32000() {
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
    let transport: Arc<dyn Transport> = Arc::new(MpscTransport {
        inbound: Mutex::new(inbound_rx),
        outbound: outbound_tx,
    });

    // Feed a sampling/createMessage request to the inbound side.
    let req = JsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: RequestId::Number(99),
        method: "sampling/createMessage".to_string(),
        params: Some(serde_json::json!({
            "messages": [{"role": "user", "content": {"type": "text", "text": "x"}}],
            "modelPreferences": {}
        })),
    });
    inbound_tx.send(req).unwrap();

    let _handle = spawn_inbound_dispatch(transport, Arc::new(DefaultDenyHandlers));

    // The pump should write a JsonRpcResponse back.
    let resp = tokio::time::timeout(std::time::Duration::from_secs(2), outbound_rx.recv())
        .await
        .expect("response written within 2s")
        .expect("channel still open");

    let JsonRpcMessage::Response(JsonRpcResponse {
        id, result, error, ..
    }) = resp
    else {
        panic!("expected Response, got {resp:?}");
    };
    assert_eq!(id, RequestId::Number(99));
    assert!(result.is_none(), "default-deny should set result=None");
    let err = error.expect("default-deny should set error");
    assert_eq!(err.code, INBOUND_REFUSED_ERROR_CODE);
    assert!(
        err.message.contains("sampling"),
        "error message mentions sampling: {}",
        err.message
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_deny_roots_returns_empty_list() {
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
    let transport: Arc<dyn Transport> = Arc::new(MpscTransport {
        inbound: Mutex::new(inbound_rx),
        outbound: outbound_tx,
    });

    let req = JsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: RequestId::Number(7),
        method: "roots/list".to_string(),
        params: None,
    });
    inbound_tx.send(req).unwrap();

    let _handle = spawn_inbound_dispatch(transport, Arc::new(DefaultDenyHandlers));

    let resp = tokio::time::timeout(std::time::Duration::from_secs(2), outbound_rx.recv())
        .await
        .expect("response written within 2s")
        .expect("channel still open");

    let JsonRpcMessage::Response(JsonRpcResponse {
        id, result, error, ..
    }) = resp
    else {
        panic!("expected Response, got {resp:?}");
    };
    assert_eq!(id, RequestId::Number(7));
    assert!(error.is_none(), "roots/list should succeed: {error:?}");
    let result = result.expect("roots/list should return result");
    let roots = result
        .get("roots")
        .and_then(|v| v.as_array())
        .expect("result.roots is an array");
    assert!(roots.is_empty(), "default-deny roots returns []");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_method_yields_jsonrpc_error() {
    let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel();
    let transport: Arc<dyn Transport> = Arc::new(MpscTransport {
        inbound: Mutex::new(inbound_rx),
        outbound: outbound_tx,
    });

    let req = JsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: RequestId::Number(1),
        method: "future/method".to_string(),
        params: None,
    });
    inbound_tx.send(req).unwrap();

    let _handle = spawn_inbound_dispatch(transport, Arc::new(DefaultDenyHandlers));

    let resp = tokio::time::timeout(std::time::Duration::from_secs(2), outbound_rx.recv())
        .await
        .expect("response written within 2s")
        .expect("channel still open");

    let JsonRpcMessage::Response(JsonRpcResponse { error, .. }) = resp else {
        panic!("expected Response");
    };
    let err = error.expect("error variant");
    assert!(err.message.contains("unsupported server-initiated"));
}
