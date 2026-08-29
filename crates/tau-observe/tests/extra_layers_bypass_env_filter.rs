//! tau-rs/tau#694 contract guard: layers passed to
//! [`InstallOptions::extra_layers`] sit OUTSIDE the `EnvFilter`.
//!
//! `install` used to layer the `EnvFilter` onto the registry, where it
//! acts as a *global* filter — and a global filter gates every layer in
//! the stack, including layers that carry their own per-layer filter (a
//! per-layer filter only narrows what its layer sees; it can never widen
//! past a global one). The consequence was that `--quiet` (`tau=warn`)
//! silently emptied the `tau workflow resume` run log and dropped every
//! `--record-protocol` frame: both are INFO-level events on `tau::*`
//! targets, so the global filter killed them before the sink layers ran.
//!
//! `tau-cli`'s `cmd_workflow` / `cmd_plugin_run_protocol` suites assert
//! the user-visible artifacts end to end. This asserts the mechanism, in
//! one process, without a plugin build.
//!
//! One `#[test]` per file on purpose: `install` is process-global and
//! idempotent, so a second install in the same binary is a no-op.

use std::sync::{Arc, Mutex};

use tau_observe::install::{install, Format, InstallOptions, Rotation, Writer};
use tau_observe::layers::only_target;
use tracing::{Event, Subscriber};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::EnvFilter;

/// Stands in for `WorkflowRunLogLayer` / `PluginRecordingLayer`: a sink
/// whose output the user asked for by name, on a `tau::*` target that a
/// restrictive `EnvFilter` would otherwise swallow.
const ARTIFACT_TARGET: &str = "tau::test::artifact";

/// A `tau::*` target the artifact layer must NOT pick up — proves the
/// per-layer filter still narrows now that it is the only thing narrowing.
const OTHER_TARGET: &str = "tau::test::console";

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);

impl<S: Subscriber> Layer<S> for Recorder {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(event.metadata().target().to_string());
    }
}

#[test]
fn extra_layers_receive_events_the_env_filter_rejects() {
    let recorder = Recorder::default();
    let seen = recorder.0.clone();

    let opts = InstallOptions {
        // Exactly what `tau --quiet` resolves to. Both events below are
        // INFO on `tau::*` targets, so a *global* `tau=warn` would drop
        // them before any layer was consulted.
        filter: EnvFilter::new("tau=warn"),
        format: Format::Human,
        writer: Writer::Stderr,
        non_blocking: false,
        file_path: None,
        rotation: Rotation::Never,
        extra_layers: vec![Box::new(
            recorder.with_filter(only_target(ARTIFACT_TARGET, LevelFilter::INFO)),
        )],
        #[cfg(feature = "otlp")]
        otlp: None,
    };
    let _guard = install(opts).expect("install returned err");

    tracing::event!(target: ARTIFACT_TARGET, tracing::Level::INFO, "artifact record");
    tracing::event!(target: OTHER_TARGET, tracing::Level::INFO, "ordinary console chatter");

    let seen = seen.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert!(
        seen.iter().any(|t| t == ARTIFACT_TARGET),
        "the extra layer must see its own INFO events under a `tau=warn` \
         filter (tau-rs/tau#694); saw {seen:?}"
    );
    assert!(
        !seen.iter().any(|t| t == OTHER_TARGET),
        "the per-layer filter must still narrow the firehose down to \
         {ARTIFACT_TARGET}; saw {seen:?}"
    );
}
