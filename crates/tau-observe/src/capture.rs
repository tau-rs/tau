//! Test-only capture subscriber.
//!
//! Each [`CapturedEvent`] records the event's `target`, level, name (the
//! event's `message` field if present, otherwise its callsite name),
//! and structured fields as a `BTreeMap<String, String>` (Display of
//! each value).
//!
//! Usage:
//! ```ignore
//! use tau_observe::capture::Captor;
//! let captor = Captor::new();
//! tracing::subscriber::with_default(captor.subscriber(), || {
//!     tracing::info!(foo = 1, "my.event");
//! });
//! let events = captor.events();
//! assert_eq!(events[0].name, "my.event");
//! ```
//!
//! Gated behind the `test-fixtures` cargo feature (and `cfg(test)` for
//! in-crate tests). Not part of the production public API.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tracing::{field::Visit, Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
use tracing_subscriber::registry::Registry;

/// One event captured by [`Captor`].
#[derive(Debug, Clone)]
pub struct CapturedEvent {
    /// `tracing::Metadata::target()` — e.g. `"tau_runtime::stream"`.
    pub target: String,
    /// Event level as a lowercase string (`"info"`, `"debug"`, …).
    pub level: String,
    /// The event message (the literal in the macro) or the event's
    /// callsite name if no `message` field was set.
    pub name: String,
    /// Structured fields rendered through `Display`/`Debug`.
    pub fields: BTreeMap<String, String>,
}

/// Shared captor handle. Cheap to clone — wraps an `Arc<Mutex<…>>` of
/// the captured event vec.
#[derive(Clone, Default)]
pub struct Captor {
    inner: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl Captor {
    /// New, empty captor.
    pub fn new() -> Self {
        Self::default()
    }

    /// Wrap the captor as a `tracing` subscriber. Pass to
    /// `tracing::subscriber::with_default`.
    pub fn subscriber(&self) -> impl Subscriber + Send + Sync {
        let layer = CaptureLayer {
            sink: self.inner.clone(),
        };
        Registry::default().with(layer)
    }

    /// Snapshot of all captured events so far.
    pub fn events(&self) -> Vec<CapturedEvent> {
        self.inner.lock().unwrap().clone()
    }
}

struct CaptureLayer {
    sink: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S> Layer<S> for CaptureLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let meta = event.metadata();
        let name = visitor
            .fields
            .remove("message")
            .unwrap_or_else(|| meta.name().to_string());
        self.sink.lock().unwrap().push(CapturedEvent {
            target: meta.target().to_string(),
            level: meta.level().to_string().to_lowercase(),
            name,
            fields: visitor.fields,
        });
    }
}

#[derive(Default)]
struct FieldVisitor {
    fields: BTreeMap<String, String>,
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_message_and_fields() {
        let captor = Captor::new();
        tracing::subscriber::with_default(captor.subscriber(), || {
            tracing::info!(turn_index = 3, "runtime.turn_started");
        });
        let events = captor.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "runtime.turn_started");
        assert_eq!(events[0].level, "info");
        assert_eq!(
            events[0].fields.get("turn_index").map(|s| s.as_str()),
            Some("3")
        );
    }

    #[test]
    fn captures_multiple_events_in_order() {
        let captor = Captor::new();
        tracing::subscriber::with_default(captor.subscriber(), || {
            tracing::info!("first");
            tracing::debug!("second");
            tracing::warn!("third");
        });
        let events = captor.events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].name, "first");
        assert_eq!(events[1].name, "second");
        assert_eq!(events[2].name, "third");
        assert_eq!(events[0].level, "info");
        assert_eq!(events[1].level, "debug");
        assert_eq!(events[2].level, "warn");
    }

    #[test]
    fn captor_clone_shares_sink() {
        let captor_a = Captor::new();
        let captor_b = captor_a.clone();
        tracing::subscriber::with_default(captor_a.subscriber(), || {
            tracing::info!("shared");
        });
        // Both handles see the same event.
        assert_eq!(captor_a.events().len(), 1);
        assert_eq!(captor_b.events().len(), 1);
    }
}
