//! Helper: turn a `Stream<Item = Result<CompletionChunk, LlmError>>`
//! into a series of `stream.chunk` notifications carrying the
//! originating msgid, plus a final [`Frame::Response`] carrying
//! `{ stop_reason, usage }` or an error envelope.
//!
//! See `docs/superpowers/specs/2026-04-28-plugin-loading-design.md`
//! §4.6 for the wire-level streaming protocol.

use futures_util::StreamExt;
use serde::Serialize;
use tau_plugin_protocol::{
    error::{RpcErrorEnvelope, INTERNAL_ERROR},
    Frame, FramedWriter,
};
use tokio::io::AsyncWrite;
use tokio::pin;

use crate::error::SdkError;

/// Final summary serialized into the terminal `Frame::Response::result`
/// at the end of a streaming dispatch.
#[derive(Debug, Clone, Serialize)]
struct StreamSummary {
    stop_reason: Option<tau_ports::StopReason>,
    usage: Option<tau_ports::TokenUsage>,
}

/// Pump an LLM completion stream out as `stream.chunk` notifications,
/// terminating with a [`Frame::Response`] carrying the final summary
/// or an error envelope.
///
/// The `msgid` is the originating `llm.stream` request id.
/// Each [`tau_ports::CompletionChunk`] yielded by the stream is encoded
/// as the second element of a `(msgid, chunk)` `stream.chunk`
/// notification. When the stream ends cleanly, a single
/// [`Frame::Response`] is sent carrying a JSON-shaped summary payload
/// (`{ stop_reason, usage }`); both fields default to `None` if no
/// terminal [`tau_ports::CompletionChunk::Finish`] was observed. If
/// the stream yields an error, that final response carries an error
/// envelope with code [`INTERNAL_ERROR`] and the dispatch terminates
/// without exhausting any further items.
///
/// # Example
///
/// ```no_run
/// # use futures_util::stream;
/// # use tau_ports::{CompletionChunk, CompletionStream, LlmError, StopReason, TokenUsage};
/// # use tau_plugin_protocol::{FramedWriter};
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let stdout = tokio::io::stdout();
///     let mut writer = FramedWriter::new(stdout);
///     // Build a minimal two-chunk stream: one text chunk + one finish.
///     let chunks: CompletionStream = Box::pin(stream::iter(vec![
///         Ok(CompletionChunk::Text { delta: "hello".to_string() }),
///         Ok(CompletionChunk::Finish {
///             stop_reason: StopReason::EndTurn,
///             usage: None,
///         }),
///     ]));
///     tau_plugin_sdk::stream_completion(&mut writer, 1, chunks).await?;
///     Ok(())
/// }
/// ```
pub async fn stream_completion<W>(
    writer: &mut FramedWriter<W>,
    msgid: u32,
    stream: tau_ports::CompletionStream,
) -> Result<(), SdkError>
where
    W: AsyncWrite + Unpin,
{
    pin!(stream);
    let mut final_stop_reason: Option<tau_ports::StopReason> = None;
    let mut final_usage: Option<tau_ports::TokenUsage> = None;

    while let Some(item) = stream.next().await {
        match item {
            Ok(chunk) => {
                // Capture the terminal Finish marker for the final summary.
                if let tau_ports::CompletionChunk::Finish { stop_reason, usage } = &chunk {
                    final_stop_reason = Some(*stop_reason);
                    final_usage = *usage;
                }
                // Forward the chunk as a `stream.chunk` notification carrying
                // the originating msgid + the chunk itself.
                let params_bytes = rmp_serde::to_vec(&(msgid, &chunk))?;
                let frame = Frame::Notification {
                    method: "stream.chunk".to_string(),
                    params: params_bytes,
                };
                writer.write_frame(&frame.encode()?).await?;
            }
            Err(llm_err) => {
                // Stream errored mid-flight: terminate with an error response.
                let envelope = RpcErrorEnvelope::new(
                    INTERNAL_ERROR,
                    format!("llm stream error: {llm_err}"),
                    None,
                );
                let frame = Frame::Response {
                    id: msgid,
                    error: Some(envelope),
                    result: None,
                };
                writer.write_frame(&frame.encode()?).await?;
                return Ok(());
            }
        }
    }

    // Stream ended cleanly: send the final summary response.
    let summary = StreamSummary {
        stop_reason: final_stop_reason,
        usage: final_usage,
    };
    let result_bytes = rmp_serde::to_vec(&summary)?;
    let frame = Frame::Response {
        id: msgid,
        error: None,
        result: Some(result_bytes),
    };
    writer.write_frame(&frame.encode()?).await?;
    Ok(())
}
