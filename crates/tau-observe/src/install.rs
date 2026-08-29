//! Canonical tracing-subscriber installer.
//!
//! Two install paths supported at v1: human-readable to stderr (CLI),
//! and JSON to stderr (plugin SDK). Both go through [`install`] so the
//! filter-resolution and idempotency behavior are identical.

use std::sync::{Mutex, OnceLock};
use thiserror::Error;
use tracing_subscriber::filter::EnvFilter;

/// Output format for the fmt layer.
///
/// ```
/// use tau_observe::install::Format;
///
/// assert_ne!(Format::Human, Format::Json);
/// // Format is Copy, so it can be stored and passed cheaply.
/// let fmt: Format = Format::Json;
/// let fmt2 = fmt; // copy
/// assert_eq!(fmt, fmt2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Human-readable (timestamp + level + target + fields + message).
    Human,
    /// JSON Lines, one event per line.
    Json,
}

/// Where the subscriber writes serialized events.
///
/// ```
/// use tau_observe::install::Writer;
///
/// assert_ne!(Writer::Stderr, Writer::Stdout);
/// // Writer is Copy, so it can be stored and passed cheaply.
/// let w: Writer = Writer::Stderr;
/// let w2 = w; // copy
/// assert_eq!(w, w2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writer {
    /// Standard error.
    Stderr,
    /// Standard output.
    Stdout,
}

/// File rotation policy when writing to a file sink.
///
/// ```
/// use tau_observe::install::Rotation;
///
/// // `Never` is the default.
/// assert_eq!(Rotation::default(), Rotation::Never);
/// // All three variants are distinct.
/// assert_ne!(Rotation::Never, Rotation::Daily);
/// assert_ne!(Rotation::Daily, Rotation::Hourly);
/// ```
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
///
/// Construct via [`InstallOptions::cli_default`] or
/// [`InstallOptions::plugin_sdk`], then override individual fields as
/// needed.
///
/// ```
/// use tau_observe::install::{InstallOptions, Format, Writer, Rotation};
///
/// let opts = InstallOptions::cli_default();
/// assert_eq!(opts.format, Format::Human);
/// assert_eq!(opts.writer, Writer::Stderr);
/// assert!(!opts.non_blocking);
/// assert!(opts.file_path.is_none());
/// assert_eq!(opts.rotation, Rotation::Never);
/// assert!(opts.extra_layers.is_empty());
/// ```
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
    /// Extra layers to compose into the registry alongside the fmt layer.
    /// Used by callers that need to wire custom on-disk JSONL sinks (e.g.
    /// `WorkflowRunLogLayer`, `PluginRecordingLayer`).
    ///
    /// Each layer must implement `Layer<Registry>` so it composes against
    /// the raw registry. [`Self::filter`] is attached as a *per-layer*
    /// filter on the console sinks (fmt + OTLP), NOT as a global filter,
    /// so these extras genuinely sit outside it: they are called for
    /// every event the process emits, whatever `-v` / `--quiet` /
    /// `RUST_LOG` say.
    ///
    /// That is the point. These layers write files the user asked for
    /// explicitly (`--record-protocol <path>`, the `tau workflow resume`
    /// run log); console verbosity must not decide whether those files
    /// get written. tau-rs/tau#694 is what happens otherwise — the
    /// `EnvFilter` used to be layered on top of the registry as a global
    /// filter, `--quiet` (`tau=warn`) short-circuited the INFO-level
    /// artifact events before these layers ever saw them, and `tau
    /// workflow run --quiet` exited 0 having written nothing.
    ///
    /// The flip side is that each extra layer owns its own filtering.
    /// Attach a per-layer filter with `Layer::with_filter` — see
    /// [`crate::layers::only_target`], which also pins the
    /// `max_level_hint` so the process-wide max level stays bounded.
    /// A layer without one is called for every event in every crate.
    pub extra_layers: Vec<
        Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync + 'static>,
    >,
    /// When set, an OpenTelemetry layer exports spans over OTLP/gRPC.
    /// Requires feature `otlp`.
    #[cfg(feature = "otlp")]
    pub otlp: Option<crate::otlp::OtlpEndpoint>,
}

impl std::fmt::Debug for InstallOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut dbg = f.debug_struct("InstallOptions");
        dbg.field("filter", &self.filter)
            .field("format", &self.format)
            .field("writer", &self.writer)
            .field("non_blocking", &self.non_blocking)
            .field("file_path", &self.file_path)
            .field("rotation", &self.rotation)
            .field(
                "extra_layers",
                &format_args!("[{} layers]", self.extra_layers.len()),
            );
        #[cfg(feature = "otlp")]
        dbg.field("otlp", &self.otlp);
        dbg.finish()
    }
}

impl InstallOptions {
    /// Default options for the `tau` CLI: human format on stderr,
    /// `tau=info` fallback filter.
    ///
    /// ```
    /// use tau_observe::install::{InstallOptions, Format, Writer};
    ///
    /// let opts = InstallOptions::cli_default();
    /// assert_eq!(opts.format, Format::Human);
    /// assert_eq!(opts.writer, Writer::Stderr);
    /// ```
    pub fn cli_default() -> Self {
        Self {
            filter: crate::filter::env_or_directive("tau=info"),
            format: Format::Human,
            writer: Writer::Stderr,
            non_blocking: false,
            file_path: None,
            rotation: Rotation::Never,
            extra_layers: Vec::new(),
            #[cfg(feature = "otlp")]
            otlp: None,
        }
    }

