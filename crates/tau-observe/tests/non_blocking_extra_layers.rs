//! tau-rs/tau#699 contract guard: the non-blocking install path composes
//! `extra_layers` and keeps them OUTSIDE the `EnvFilter`.
//!
//! `install` takes an early return into `install_non_blocking_inner`
//! whenever `--log-non-blocking --log-file <f>` is in play. That inner
//! path used to build `registry()` + the fmt layer and nothing else, so
//! every caller-supplied artifact sink was silently discarded: the
//! `WorkflowRunLogLayer` wrote no `workflow-runs/*.jsonl` (making
//! `tau workflow resume` replay nothing — the exact damage of #650, via
//! a second route), and `--record-protocol` produced no frames. The flag
//! was accepted, the process exited 0, and the artifact was absent.
//!
//! This is the non-blocking twin of `extra_layers_bypass_env_filter.rs`;
//! it asserts both halves at once — the layer is composed at all, and it
//! still sits outside the `EnvFilter` (#694) rather than under it.
//!
//! One `#[test]` per file on purpose: `install` is process-global and
//! idempotent, so a second install in the same binary is a no-op.
#![cfg(feature = "non_blocking")]

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
fn non_blocking_install_composes_extra_layers_outside_the_env_filter() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let log_path = tmp.path().join("tau.log");

    let recorder = Recorder::default();
    let seen = recorder.0.clone();

    let opts = InstallOptions {
        // Exactly what `tau --quiet` resolves to. Both events below are
        // INFO on `tau::*` targets, so a *global* `tau=warn` would drop
        // them before any layer was consulted.
        filter: EnvFilter::new("tau=warn"),
        format: Format::Human,
        writer: Writer::Stderr,
        // The two flags that route `install` into
        // `install_non_blocking_inner`: `--log-non-blocking --log-file`.
        non_blocking: true,
        file_path: Some(log_path),
        rotation: Rotation::Never,
        extra_layers: vec![Box::new(
            recorder.with_filter(only_target(ARTIFACT_TARGET, LevelFilter::INFO)),
        )],
        #[cfg(feature = "otlp")]
        otlp: None,
    };
    // The guard owns the appender worker; holding it keeps the
    // non-blocking writer alive for the duration of the test.
    let _guard = install(opts).expect("install returned err");

    tracing::event!(target: ARTIFACT_TARGET, tracing::Level::INFO, "artifact record");
    tracing::event!(target: OTHER_TARGET, tracing::Level::INFO, "ordinary console chatter");

    let seen = seen.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert!(
        seen.iter().any(|t| t == ARTIFACT_TARGET),
        "the non-blocking install path must compose `extra_layers`, and \
         they must see their own INFO events under a `tau=warn` filter \
         (tau-rs/tau#699, tau-rs/tau#694); saw {seen:?}"
    );
    assert!(
        !seen.iter().any(|t| t == OTHER_TARGET),
        "the per-layer filter must still narrow the firehose down to \
         {ARTIFACT_TARGET}; saw {seen:?}"
    );
}
