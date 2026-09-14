//! One model call: `send` the request, `recv` the reply, `read` its bytes.

use tau_kernel::abi::{BlobRef, Capability, Corr, Msg, MsgKind};
use tau_kernel::bridge::{ModelReply, ModelRequest, VERSION};
use tau_kernel::kernel::KernelError;
use tau_kernel::syscall::{Handle, Match};

/// Why [`infer`] did not return a reply.
///
/// Every variant is terminal for the call. In particular a `send` the kernel
/// refuses — the ceiling cannot be reserved, the capability is not held, the
/// agent is frozen, the driver's inbox is full — is returned as
/// [`InferError::Send`] and never retried: a retry policy is a follow-on,
/// and a budget refusal retried is a loop with an empty purse (ADR-0006 §4).
#[derive(Debug, thiserror::Error)]
pub enum InferError {
    /// The request could not be encoded as bridge JSON.
    #[error("request could not be encoded: {0}")]
    Encode(#[source] serde_json::Error),
    /// The kernel refused the `send`. Nothing was delivered or billed.
    #[error("send refused: {0}")]
    Send(#[source] KernelError),
    /// The `recv` failed.
    #[error("recv failed: {0}")]
    Recv(#[source] KernelError),
    /// A `Notice` arrived while waiting for the reply: this agent has been
    /// cancelled and is in its grace period. `reason` is the canceller's
    /// payload, which the kernel does not read and this crate does not
    /// interpret.
    #[error("cancelled while waiting for the model")]
    Cancelled {
        /// The reason the canceller gave, as bytes.
        reason: Vec<u8>,
    },
    /// The reply's payload is not in the blob store.
    #[error("reply payload {} is not readable", .0)]
    MissingPayload(BlobRef),
    /// The reply bytes are not a bridge reply.
    #[error("reply could not be decoded: {0}")]
    Decode(#[source] serde_json::Error),
    /// The reply is stamped with a bridge version this crate does not speak.
    #[error("reply is bridge version {found}; this crate speaks {VERSION}")]
    Version {
        /// The version on the reply.
        found: u16,
    },
}

/// The bytes a `send` to a model capability carries: the request as bridge
/// JSON (ADR-0006 §2).
///
/// # Errors
///
/// [`InferError::Encode`] if serialization fails, which for a well-formed
/// request it does not.
pub fn encode_request(request: &ModelRequest) -> Result<Vec<u8>, InferError> {
    serde_json::to_vec(request).map_err(InferError::Encode)
}

/// The reply behind a model driver's bytes (ADR-0006 §3).
///
/// # Errors
///
/// [`InferError::Decode`] if the bytes are not a reply;
/// [`InferError::Version`] if they are a reply from another bridge version.
pub fn decode_reply(bytes: &[u8]) -> Result<ModelReply, InferError> {
    let reply: ModelReply = serde_json::from_slice(bytes).map_err(InferError::Decode)?;
    if reply.v != VERSION {
        return Err(InferError::Version { found: reply.v });
    }
    Ok(reply)
}

/// One model call over the seven syscalls.
///
/// Encodes `request`, sends it to `model`, waits for the reply on that
/// correlation, reads the reply's bytes, and decodes them. The wait also
/// watches for a `Notice`, so a cancel of this agent ends the call with
/// [`InferError::Cancelled`] instead of a hang until the hard abort.
///
/// Cancel-safe: the future holds the handle, the request, and plain memory,
/// and nothing across an await that a drop would have to undo (ADR-0003).
///
/// # Errors
///
/// See [`InferError`]. Every variant is terminal for this call.
pub async fn infer(
    handle: &Handle,
    model: Capability,
    request: &ModelRequest,
) -> Result<ModelReply, InferError> {
    let bytes = encode_request(request)?;
    let corr = handle.send(model, &bytes).map_err(InferError::Send)?;
    let msg = await_reply(handle, corr).await?;
    let payload = handle
        .read(msg.payload)
        .ok_or(InferError::MissingPayload(msg.payload))?;
    decode_reply(&payload)
}

/// The reply on `corr`, or the cancel notice that pre-empts it.
///
/// A `Partial` on the correlation is skipped: v1 of the bridge does not
/// stream, and a fragment is not the answer. Any `Notice` is taken to be a
/// cancellation, which in M1 is the only notice an agent can receive; when
/// hooks can `Emit` (M2) this is where their notices will be told apart.
pub(crate) async fn await_reply(handle: &Handle, corr: Corr) -> Result<Msg, InferError> {
    loop {
        let msg = handle
            .recv(Match::Or(vec![
                Match::Corr(corr),
                Match::Kind(MsgKind::Notice),
            ]))
            .await
            .map_err(InferError::Recv)?;
        match msg.kind {
            MsgKind::Reply => return Ok(msg),
            MsgKind::Notice => {
                let reason = handle.read(msg.payload).unwrap_or_default();
                return Err(InferError::Cancelled { reason });
            }
            // v1 does not stream; a fragment is not the answer. The enum is
            // non-exhaustive: a kind this build does not know is skipped too.
            _ => continue,
        }
    }
}
