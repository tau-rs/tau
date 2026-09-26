//! One model call: `send` the request, `recv` the reply, `read` its bytes.
//! And, under a [`RetryPolicy`], the same call again when the driver could
//! not get an answer.

use std::future::{ready, Future};
use std::time::Duration;

use tau_kernel::abi::{BlobRef, Capability, Corr, Endpoint, HookId, Msg, MsgKind, Seq};
use tau_kernel::bridge::{ErrorKind, ModelError, ModelReply, ModelRequest, StopReason, VERSION};
use tau_kernel::kernel::KernelError;
use tau_kernel::syscall::{Handle, Match};

/// Why [`infer`] did not return a reply.
///
/// Every variant is terminal for the call, under any [`RetryPolicy`]. In
/// particular a `send` the kernel refuses — the ceiling cannot be reserved,
/// the capability is not held, the agent is frozen, the driver's inbox is
/// full — is returned as [`InferError::Send`] and never retried: a budget
/// refusal retried is a loop with an empty purse (ADR-0006 §4).
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
    /// The driver never answered (ADR-0014 §7): it crashed, or the request
    /// passed the bound the harness registered it with, or the harness
    /// retired it. The reply came from the kernel, empty, and the request
    /// was billed its ceiling if the driver had taken it. Not a
    /// [`ModelReply`] and not [`MissingPayload`](Self::MissingPayload): an
    /// unanswered request and a shredded reply are different facts.
    #[error("the driver did not answer request {corr}")]
    Unanswered {
        /// The request that closed without an answer.
        corr: Corr,
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

/// A note a hook left for this agent (ADR-0008 §3): the `Notice` its
/// `Emit` verdict produced, as this crate saw it while waiting for a reply.
///
/// A call waits with `recv` on the reply's correlation *or* any `Notice`,
/// so a cancel can pre-empt it. A hook's note satisfies that filter too,
/// and a `recv` that matched it has resolved it out of the mailbox: nothing
/// in the seven syscalls puts it back. So instead of being dropped (#83) it
/// is pushed onto the `notes` the caller passed in, and the wait goes on.
/// The program reads it after the call; the bytes are behind `payload`,
/// via `read`, exactly as for any message.
///
/// Only notes a call *resolved* land here. One that arrives after the last
/// `recv` of a call is still in the mailbox, where a `recv` on
/// `Match::Sender(Endpoint::Hook { id })` finds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    /// The hook whose `Emit` produced it.
    pub hook: HookId,
    /// The note's position in the log: the `Emitted` entry.
    pub seq: Seq,
    /// The note's bytes, by reference.
    pub payload: BlobRef,
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

/// How many times, and after what wait, a model call is made again when
/// the driver's reply says it could not get an answer.
///
/// # Design note (#34)
///
/// A retry is a second `send`: a second reservation of the driver's ceiling
/// plus one `calls` (ADR-0002, M1b), a second `send`/reply pair in the log,
/// and a second delivery the driver sees. Nothing is hidden: a driver never
/// retries on its own, and this policy is the only place a call is repeated.
/// The caller's budget is therefore the natural cap on `retries`: an attempt
/// whose reservation is refused ends the retries the way it ends everything
/// else, with [`InferError::Send`], never with another attempt.
///
/// What is retried is decided per reply by [`should_retry`]:
///
/// | reply | retried | why |
/// |---|---|---|
/// | `error.transport` | yes | no answer arrived; nothing the caller did |
/// | a reply from the kernel — [`InferError::Unanswered`] | yes, as `transport` | the driver crashed, hung past its bound, or was retired (ADR-0014 §7); if it was retired the retry's `send` is `Unroutable` and ends the call like any refused `send` |
/// | `error.provider`, message `HTTP 429` or `HTTP 5xx` (529 included) | yes | the provider said "not now" |
/// | `error.provider`, any other status, or no status to read | no | the request, or the driver's mapping, is wrong |
/// | `error.over_ceiling`, `error.unsupported` | no | the caller's mistake; nothing was billed |
/// | `refusal` and every other stop reason | no | a stop reason, not an error (ADR-0006 §3) |
/// | a refused `send`, a cancel, an unreadable reply | no | terminal, see [`InferError`] |
///
/// The status is read from the message's `HTTP <status>` prefix, which is
/// how a provider driver words it (ADR-0006 §3, "`message` carries the
/// provider's text"). A structured status on the reply is deferred: it is an
/// ABI change.
///
/// The wait before retry `n` (counting from zero) is `backoff · 2ⁿ`, capped
/// at `max_backoff`. No jitter: this crate draws no randomness, and a caller
/// that wants it can fold it into the sleep it supplies. Who sleeps is the
/// caller's business too: [`infer_with`] takes the sleep as a function, so
/// this crate never reads a clock and never depends on an executor.
///
/// The default is [`RetryPolicy::NONE`]: one `send`, ever.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RetryPolicy {
    /// Attempts after the first. Zero means one `send`, ever.
    pub retries: u32,
    /// The wait before the first retry. Doubles with each retry.
    pub backoff: Duration,
    /// The most any wait may be.
    pub max_backoff: Duration,
}

