//! Smoke test for the feature-gated OpenTelemetry layer composition.
//!
//! This test does NOT hit a network OTLP collector. Instead it composes
//! a `tracing-opentelemetry` layer on top of the in-process
//! `opentelemetry-stdout` exporter and asserts that emitting a nested
//! span tree does not panic. Visual inspection via
//! `cargo test -- --nocapture` confirms the spans serialize correctly.
//!
//! Stronger assertions (intercepting writer + asserting JSON content)
//! are intentionally deferred per the logging-F plan.

#![cfg(feature = "otlp")]

#[test]
fn span_tree_emits_to_stdout_exporter() {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::Registry;

    let exporter = opentelemetry_stdout::SpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter)
        .build();
    let tracer = provider.tracer("test");
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let subscriber = Registry::default().with(otel_layer);

    tracing::subscriber::with_default(subscriber, || {
        let outer = tracing::info_span!("runtime.agent_run", agent_id = "test");
        let _e = outer.enter();
        let turn = tracing::info_span!("runtime.turn", turn_index = 1u64);
        let _e2 = turn.enter();
        let llm = tracing::info_span!("llm.complete");
        let _e3 = llm.enter();
        tracing::info!("llm.request_built");
    });
}
