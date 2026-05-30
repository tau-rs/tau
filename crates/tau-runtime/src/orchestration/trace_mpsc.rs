//! Tokio-shell implementation of [`TraceSubscriber`].
//!
//! Wraps a `tokio::sync::mpsc::UnboundedSender<TraceEvent>` so that the
//! orchestration core's `TraceStream` can fan out to the JSONL persistence
//! writer without knowing about tokio.
//!
//! # Wire-up
//!
//! In `Runtime::spawn_root_agent`, after constructing `RunState`:
//!
//! ```ignore
//! use std::sync::Arc;
//! use crate::orchestration::trace_mpsc::MpscTraceSubscriber;
//! use crate::orchestration::persistence::{run_log_path, spawn_writer};
//!
//! let log_path = run_log_path(&scope_root, &run_id);
//! let (subscriber, rx) = MpscTraceSubscriber::channel();
//! let _writer_handle = spawn_writer(log_path, rx);
//! state.trace.add_subscriber(Arc::new(subscriber));
//! ```

use std::sync::Arc;

use tokio::sync::mpsc;

use tau_ports::TraceEvent;
use tau_runtime_core::orchestration::trace::TraceSubscriber;

/// A [`TraceSubscriber`] backed by a tokio unbounded mpsc channel sender.
///
/// Silently drops events when the channel is closed (receiver dropped).
///
/// # Example
///
/// ```
/// use std::sync::Arc;
/// use tau_runtime::orchestration::trace_mpsc::MpscTraceSubscriber;
/// use tau_runtime_core::orchestration::trace::TraceSubscriber;
/// use tau_ports::{TraceEvent, TraceEventKind};
/// use chrono::Utc;
///
/// let (subscriber, mut rx) = MpscTraceSubscriber::channel();
/// let sub: Arc<dyn TraceSubscriber> = Arc::new(subscriber);
///
/// sub.emit(TraceEvent {
///     id: "e1".into(),
///     ts: Utc::now(),
///     run_id: "r".into(),
///     agent_id: None,
///     kind: TraceEventKind::Turn {
///         agent_id: "a".into(),
///         turn_index: 0,
///         duration_ms: 1,
///     },
/// });
///
/// let received = rx.try_recv().expect("event in channel");
/// assert_eq!(received.id, "e1");
/// ```
pub struct MpscTraceSubscriber {
    tx: mpsc::UnboundedSender<TraceEvent>,
}

impl MpscTraceSubscriber {
    /// Create a subscriber + the corresponding receiver.
    /// Caller is responsible for draining the receiver (e.g. via
    /// [`super::persistence::spawn_writer`]).
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<TraceEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }
}

impl TraceSubscriber for MpscTraceSubscriber {
    fn emit(&self, event: TraceEvent) {
        // Silently drop if channel closed.
        let _ = self.tx.send(event);
    }
}

/// Factory helper: create an `MpscTraceSubscriber` + spawn the JSONL writer,
/// returning the subscriber ready to be added to a `TraceStream`.
///
/// This is a convenience wrapper around
/// `MpscTraceSubscriber::channel` + `persistence::spawn_writer`.
pub fn channel_with_writer(
    log_path: std::path::PathBuf,
) -> Arc<dyn TraceSubscriber> {
    let (subscriber, rx) = MpscTraceSubscriber::channel();
    super::persistence::spawn_writer(log_path, rx);
    Arc::new(subscriber)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tau_ports::TraceEventKind;

    fn make_event(id: &str) -> TraceEvent {
        TraceEvent {
            id: id.into(),
            ts: Utc::now(),
            run_id: "r".into(),
            agent_id: None,
            kind: TraceEventKind::Turn {
                agent_id: "a".into(),
                turn_index: 0,
                duration_ms: 1,
            },
        }
    }

    #[test]
    fn emit_sends_to_receiver() {
        let (subscriber, mut rx) = MpscTraceSubscriber::channel();
        subscriber.emit(make_event("e1"));
        let received = rx.try_recv().expect("event present");
        assert_eq!(received.id, "e1");
    }

    #[test]
    fn emit_silently_drops_when_receiver_closed() {
        let (subscriber, rx) = MpscTraceSubscriber::channel();
        drop(rx);
        // Must not panic.
        subscriber.emit(make_event("e1"));
    }
}
