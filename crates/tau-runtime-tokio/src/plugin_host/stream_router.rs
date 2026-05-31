//! Assemble `stream.chunk` notifications + a terminating `Frame::Response`
//! into a [`tau_ports::CompletionStream`] that the runtime consumes
//! identically to an in-process [`tau_ports::LlmBackend`].
//!
//! Each [`tau_ports::CompletionChunk`] arrives via an mpsc channel
//! populated by the plugin-host read-loop in
//! [`crate::plugin_host::process`] (the loop routes `stream.chunk`
//! notifications into the per-stream `mpsc::Sender` registered under
//! [`crate::plugin_host::process::PluginProcess::in_flight_streams`]).
//! The originating `llm.stream` request's response arrives via a
//! oneshot, terminating the stream cleanly on `Ok` or with
//! [`tau_ports::LlmError::Internal`] on `Err`.
//!
//! See spec §4.6 (streaming wire shape) and §7.4 (per-port adapters).

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;
use tau_plugin_protocol::error::RpcErrorEnvelope;
use tau_ports::{CompletionChunk, CompletionStream, LlmError};
use tokio::sync::{mpsc, oneshot};

use crate::plugin_host::process::RpcResult;

/// Build a [`CompletionStream`] from a chunk channel + a final-response
/// oneshot. The returned stream yields one `Result<CompletionChunk, _>`
/// per `stream.chunk` notification received; it terminates when the
/// originating `llm.stream` request's `Frame::Response` arrives, with
/// either:
///
/// - a clean `None` if the response carried no error, or
/// - one final `Some(Err(_))` followed by `None` if the response
///   carried an error envelope (mid-stream plugin failure).
///
/// If the read loop drops the chunk sender + oneshot sender without a
/// response (plugin EOF mid-stream), the stream yields one synthetic
/// `Internal` error and terminates.
pub(crate) fn assemble(
    chunk_rx: mpsc::Receiver<CompletionChunk>,
    final_rx: oneshot::Receiver<RpcResult>,
) -> CompletionStream {
    Box::pin(StreamRouter::new(chunk_rx, final_rx))
}

/// State machine for the assembled stream. `done` flips to `true` once
/// the stream has yielded its terminal item (or once the final response
/// has been consumed without producing one), so subsequent polls return
/// `Poll::Ready(None)` without re-entering the channel logic.
struct StreamRouter {
    chunk_rx: mpsc::Receiver<CompletionChunk>,
    final_rx: Option<oneshot::Receiver<RpcResult>>,
    done: bool,
}

impl StreamRouter {
    fn new(
        chunk_rx: mpsc::Receiver<CompletionChunk>,
        final_rx: oneshot::Receiver<RpcResult>,
    ) -> Self {
        Self {
            chunk_rx,
            final_rx: Some(final_rx),
            done: false,
        }
    }

    fn poll_final(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<CompletionChunk, LlmError>>> {
        match self.final_rx.as_mut() {
            Some(final_rx) => match Pin::new(final_rx).poll(cx) {
                Poll::Ready(Ok(Ok(_summary))) => {
                    // Plugin signalled clean stream termination. The
                    // summary bytes (`{ stop_reason, usage }` per spec
                    // §4.6) are intentionally ignored here: the
                    // streaming contract terminates the stream as the
                    // signal, and any caller that needs a summary
                    // should consume one via `CompletionChunk::Finish`
                    // (which the SDK runner emits as the last chunk
                    // before sending the response).
                    self.done = true;
                    self.final_rx = None;
                    Poll::Ready(None)
                }
                Poll::Ready(Ok(Err(envelope))) => {
                    self.done = true;
                    self.final_rx = None;
                    Poll::Ready(Some(Err(map_envelope(envelope))))
                }
                Poll::Ready(Err(_)) => {
                    // The oneshot sender was dropped before sending.
                    // This happens if the read loop exits (plugin
                    // crashed / stdout EOF) before the response arrives.
                    self.done = true;
                    self.final_rx = None;
                    Poll::Ready(Some(Err(LlmError::Internal {
                        message: "stream final-response sender dropped (plugin crashed?)"
                            .to_string(),
                    })))
                }
                Poll::Pending => Poll::Pending,
            },
            None => {
                // Final response already consumed (e.g. by an earlier
                // poll that observed `Ready(Ok(Ok(_)))` then returned
                // `None`). Subsequent polls just keep returning `None`.
                self.done = true;
                Poll::Ready(None)
            }
        }
    }
}

impl Stream for StreamRouter {
    type Item = Result<CompletionChunk, LlmError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }
        // Drain any waiting chunks first. `mpsc::Receiver::poll_recv`
        // yields chunks until every sender is dropped; the read loop
        // holds the sender and drops it on plugin EOF or after the
        // process is torn down.
        match self.chunk_rx.poll_recv(cx) {
            Poll::Ready(Some(chunk)) => Poll::Ready(Some(Ok(chunk))),
            // Chunk channel closed: the only way to terminate the
            // stream is via the final response. Fall through to
            // polling it.
            Poll::Ready(None) => self.poll_final(cx),
            // No chunk ready: race the final response. If it's also
            // pending, both wakers are now registered with `cx` so we
            // get re-polled when either arrives.
            Poll::Pending => self.poll_final(cx),
        }
    }
}

