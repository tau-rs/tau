//! Custom `tracing_subscriber::Layer` impls that materialize internal
//! events into on-disk JSONL artifacts. See sub-project D in the
//! 2026-05-17 logging upgrades design.

pub mod plugin_recording;
pub mod workflow_run_log;

use tracing::Metadata;
use tracing_subscriber::filter::{filter_fn, FilterFn, LevelFilter};

/// Build the per-layer filter that a layer passed to
/// [`InstallOptions::extra_layers`](crate::install::InstallOptions::extra_layers)
/// is expected to carry: "only events on `target`, never anything more
/// verbose than `max_level`".
///
/// [`crate::install::install`] attaches the caller's `EnvFilter` as a
/// *per-layer* filter on the console sinks rather than as a global
/// filter (tau-rs/tau#694), so extra layers sit outside it and see every
/// event the process emits. That is what makes on-disk artifacts survive
/// `--quiet` / `RUST_LOG=error` — and it means each extra layer must
/// narrow the firehose itself.
///
/// `max_level` is not cosmetic. `Filter::max_level_hint` feeds
/// `tracing`'s process-wide max-level cache: with a hint of `INFO`,
/// every DEBUG/TRACE callsite in the process is disabled outright
/// instead of being evaluated and rejected once per event. Pass the
/// level the target's producers actually emit at — a hint that is too
/// coarse costs performance, one that is too fine silently drops
/// records.
///
/// ```
/// use tau_observe::layers::{only_target, workflow_run_log};
/// use tracing_subscriber::filter::LevelFilter;
///
/// // Wire a layer so it only ever sees its own target's events.
/// use tracing_subscriber::Layer as _;
/// let layer = workflow_run_log::WorkflowRunLogLayer::new("/tmp/run.jsonl".into())
///     .with_filter(only_target(workflow_run_log::TARGET, LevelFilter::INFO));
/// let _boxed: Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync> =
///     Box::new(layer);
/// ```
pub fn only_target(
    target: &'static str,
    max_level: LevelFilter,
) -> FilterFn<impl Fn(&Metadata<'_>) -> bool + Clone> {
    filter_fn(move |meta: &Metadata<'_>| meta.target() == target).with_max_level_hint(max_level)
}