    /// Default options for plugins authored against `tau-plugin-sdk`:
    /// JSON to stderr (read by the host), `info` fallback filter.
    ///
    /// ```
    /// use tau_observe::install::{InstallOptions, Format, Writer};
    ///
    /// let opts = InstallOptions::plugin_sdk();
    /// assert_eq!(opts.format, Format::Json);
    /// assert_eq!(opts.writer, Writer::Stderr);
    /// ```
    pub fn plugin_sdk() -> Self {
        Self {
            filter: crate::filter::env_or_directive("info"),
            format: Format::Json,
            writer: Writer::Stderr,
            non_blocking: false,
            file_path: None,
            rotation: Rotation::Never,
            extra_layers: Vec::new(),
            #[cfg(feature = "otlp")]
            otlp: None,
        }
    }
}

/// Errors from [`install`].
///
/// ```
/// use tau_observe::install::InstallError;
///
/// // The Display message matches the `thiserror` annotation.
/// let err = InstallError::AlreadyInstalled;
/// assert!(err.to_string().contains("already installed"));
/// ```
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
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::{WithExportConfig as _, WithTonicConfig as _};

    let mut exporter_builder = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
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
        exporter_builder = exporter_builder.with_metadata(metadata);
    }
    let exporter = exporter_builder.build().expect("build OTLP span exporter");

    let resource = opentelemetry_sdk::Resource::builder()
        .with_attributes([
            opentelemetry::KeyValue::new("service.name", "tau"),
            opentelemetry::KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
        ])
        .build();

    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    let tracer = provider.tracer("tau");
    tracing_opentelemetry::layer().with_tracer(tracer)
}

/// Install the global tracing subscriber. Idempotent: subsequent calls
/// after a successful install are no-ops that return a fresh guard
/// without re-installing.
///
/// `no_run`: installs a process-global tracing subscriber. Running this
/// in a parallel doctest context would race with other tests that also
/// install a subscriber; the call shape is demonstrated without execution.
/// ```no_run
/// use tau_observe::install::{install, InstallOptions};
///
/// // Install once at process start; the guard must stay alive.
/// let _guard = install(InstallOptions::cli_default())
///     .expect("tracing subscriber install failed");
/// ```
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
    use tracing_subscriber::layer::Layer as _;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    // Compose caller-provided extra layers onto the raw registry first.
    // `Vec<Box<dyn Layer<Registry> + Send + Sync>>` implements `Layer<S>`
    // via tracing-subscriber's blanket impl for `Vec<L>`. We wrap the
    // vec in `Option` so it disappears entirely when empty — the Vec
    // impl's `enabled` is `iter().any(…)`, which is `false` for an empty
    // vec and would suppress every event process-wide.
    let extras = if opts.extra_layers.is_empty() {
        None
    } else {
        Some(opts.extra_layers)
    };

    // The `EnvFilter` is a PER-LAYER filter on the console sinks, not a
    // global one (tau-rs/tau#694). Layering it onto the registry made it
    // a global filter, and a global filter gates every layer in the
    // stack — including layers carrying their own per-layer filter. A
    // per-layer filter can only narrow what its layer sees; it can never
    // widen past a global filter. That made `--quiet` silently drop the
    // workflow run log and the `--record-protocol` recording, both of
    // which are on-disk artifacts the user asked for by name.
    //
    // The optional OpenTelemetry layer is an operator-facing sink too,
    // so it rides under the same filter. `Layer::and_then` folds it and
    // the fmt layer into a single `Layer<Registry>`, which lets one
    // `EnvFilter` cover both — `EnvFilter` is not `Clone`, so filtering
    // them separately would mean re-parsing the directives. `None` is a
    // zero-cost no-op because `Option<Layer>` is itself a `Layer`.
    #[cfg(feature = "otlp")]
    let console_extra = opts.otlp.as_ref().map(build_otel_layer);
    #[cfg(not(feature = "otlp"))]
    let console_extra: Option<tracing_subscriber::layer::Identity> = None;

    let filter = opts.filter;
    let base = tracing_subscriber::registry().with(extras);

    let result = match (opts.format, opts.writer) {
        (Format::Human, Writer::Stderr) => base
            .with(
                fmt::layer()
                    .with_writer(std::io::stderr)
                    .and_then(console_extra)
                    .with_filter(filter),
            )
            .try_init(),
        (Format::Human, Writer::Stdout) => base
            .with(
                fmt::layer()
                    .with_writer(std::io::stdout)
                    .and_then(console_extra)
                    .with_filter(filter),
            )
            .try_init(),
        (Format::Json, Writer::Stderr) => base
            .with(
                fmt::layer()
                    .json()
                    .with_writer(std::io::stderr)
                    .with_current_span(true)
                    .with_span_list(false)
                    .and_then(console_extra)
                    .with_filter(filter),
            )
            .try_init(),
        (Format::Json, Writer::Stdout) => base
            .with(
                fmt::layer()
                    .json()
                    .with_writer(std::io::stdout)
                    .with_current_span(true)
                    .with_span_list(false)
                    .and_then(console_extra)
                    .with_filter(filter),
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
    use tracing_subscriber::layer::Layer as _;
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

    // Per-layer filter on the fmt layer, matching the blocking path —
    // see the `install` body for why the `EnvFilter` must not be a
    // global filter (tau-rs/tau#694). This path does not compose
    // `extra_layers` or the OTLP layer at all yet; that gap is #699.
    let registry = tracing_subscriber::registry();
    let result = match opts.format {
        Format::Human => registry
            .with(fmt::layer().with_writer(writer).with_filter(opts.filter))
            .try_init(),
        Format::Json => registry
            .with(
                fmt::layer()
                    .json()
                    .with_writer(writer)
                    .with_current_span(true)
                    .with_span_list(false)
                    .with_filter(opts.filter),
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
