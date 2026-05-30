//! TraceSubscriber trait + TraceStream fan-out.
//!
//! The `TraceSubscriber` trait is executor-agnostic: `emit` is a
//! synchronous one-way fire-and-forget. Host shells plug in their own
//! implementation:
//!
//! - **tokio shell** (`tau-runtime`): `MpscTraceSubscriber` in
//!   `orchestration/trace_mpsc.rs` — wraps an `UnboundedSender<TraceEvent>`.
//!   The persistence writer drains the receiver side.
//! - **no-op / test**: `NoopTraceSubscriber` defined here.
//!
//! `TraceStream` holds a `Vec<Arc<dyn TraceSubscriber>>` and fans events
//! out to every registered subscriber.

use alloc::sync::Arc;
use alloc::vec::Vec;

use tau_ports::TraceEvent;

/// Sink for trace events emitted by the orchestration kernel.
///
/// Implementors: `NoopTraceSubscriber` (this module), `MpscTraceSubscriber`
/// (tau-runtime tokio shell). Embassy / wasm shells provide their own.
///
/// The method is `&self` (shared reference) so subscribers can be held in
/// `Arc<dyn TraceSubscriber>` — the subscriber is responsible for internal
/// synchronisation if needed (e.g. `Mutex<Sender>`).
pub trait TraceSubscriber: Send + Sync {
    /// Deliver `event` to this subscriber. Infallible: a subscriber that
    /// cannot accept an event (e.g. channel closed) should silently discard.
    fn emit(&self, event: TraceEvent);
}

/// No-op subscriber. Discards every event. Used in tests and single-agent
/// `Runtime::run` paths where no trace sink is configured.
///
/// # Example
///
/// ```
/// use std::sync::Arc;
/// use tau_runtime_core::orchestration::trace::{NoopTraceSubscriber, TraceSubscriber};
/// use tau_ports::{TraceEvent, TraceEventKind};
/// use chrono::Utc;
///
/// let noop = Arc::new(NoopTraceSubscriber);
/// noop.emit(TraceEvent {
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
/// // No panic — NoopTraceSubscriber silently discards.
/// ```
pub struct NoopTraceSubscriber;

impl TraceSubscriber for NoopTraceSubscriber {
    fn emit(&self, _event: TraceEvent) {}
}

/// Multi-subscriber fan-out. Each `emit` clones the event to every
/// registered subscriber.
///
/// # Example
///
/// ```
/// use std::sync::Arc;
/// use tau_runtime_core::orchestration::trace::{TraceStream, NoopTraceSubscriber};
///
/// let mut stream = TraceStream::new();
/// assert_eq!(stream.subscriber_count(), 0);
/// stream.add_subscriber(Arc::new(NoopTraceSubscriber));
/// assert_eq!(stream.subscriber_count(), 1);
/// ```
#[derive(Default)]
pub struct TraceStream {
    subscribers: Vec<Arc<dyn TraceSubscriber>>,
}

impl TraceStream {
    /// Empty stream with no subscribers.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::orchestration::trace::TraceStream;
    ///
    /// let stream = TraceStream::new();
    /// assert_eq!(stream.subscriber_count(), 0);
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a subscriber. The subscriber must implement [`TraceSubscriber`].
    ///
    /// # Example
    ///
    /// ```
    /// use std::sync::Arc;
    /// use tau_runtime_core::orchestration::trace::{TraceStream, NoopTraceSubscriber};
    ///
    /// let mut stream = TraceStream::new();
    /// stream.add_subscriber(Arc::new(NoopTraceSubscriber));
    /// stream.add_subscriber(Arc::new(NoopTraceSubscriber));
    /// assert_eq!(stream.subscriber_count(), 2);
    /// ```
    pub fn add_subscriber(&mut self, sub: Arc<dyn TraceSubscriber>) {
        self.subscribers.push(sub);
    }

    /// Fan out one event to every subscriber.
    ///
    /// # Example
    ///
    /// ```
    /// use std::sync::{Arc, Mutex};
    /// use tau_runtime_core::orchestration::trace::{TraceStream, TraceSubscriber};
    /// use tau_ports::{TraceEvent, TraceEventKind};
    /// use chrono::Utc;
    ///
    /// struct CollectSubscriber(Mutex<Vec<TraceEvent>>);
    /// impl TraceSubscriber for CollectSubscriber {
    ///     fn emit(&self, event: TraceEvent) {
    ///         self.0.lock().unwrap().push(event);
    ///     }
    /// }
    ///
    /// let collector = Arc::new(CollectSubscriber(Mutex::new(vec![])));
    /// let mut stream = TraceStream::new();
    /// stream.add_subscriber(Arc::clone(&collector) as Arc<dyn TraceSubscriber>);
    ///
    /// let event = TraceEvent {
    ///     id: "evt-1".into(),
    ///     ts: Utc::now(),
    ///     run_id: "run-1".into(),
    ///     agent_id: Some("agent-1".into()),
    ///     kind: TraceEventKind::Turn {
    ///         agent_id: "agent-1".into(),
    ///         turn_index: 0,
    ///         duration_ms: 50,
    ///     },
    /// };
    /// stream.emit(event);
    /// assert_eq!(collector.0.lock().unwrap().len(), 1);
    /// assert_eq!(collector.0.lock().unwrap()[0].id, "evt-1");
    /// ```
    pub fn emit(&self, event: TraceEvent) {
        if self.subscribers.is_empty() {
            return;
        }
        // Clone to all but last; last gets the original.
        let last = self.subscribers.len() - 1;
        for sub in &self.subscribers[..last] {
            sub.emit(event.clone());
        }
        self.subscribers[last].emit(event);
    }

    /// Number of registered subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use alloc::vec;
    use chrono::Utc;
    use std::sync::Mutex;
    use tau_ports::TraceEventKind;

    struct CollectSubscriber(Mutex<Vec<TraceEvent>>);
    impl TraceSubscriber for CollectSubscriber {
        fn emit(&self, event: TraceEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn make_event(id: &str) -> TraceEvent {
        TraceEvent {
            id: id.into(),
            ts: Utc::now(),
            run_id: "run_01".into(),
            agent_id: Some("agent_01".into()),
            kind: TraceEventKind::Turn {
                agent_id: "agent_01".into(),
                turn_index: 0,
                duration_ms: 100,
            },
        }
    }

    #[test]
    fn emit_delivers_to_all_subscribers() {
        let a = Arc::new(CollectSubscriber(Mutex::new(vec![])));
        let b = Arc::new(CollectSubscriber(Mutex::new(vec![])));
        let mut stream = TraceStream::new();
        stream.add_subscriber(Arc::clone(&a) as Arc<dyn TraceSubscriber>);
        stream.add_subscriber(Arc::clone(&b) as Arc<dyn TraceSubscriber>);
        stream.emit(make_event("e1"));
        assert_eq!(a.0.lock().unwrap().len(), 1);
        assert_eq!(b.0.lock().unwrap().len(), 1);
        assert_eq!(a.0.lock().unwrap()[0].id, "e1");
        assert_eq!(b.0.lock().unwrap()[0].id, "e1");
    }

    #[test]
    fn noop_subscriber_does_not_panic() {
        let mut stream = TraceStream::new();
        stream.add_subscriber(Arc::new(NoopTraceSubscriber));
        stream.emit(make_event("e1")); // must not panic
        assert_eq!(stream.subscriber_count(), 1);
    }

    #[test]
    fn emit_with_no_subscribers_is_noop() {
        let stream = TraceStream::new();
        stream.emit(make_event("e1")); // must not panic
    }
}