impl RetryPolicy {
    /// No retries. The default, and what [`infer`] and
    /// [`tool_loop`](fn@crate::tool_loop) use.
    pub const NONE: Self = Self {
        retries: 0,
        backoff: Duration::ZERO,
        max_backoff: Duration::ZERO,
    };

    /// `retries` attempts after the first, waiting 500 ms, then 1 s, 2 s,
    /// 4 s, and 8 s at most.
    #[must_use]
    pub const fn transient(retries: u32) -> Self {
        Self {
            retries,
            backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(8),
        }
    }

    /// The wait before retry number `retry`, counting from zero:
    /// `backoff · 2^retry`, saturating, and never above `max_backoff`.
    #[must_use]
    pub fn wait(&self, retry: u32) -> Duration {
        if self.backoff.is_zero() {
            return Duration::ZERO;
        }
        let mut wait = self.backoff;
        // Bounded: `wait` doubles until it reaches the cap or saturates,
        // which a non-zero duration does within a hundred doublings.
        for _ in 0..retry {
            if wait >= self.max_backoff {
                break;
            }
            wait = wait.saturating_mul(2);
        }
        wait.min(self.max_backoff)
    }
}

/// Whether a reply that stopped for `stop` is worth a second `send`. The
/// decision table is on [`RetryPolicy`].
#[must_use]
pub fn should_retry(stop: &StopReason) -> bool {
    match stop {
        StopReason::Error(ModelError {
            kind: ErrorKind::Transport,
            ..
        }) => true,
        StopReason::Error(ModelError {
            kind: ErrorKind::Provider,
            message,
        }) => matches!(http_status(message), Some(429 | 500..=599)),
        _ => false,
    }
}

/// The status in a provider driver's `HTTP <status> ...` message, if the
/// message is worded that way.
fn http_status(message: &str) -> Option<u16> {
    let rest = message.strip_prefix("HTTP ")?;
    let digits = rest.split(|c: char| !c.is_ascii_digit()).next()?;
    digits.parse().ok()
}

/// One model call over the seven syscalls.
///
/// Encodes `request`, sends it to `model`, waits for the reply on that
/// correlation, reads the reply's bytes, and decodes them. The wait also
/// watches for a `Notice`, so a cancel of this agent ends the call with
/// [`InferError::Cancelled`] instead of a hang until the hard abort. A
/// `Notice` from a hook is not a cancel: it is pushed onto `notes` and the
/// wait goes on ([`Note`]). Notes pushed before an error stay pushed.
///
/// No retries: an error reply is returned as it came. [`infer_with`] is the
/// same call under a [`RetryPolicy`].
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
    notes: &mut Vec<Note>,
) -> Result<ModelReply, InferError> {
    infer_with(handle, model, request, notes, &RetryPolicy::NONE, |_| {
        ready(())
    })
    .await
}

