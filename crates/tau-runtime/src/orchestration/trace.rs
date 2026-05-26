//! TraceStream: append-only event log with mpsc subscribers.
//!
//! Producers: agents (via virtual tools), the host (budget, lease).
//! Consumers: CLI printer, JSONL persister, watchdog. All consumers
//! subscribe via mpsc senders received when they register.
//!
//! Backpressure: bounded mpsc would block producers; unbounded is the
//! right choice for v1 since trace events are small and consumers
//! drain quickly. Reconsider if memory becomes a concern.

use tokio::sync::mpsc;

use tau_ports::TraceEvent;

/// One subscriber's sender side. The corresponding receiver is owned
/// by the subscriber (CLI printer, JSONL persister, etc.).
pub type TraceSubscriber = mpsc::UnboundedSender<TraceEvent>;

/// Multi-consumer fan-out. Each emit clones the event to every subscriber.
///
/// # Example
///
/// ```
/// use tau_runtime::orchestration::trace::TraceStream;
///
/// let mut stream = TraceStream::new();
/// assert_eq!(stream.subscriber_count(), 0);
/// let _rx = stream.subscribe();
/// assert_eq!(stream.subscriber_count(), 1);
/// ```
#[derive(Debug, Default)]
pub struct TraceStream {
    subscribers: Vec<TraceSubscriber>,
}

impl TraceStream {
    /// Empty stream with no subscribers.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime::orchestration::trace::TraceStream;
    ///
    /// let stream = TraceStream::new();
    /// assert_eq!(stream.subscriber_count(), 0);
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new subscriber. Returns the receiver side; caller is
    /// responsible for draining it.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime::orchestration::trace::TraceStream;
    ///
    /// let mut stream = TraceStream::new();
    /// let _rx1 = stream.subscribe();
    /// let _rx2 = stream.subscribe();
    /// assert_eq!(stream.subscriber_count(), 2);
    /// ```
    pub fn subscribe(&mut self) -> mpsc::UnboundedReceiver<TraceEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.subscribers.push(tx);
        rx
    }

    /// Fan out one event to every subscriber. Dropped subscribers (closed
    /// receivers) are silently removed from the next emit.
    ///
    /// # Example
    ///
    /// ```
    /// use chrono::Utc;
    /// use tau_runtime::orchestration::trace::TraceStream;
    /// use tau_ports::{TraceEvent, TraceEventKind};
    ///
    /// let mut stream = TraceStream::new();
    /// let mut rx = stream.subscribe();
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
    /// let received = rx.try_recv().expect("event delivered");
    /// assert_eq!(received.id, "evt-1");
    /// ```
    pub fn emit(&mut self, event: TraceEvent) {
        self.subscribers.retain(|tx| tx.send(event.clone()).is_ok());
    }

    /// Number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }
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
            run_id: "run_01".into(),
            agent_id: Some("agent_01".into()),
            kind: TraceEventKind::Turn {
                agent_id: "agent_01".into(),
                turn_index: 0,
                duration_ms: 100,
            },
        }
    }

    #[tokio::test]
    async fn emit_delivers_to_all_subscribers() {
        let mut stream = TraceStream::new();
        let mut a = stream.subscribe();
        let mut b = stream.subscribe();
        stream.emit(make_event("e1"));
        assert_eq!(a.recv().await.unwrap().id, "e1");
        assert_eq!(b.recv().await.unwrap().id, "e1");
    }

    #[tokio::test]
    async fn dropped_subscriber_does_not_block_emit() {
        let mut stream = TraceStream::new();
        let _a = stream.subscribe();
        {
            let _b = stream.subscribe();
        } // b's receiver dropped
        stream.emit(make_event("e1"));
        // After this emit, dropped subscriber is reaped.
        assert_eq!(stream.subscriber_count(), 1);
    }
}
