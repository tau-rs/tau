//! Line-delimited JSON-RPC framer over async I/O.
//!
//! Per MCP stdio transport spec: every JSON-RPC message is one JSON
//! object terminated by `\n`. The framer reads lines from an
//! `AsyncBufRead` and writes them to an `AsyncWrite`, deserializing
//! / serializing through `JsonRpcMessage`.

use tau_mcp::protocol::JsonRpcMessage;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::transport_stdio::error::StdioTransportError;

/// Line-delimited JSON-RPC framer.
pub struct JsonLineFramer<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    reader: BufReader<R>,
    writer: W,
    line_buf: String,
}

impl<R, W> JsonLineFramer<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    /// Construct a framer over the given reader+writer.
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            line_buf: String::new(),
        }
    }

    /// Read one MCP message. Returns `Ok(None)` on EOF (clean close).
    pub async fn read_message(&mut self) -> Result<Option<JsonRpcMessage>, StdioTransportError> {
        self.line_buf.clear();
        let n = self.reader.read_line(&mut self.line_buf).await?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = self.line_buf.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            // Skip blank lines as a robustness measure.
            return self.read_message_boxed().await;
        }
        let msg: JsonRpcMessage = serde_json::from_str(trimmed)?;
        Ok(Some(msg))
    }

    fn read_message_boxed<'a>(
        &'a mut self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<JsonRpcMessage>, StdioTransportError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(self.read_message())
    }

    /// Write one MCP message followed by `\n`.
    pub async fn write_message(&mut self, msg: &JsonRpcMessage) -> Result<(), StdioTransportError> {
        let bytes = serde_json::to_vec(msg)?;
        self.writer.write_all(&bytes).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_mcp::protocol::jsonrpc::{
        JsonRpcMessage, JsonRpcRequest, JsonRpcResponse, RequestId, JSONRPC_VERSION,
    };
    use tokio::io::duplex;

    #[tokio::test(flavor = "current_thread")]
    async fn write_then_read_round_trips_request() {
        let (peer_r, mut peer_w) = duplex(4096);
        let (mut my_r, _my_w) = duplex(4096);

        // Use peer's write as MY read.
        let msg = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(7),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name":"echo"})),
        });

        let bytes = serde_json::to_vec(&msg).unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut peer_w, &bytes)
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut peer_w, b"\n")
            .await
            .unwrap();
        drop(peer_w);

        let mut framer = JsonLineFramer::new(peer_r, &mut my_r);
        let received = framer.read_message().await.unwrap().expect("got a message");
        assert_eq!(received, msg);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn eof_returns_none() {
        let (peer_r, peer_w) = duplex(4096);
        let (mut my_r, _my_w) = duplex(4096);
        drop(peer_w); // EOF immediately

        let mut framer = JsonLineFramer::new(peer_r, &mut my_r);
        let received = framer.read_message().await.unwrap();
        assert!(
            received.is_none(),
            "EOF should yield Ok(None), got {received:?}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn malformed_json_errors() {
        let (peer_r, mut peer_w) = duplex(4096);
        let (mut my_r, _my_w) = duplex(4096);
        tokio::io::AsyncWriteExt::write_all(&mut peer_w, b"not json\n")
            .await
            .unwrap();
        drop(peer_w);

        let mut framer = JsonLineFramer::new(peer_r, &mut my_r);
        let err = framer.read_message().await.expect_err("should error");
        assert!(matches!(err, StdioTransportError::Json(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn skip_blank_lines_then_read_message() {
        let (peer_r, mut peer_w) = duplex(4096);
        let (mut my_r, _my_w) = duplex(4096);

        let msg = JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(1),
            result: Some(serde_json::json!({"ok":true})),
            error: None,
        });
        let bytes = serde_json::to_vec(&msg).unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut peer_w, b"\n\n")
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut peer_w, &bytes)
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut peer_w, b"\n")
            .await
            .unwrap();
        drop(peer_w);

        let mut framer = JsonLineFramer::new(peer_r, &mut my_r);
        let received = framer.read_message().await.unwrap().expect("got a message");
        assert_eq!(received, msg);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn write_message_writes_one_line_with_trailing_newline() {
        let (mut peer_r, peer_w) = duplex(4096);
        let (_my_r, my_w) = duplex(4096);
        let mut framer = JsonLineFramer::new(_my_r, peer_w);

        let msg = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(0),
            method: "initialize".to_string(),
            params: None,
        });
        framer.write_message(&msg).await.unwrap();
        // Drop the framer to close peer_w so read_to_end below sees EOF.
        drop(framer);
        drop(my_w);

        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut peer_r, &mut buf)
            .await
            .unwrap();
        let s = std::str::from_utf8(&buf).unwrap();
        assert!(s.ends_with('\n'));
        assert_eq!(s.matches('\n').count(), 1);
    }
}