/// [`infer`] under a [`RetryPolicy`], with the wait done by `sleep`.
///
/// Each attempt is a full `send`/`recv`/`read` and its own pair of log
/// entries. After an attempt whose reply [`should_retry`], and while
/// `policy.retries` are left, `sleep` is awaited for
/// [`RetryPolicy::wait`] and the request is sent again, byte for byte. When
/// the retries run out, the last reply is returned as it came: an error
/// reply is `Ok`, with the error in its `stop`, exactly as from [`infer`].
///
/// `sleep` is the caller's: a harness on tokio passes `tokio::time::sleep`;
/// a test passes something that returns at once. This crate reads no clock.
/// A cancel `Notice` that lands during a sleep is not raced against it: the
/// next attempt's `send` is refused because the agent is frozen, and that
/// refusal is returned as [`InferError::Send`].
///
/// Cancel-safe as [`infer`] is, provided `sleep`'s future is: between
/// attempts the loop holds only plain memory.
///
/// # Errors
///
/// See [`InferError`]. Every variant is terminal: a refused `send` — a
/// budget that cannot cover one more attempt above all — ends the retries.
pub async fn infer_with<S, F>(
    handle: &Handle,
    model: Capability,
    request: &ModelRequest,
    notes: &mut Vec<Note>,
    policy: &RetryPolicy,
    mut sleep: S,
) -> Result<ModelReply, InferError>
where
    S: FnMut(Duration) -> F,
    F: Future<Output = ()>,
{
    let bytes = encode_request(request)?;
    let mut retry = 0;
    loop {
        // A reply from the kernel is retried like a transport error: no
        // answer arrived and nothing the caller did caused it (ADR-0014
        // §7). Every other error is terminal.
        let again = match call_once(handle, model, &bytes, notes).await {
            Ok(reply) => {
                if retry >= policy.retries || !should_retry(&reply.stop) {
                    return Ok(reply);
                }
                true
            }
            Err(InferError::Unanswered { .. }) if retry < policy.retries => true,
            Err(err) => return Err(err),
        };
        debug_assert!(again);
        sleep(policy.wait(retry)).await;
        retry += 1;
    }
}

/// One attempt: `send`, `recv`, `read`, decode.
async fn call_once(
    handle: &Handle,
    model: Capability,
    bytes: &[u8],
    notes: &mut Vec<Note>,
) -> Result<ModelReply, InferError> {
    let corr = handle.send(model, bytes).map_err(InferError::Send)?;
    let msg = await_reply(handle, corr, notes).await?;
    if msg.from == Endpoint::Kernel {
        return Err(InferError::Unanswered { corr });
    }
    let payload = handle
        .read(msg.payload)
        .ok_or(InferError::MissingPayload(msg.payload))?;
    decode_reply(&payload)
}

/// The reply on `corr`, or the cancel notice that pre-empts it.
///
/// A `Partial` on the correlation is skipped: v1 of the bridge does not
/// stream, and a fragment is not the answer. A `Notice` from an agent or
/// the harness is a cancellation. A `Notice` from a hook — an `Emit`
/// verdict, ADR-0008 §3 — is not: the `recv` has resolved it, so it is
/// kept as a [`Note`] on `notes` for the program, and the wait goes on.
pub(crate) async fn await_reply(
    handle: &Handle,
    corr: Corr,
    notes: &mut Vec<Note>,
) -> Result<Msg, InferError> {
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
                if let Endpoint::Hook { id } = msg.from {
                    notes.push(Note {
                        hook: id,
                        seq: msg.seq,
                        payload: msg.payload,
                    });
                    continue;
                }
                let reason = handle.read(msg.payload).unwrap_or_default();
                return Err(InferError::Cancelled { reason });
            }
            // v1 does not stream; a fragment is not the answer. The enum is
            // non-exhaustive: a kind this build does not know is skipped too.
            _ => continue,
        }
    }
}
