//! Canonical tracing-subscriber installer.
//!
//! Two install paths supported at v1: human-readable to stderr (CLI),
//! and JSON to stderr (plugin SDK). Both go through [`install`] so the
//! filter-resolution and idempotency behavior are identical.

use std::sync::{Mutex, OnceLock};
use thiserror::Error;
use tracing_subscriber::filter::EnvFilter;

/// Output format for the fmt layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Human-readable (timestamp + level + target + fields + message).
    Human,
    /// JSON Lines, one event per line.
    Json,
}

/// Where the subscriber writes serialized events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writer {
    /// Standard error.
    Stderr,
    /// Standard output.
    Stdout,
}

/// All knobs the canonical installer accepts.
#[derive(Debug)]
pub struct InstallOptions {
    /// Filter to apply. Build via `tau_observe::filter::env_or_directive`.
    pub filter: EnvFilter,
    /// Serialization format.
    pub format: Format,
    /// Sink.
    pub writer: Writer,
    /// When set, an OpenTelemetry layer exports spans over OTLP/gRPC.
    /// Requires feature `otlp`.
    #[cfg(feature = "otlp")]
    pub otlp: Option<crate::otlp::OtlpEndpoint>,
}

impl InstallOptions {
    /// Default options for the `tau` CLI: human format on stderr,
    /// `tau=info` fallback filter.
    pub fn cli_default() -> Self {
        Self {
            filter: crate::filter::env_or_directive("tau=info"),
            format: Format::Human,
            writer: Writer::Stderr,
            #[cfg(feature = "otlp")]
            otlp: None,
        }
    }

    /// Default options for plugins authored against `tau-plugin-sdk`:
    /// JSON to stderr (read by the host), `info` fallback filter.
    pub fn plugin_sdk() -> Self {
        Self {
            filter: crate::filter::env_or_directive("info"),
            format: Format::Json,
            writer: Writer::Stderr,
            #[cfg(feature = "otlp")]
            otlp: None,
        }
    }
}

/// Errors from [`install`].
#[derive(Debug, Error)]
pub enum InstallError {
    /// A subscriber is already installed in this process and the global
    /// init was attempted a second time. Calls that want idempotent
    /// install go through [`install`] (which short-circuits) — this
    /// error is reserved for explicit `install_unique`-style entry
    /// points that may be added later.
    #[error("a tracing subscriber is already installed for this process")]
    AlreadyInstalled,
}

/// Guard returned by [`install`]. Drop runs after-effects (currently
/// none; reserved for sub-project E's non-blocking writer flush).
#[derive(Debug)]
pub struct InstallGuard {
    _private: (),
}

static INSTALL_ONCE: OnceLock<Mutex<bool>> = OnceLock::new();

/// Build a `tracing-opentelemetry` layer from an [`OtlpEndpoint`].
///
/// Panics if the OTLP pipeline cannot be constructed (e.g. malformed
/// endpoint). v1 deliberately treats a misconfigured OTLP endpoint as a
/// fatal user error rather than degrading silently.
#[cfg(feature = "otlp")]
fn build_otel_layer<S>(
    otlp_ep: &crate::otlp::OtlpEndpoint,
) -> tracing_opentelemetry::OpenTelemetryLayer<S, opentelemetry_sdk::trace::Tracer>
where
    S: tracing::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    use opentelemetry_otlp::WithExportConfig as _;
    let mut exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint(&otlp_ep.endpoint);
    if !otlp_ep.headers.is_empty() {
        let mut metadata = tonic::metadata::MetadataMap::new();
        for (k, v) in &otlp_ep.headers {
            if let (Ok(name), Ok(val)) = (
                k.parse::<tonic::metadata::MetadataKey<tonic::metadata::Ascii>>(),
                v.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>(),
            ) {
                metadata.insert(name, val);
            }
        }
        exporter = exporter.with_metadata(metadata);
    }
    let tracer = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(exporter)
        .with_trace_config(opentelemetry_sdk::trace::config().with_resource(
            opentelemetry_sdk::Resource::default().merge(&opentelemetry_sdk::Resource::new([
                opentelemetry::KeyValue::new("service.name", "tau"),
                opentelemetry::KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
            ])),
        ))
        .install_batch(opentelemetry_sdk::runtime::Tokio)
        .expect("install OTLP pipeline");
    tracing_opentelemetry::layer().with_tracer(tracer)
}

/// Install the global tracing subscriber. Idempotent: subsequent calls
/// after a successful install are no-ops that return a fresh guard
/// without re-installing.
pub fn install(opts: InstallOptions) -> Result<InstallGuard, InstallError> {
    let cell = INSTALL_ONCE.get_or_init(|| Mutex::new(false));
    let mut installed = cell.lock().unwrap_or_else(|p| p.into_inner());
    if *installed {
        return Ok(InstallGuard { _private: () });
    }

    use tracing_subscriber::fmt;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let base = tracing_subscriber::registry().with(opts.filter);

    // Optional OpenTelemetry layer goes BELOW the fmt layer so that its
    // `Layer<S>` impl resolves against the stable `Layered<EnvFilter,
    // Registry>` subscriber type (independent of which fmt branch the
    // match selects below). `Option<Layer>` is itself a `Layer`, so
    // `None` is a zero-cost no-op.
    #[cfg(feature = "otlp")]
    let base = base.with(opts.otlp.as_ref().map(build_otel_layer));

    let result = match (opts.format, opts.writer) {
        (Format::Human, Writer::Stderr) => base
            .with(fmt::layer().with_writer(std::io::stderr))
            .try_init(),
        (Format::Human, Writer::Stdout) => base
            .with(fmt::layer().with_writer(std::io::stdout))
            .try_init(),
        (Format::Json, Writer::Stderr) => base
            .with(
                fmt::layer()
                    .json()
                    .with_writer(std::io::stderr)
                    .with_current_span(true)
                    .with_span_list(false),
            )
            .try_init(),
        (Format::Json, Writer::Stdout) => base
            .with(
                fmt::layer()
                    .json()
                    .with_writer(std::io::stdout)
                    .with_current_span(true)
                    .with_span_list(false),
            )
            .try_init(),
    };

    match result {
        Ok(()) => {
            *installed = true;
            Ok(InstallGuard { _private: () })
        }
        // `try_init` returns Err when a subscriber is already installed.
        // We treat that as success because another part of the process
        // (e.g. a foreign test harness) has already initialized one.
        // The guard the caller receives is a no-op.
        Err(_) => {
            *installed = true;
            Ok(InstallGuard { _private: () })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_default_uses_human_stderr() {
        let opts = InstallOptions::cli_default();
        assert_eq!(opts.format, Format::Human);
        assert_eq!(opts.writer, Writer::Stderr);
    }

    #[test]
    fn plugin_sdk_uses_json_stderr() {
        let opts = InstallOptions::plugin_sdk();
        assert_eq!(opts.format, Format::Json);
        assert_eq!(opts.writer, Writer::Stderr);
    }

    #[test]
    fn install_is_idempotent() {
        // Two installs in the same test binary must both succeed.
        let _g1 = install(InstallOptions::cli_default()).unwrap();
        let _g2 = install(InstallOptions::cli_default()).unwrap();
    }
}