/// Map an [`RpcErrorEnvelope`] from the wire onto the typed
/// [`LlmError`] surface. The host crate doesn't have a richer `LlmError`
/// taxonomy yet (see the plan-erratum note in the design doc — adding
/// new variants is deferred until at least one caller needs to
/// discriminate), so for now every envelope shape collapses into
/// `Internal`.
fn map_envelope(envelope: RpcErrorEnvelope) -> LlmError {
    LlmError::Internal {
        message: format!(
            "plugin error code {} message {}",
            envelope.code, envelope.message
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use tau_ports::CompletionChunk;

    #[tokio::test]
    async fn assembles_chunks_and_terminates_cleanly() {
        let (chunk_tx, chunk_rx) = mpsc::channel::<CompletionChunk>(8);
        let (final_tx, final_rx) = oneshot::channel::<RpcResult>();
        // `collect()` consumes the stream by value; no `mut` needed.
        let stream = assemble(chunk_rx, final_rx);

        // Send 2 chunks, then drop the chunk sender + send a clean
        // terminating response.
        chunk_tx
            .send(CompletionChunk::Text {
                delta: "hello ".to_string(),
            })
            .await
            .unwrap();
        chunk_tx
            .send(CompletionChunk::Text {
                delta: "world".to_string(),
            })
            .await
            .unwrap();
        drop(chunk_tx);
        final_tx.send(Ok(Vec::new())).unwrap();

        let chunks: Vec<_> = stream.collect().await;
        assert_eq!(chunks.len(), 2);
        for chunk in chunks {
            assert!(chunk.is_ok(), "chunk should be Ok");
        }
    }

    #[tokio::test]
    async fn final_error_propagates_as_err_then_terminates() {
        let (chunk_tx, chunk_rx) = mpsc::channel::<CompletionChunk>(8);
        let (final_tx, final_rx) = oneshot::channel::<RpcResult>();
        let mut stream = assemble(chunk_rx, final_rx);

        let envelope = RpcErrorEnvelope::new(
            tau_plugin_protocol::error::INTERNAL_ERROR,
            "rate limited".to_string(),
            None,
        );
        // Drop the chunk sender so the receiver closes; otherwise the
        // mpsc poll stays Pending and the test hangs.
        drop(chunk_tx);
        final_tx.send(Err(envelope)).unwrap();

        let mut count = 0;
        let mut got_err = false;
        while let Some(item) = stream.next().await {
            count += 1;
            assert!(item.is_err(), "expected the single yielded item to be Err");
            got_err = true;
        }
        assert!(got_err, "expected one Err item");
        assert_eq!(count, 1, "stream must terminate after the single Err");
    }

    #[tokio::test]
    async fn dropped_oneshot_sender_yields_synthetic_internal_error() {
        let (chunk_tx, chunk_rx) = mpsc::channel::<CompletionChunk>(8);
        let (final_tx, final_rx) = oneshot::channel::<RpcResult>();
        let mut stream = assemble(chunk_rx, final_rx);

        // Simulate the read loop exiting mid-stream: drop both
        // the chunk sender and the oneshot sender without sending a
        // response. The router should yield exactly one synthetic
        // `Internal` error and terminate.
        drop(chunk_tx);
        drop(final_tx);

        let mut count = 0;
        while let Some(item) = stream.next().await {
            count += 1;
            assert!(item.is_err());
        }
        assert_eq!(count, 1);
    }
}
