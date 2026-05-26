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

/// File rotation policy when writing to a file sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation {
    /// No rotation — append forever to a single file.
    #[default]
    Never,
    /// Roll over each day at UTC midnight (filename gains a date suffix).
    Daily,
    /// Roll over each hour (filename gains a date+hour suffix).
    Hourly,
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
    /// When `true`, the fmt layer writes through a non-blocking MPSC
    /// channel. Requires feature `non_blocking` and `file_path` to be
    /// set; otherwise the flag is ignored (blocking install runs).
    pub non_blocking: bool,
    /// Optional file sink. When set together with `non_blocking`,
    /// overrides [`Writer`]. Requires feature `non_blocking`.
    pub file_path: Option<std::path::PathBuf>,
    /// File rotation policy. Ignored unless `file_path` is set.
    pub rotation: Rotation,
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
            non_blocking: false,
            file_path: None,
            rotation: Rotation::Never,
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
            non_blocking: false,
            file_path: None,
            rotation: Rotation::Never,
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
/// holds the non-blocking appender's `WorkerGuard` when the
/// `non_blocking` feature is enabled and the non-blocking install
/// path was taken).
#[derive(Debug)]
pub struct InstallGuard {
    #[cfg(feature = "non_blocking")]
    _appender_guard: Option<tracing_appender::non_blocking::WorkerGuard>,
    _private: (),
}

impl InstallGuard {
    fn empty() -> Self {
        Self {
            #[cfg(feature = "non_blocking")]
            _appender_guard: None,
            _private: (),
        }
    }
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
        return Ok(InstallGuard::empty());
    }

    #[cfg(feature = "non_blocking")]
    {
        if opts.non_blocking && opts.file_path.is_some() {
            let guard = install_non_blocking_inner(opts)?;
            *installed = true;
            return Ok(guard);
        }
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
            Ok(InstallGuard::empty())
        }
        // `try_init` returns Err when a subscriber is already installed.
        // We treat that as success because another part of the process
        // (e.g. a foreign test harness) has already initialized one.
        // The guard the caller receives is a no-op.
        Err(_) => {
            *installed = true;
            Ok(InstallGuard::empty())
        }
    }
}

/// Split a file path into the (directory, file-name) pair that
/// `tracing-appender` needs.
///
/// `Path::parent()` returns `Some("")` for a bare filename like
/// `PathBuf::from("app.log")` (it is `None` only for paths like `/`
/// and `""`). `tracing-appender` treats an empty directory as the
/// process CWD silently — we make that explicit by normalizing to
/// `"."` so logs always land somewhere predictable.
///
/// The `file_name()` `.expect()` stays: paths ending in `/` or `..`
/// have no filename, and the resulting panic surfaces a programmer
/// error instead of passing nonsense to `tracing-appender`.
#[cfg(feature = "non_blocking")]
fn resolve_appender_paths(path: &std::path::Path) -> (std::path::PathBuf, String) {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let prefix = path
        .file_name()
        .expect("file_path has no filename component")
        .to_string_lossy()
        .into_owned();
    (dir, prefix)
}

/// Build the non-blocking install path. Caller must have validated
/// that `opts.file_path.is_some()` and holds the install lock.
#[cfg(feature = "non_blocking")]
fn install_non_blocking_inner(opts: InstallOptions) -> Result<InstallGuard, InstallError> {
    use tracing_appender::rolling;
    use tracing_subscriber::fmt;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let path = opts
        .file_path
        .clone()
        .expect("install_non_blocking_inner called without file_path");
    let (dir, prefix) = resolve_appender_paths(&path);

    let file_appender = match opts.rotation {
        Rotation::Never => rolling::never(&dir, &prefix),
        Rotation::Daily => rolling::daily(&dir, &prefix),
        Rotation::Hourly => rolling::hourly(&dir, &prefix),
    };
    let (writer, worker_guard) = tracing_appender::non_blocking(file_appender);

    let registry = tracing_subscriber::registry().with(opts.filter);
    let result = match opts.format {
        Format::Human => registry.with(fmt::layer().with_writer(writer)).try_init(),
        Format::Json => registry
            .with(
                fmt::layer()
                    .json()
                    .with_writer(writer)
                    .with_current_span(true)
                    .with_span_list(false),
            )
            .try_init(),
    };
    // `try_init` returns Err when a subscriber is already installed —
    // treat that as success (parity with the blocking path).
    let _ = result;

    Ok(InstallGuard {
        _appender_guard: Some(worker_guard),
        _private: (),
    })
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
    fn cli_default_has_non_blocking_off_and_no_file() {
        let opts = InstallOptions::cli_default();
        assert!(!opts.non_blocking);
        assert!(opts.file_path.is_none());
        assert_eq!(opts.rotation, Rotation::Never);
    }

    #[test]
    fn install_is_idempotent() {
        // Two installs in the same test binary must both succeed.
        let _g1 = install(InstallOptions::cli_default()).unwrap();
        let _g2 = install(InstallOptions::cli_default()).unwrap();
    }

    #[cfg(feature = "non_blocking")]
    #[test]
    fn resolve_appender_paths_normalizes_bare_filename_to_cwd() {
        use std::path::{Path, PathBuf};

        // Bare filename: parent() yields Some(""), which we normalize
        // to "." so tracing-appender doesn't silently fall back to CWD.
        let (dir, prefix) = resolve_appender_paths(Path::new("app.log"));
        assert_eq!(dir, PathBuf::from("."));
        assert_eq!(prefix, "app.log");
    }

    #[cfg(feature = "non_blocking")]
    #[test]
    fn resolve_appender_paths_preserves_nested_directory() {
        use std::path::{Path, PathBuf};

        let (dir, prefix) = resolve_appender_paths(Path::new("logs/sub/app.log"));
        assert_eq!(dir, PathBuf::from("logs/sub"));
        assert_eq!(prefix, "app.log");
    }

    #[cfg(feature = "non_blocking")]
    #[test]
    fn resolve_appender_paths_preserves_absolute_directory() {
        use std::path::{Path, PathBuf};

        let (dir, prefix) = resolve_appender_paths(Path::new("/var/log/tau/app.log"));
        assert_eq!(dir, PathBuf::from("/var/log/tau"));
        assert_eq!(prefix, "app.log");
    }
}
