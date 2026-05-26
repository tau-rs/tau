//! Tracing-subscriber configuration for tau-cli.
//!
//! The CLI's job is to map clap flags onto a final filter directive.
//! Subscriber install itself lives in [`tau_observe::install`] so the
//! CLI and plugin SDK share one code path (sub-project A consolidation).
//!
//! Per spec §3.7: stderr-targeted subscriber, default level INFO scoped
//! to `tau=*`, verbosity flags promote (-v: DEBUG, -vv: TRACE), --quiet
//! demotes to WARN, --debug behaves as -v plus expanded error chain at
//! print time, RUST_LOG overrides everything.

use tau_observe::filter::env_or_directive;
use tau_observe::install::{install as observe_install, Format, InstallOptions, Writer};
use tracing_subscriber::filter::EnvFilter;

use crate::cli::Cli;

/// Compute the `EnvFilter` from CLI flags + `RUST_LOG` env.
///
/// Resolution order:
/// 1. `RUST_LOG` (if set) — used verbatim, overrides flags.
/// 2. `--verbose` count >= 2 → `"tau=trace"`.
/// 3. `--debug` OR `--verbose` count >= 1 → `"tau=debug"`.
/// 4. `--quiet` → `"tau=warn"`.
/// 5. Default → `"tau=info"`.
pub fn build_filter(cli: &Cli) -> EnvFilter {
    let directive = if cli.verbose >= 2 {
        "tau=trace"
    } else if cli.debug || cli.verbose >= 1 {
        "tau=debug"
    } else if cli.quiet {
        "tau=warn"
    } else {
        "tau=info"
    };
    env_or_directive(directive)
}

/// Install the global tracing subscriber for the `tau` CLI.
///
/// Delegates to [`tau_observe::install::install`] with the CLI's
/// human-format, stderr-writer configuration. Idempotent — the
/// underlying installer short-circuits second calls.
pub fn install(cli: &Cli) {
    // --log-non-blocking without --log-file is a configuration error:
    // the non-blocking install path requires a file sink (it has no
    // meaning over stderr). Exit early so the misconfiguration is
    // surfaced loudly instead of being silently downgraded.
    if cli.log_non_blocking && cli.log_file.is_none() {
        eprintln!("error: --log-non-blocking requires --log-file");
        std::process::exit(2);
    }

    let opts = InstallOptions {
        filter: build_filter(cli),
        format: Format::Human,
        writer: Writer::Stderr,
        non_blocking: cli.log_non_blocking,
        file_path: cli.log_file.clone(),
        rotation: cli.log_rotation.into(),
        // Resolution order for OTLP endpoint:
        //  1. `--otlp-endpoint` flag (clap also auto-reads
        //     `OTEL_EXPORTER_OTLP_ENDPOINT` because of `env =` on the
        //     flag, so the flag arm covers both cases).
        //  2. `OtlpEndpoint::from_env()` — picks up
        //     `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` too, with its own
        //     header parsing. Reached when no flag was given AND the
        //     base env var is unset (clap would otherwise have
        //     populated `cli.otlp_endpoint`).
        #[cfg(feature = "otlp")]
        otlp: match cli.otlp_endpoint.clone() {
            Some(endpoint) => {
                let headers = std::env::var("OTEL_EXPORTER_OTLP_HEADERS")
                    .ok()
                    .map(parse_headers_str)
                    .unwrap_or_default();
                Some(tau_observe::otlp::OtlpEndpoint { endpoint, headers })
            }
            None => tau_observe::otlp::OtlpEndpoint::from_env(),
        },
    };
    // The CLI does not propagate install errors; the only failure mode
    // is "already installed", which the underlying installer maps to a
    // no-op guard.
    //
    // The non-blocking guard MUST live for the process lifetime so the
    // appender's worker thread keeps draining the MPSC channel. The
    // CLI `main` returns immediately after the subcommand handler, so
    // we forget the guard rather than `let _ =` it (which would drop
    // at end-of-fn and flush+stop the worker mid-write).
    let guard =
        observe_install(opts).expect("tau_observe::install never returns Err in current impl");
    std::mem::forget(guard);
}

/// Parse `OTEL_EXPORTER_OTLP_HEADERS` form (`key=value,key2=value2`) into a
/// map. Duplicates `tau_observe::otlp::parse_headers` because that helper
/// is module-private; the spec for the header format is fixed by the OTel
/// SDK so divergence is unlikely.
#[cfg(feature = "otlp")]
fn parse_headers_str(raw: String) -> std::collections::HashMap<String, String> {
    raw.split(',')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next()?.trim().to_string();
            let val = parts.next()?.trim().to_string();
            if key.is_empty() {
                None
            } else {
                Some((key, val))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, ColorMode, Command, ListArgs, ListResource, LogRotationCli};

    fn make_cli(verbose: u8, quiet: bool, debug: bool) -> Cli {
        Cli {
            command: Command::List(ListArgs {
                resource: ListResource::Packages,
                global: false,
                all: false,
                capabilities: false,
                dry_run: false,
            }),
            verbose,
            quiet,
            debug,
            color: ColorMode::Auto,
            json: false,
            record_protocol: None,
            no_sandbox: false,
            sandbox: None,
            log_non_blocking: false,
            log_file: None,
            log_rotation: LogRotationCli::Never,
            otlp_endpoint: None,
        }
    }

    /// Serialize tests that mutate `RUST_LOG` against each other in the
    /// same process (cargo test runs unit tests in one binary, so env
    /// state is shared).
    fn rust_log_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn build_filter_default_is_info() {
        let _g = rust_log_lock();
        std::env::remove_var("RUST_LOG");
        let cli = make_cli(0, false, false);
        let filter = build_filter(&cli);
        assert!(filter.to_string().contains("tau=info"), "got: {filter}");
    }

    #[test]
    fn build_filter_minus_v_is_debug() {
        let _g = rust_log_lock();
        std::env::remove_var("RUST_LOG");
        let cli = make_cli(1, false, false);
        let filter = build_filter(&cli);
        assert!(filter.to_string().contains("tau=debug"), "got: {filter}");
    }

    #[test]
    fn build_filter_minus_vv_is_trace() {
        let _g = rust_log_lock();
        std::env::remove_var("RUST_LOG");
        let cli = make_cli(2, false, false);
        let filter = build_filter(&cli);
        assert!(filter.to_string().contains("tau=trace"), "got: {filter}");
    }

    #[test]
    fn build_filter_quiet_is_warn() {
        let _g = rust_log_lock();
        std::env::remove_var("RUST_LOG");
        let cli = make_cli(0, true, false);
        let filter = build_filter(&cli);
        assert!(filter.to_string().contains("tau=warn"), "got: {filter}");
    }

    #[test]
    fn build_filter_debug_is_debug() {
        let _g = rust_log_lock();
        std::env::remove_var("RUST_LOG");
        let cli = make_cli(0, false, true);
        let filter = build_filter(&cli);
        assert!(filter.to_string().contains("tau=debug"), "got: {filter}");
    }

    #[test]
    fn build_filter_rust_log_overrides_flags() {
        let _g = rust_log_lock();
        std::env::set_var("RUST_LOG", "my_plugin=trace");
        let cli = make_cli(0, true, false);
        let filter = build_filter(&cli);
        assert!(
            filter.to_string().contains("my_plugin=trace"),
            "got: {filter}"
        );
        std::env::remove_var("RUST_LOG");
    }

    #[test]
    fn build_filter_scopes_to_tau_when_no_rust_log() {
        let _g = rust_log_lock();
        std::env::remove_var("RUST_LOG");
        let cli = make_cli(0, false, false);
        let filter = build_filter(&cli);
        assert!(filter.to_string().starts_with("tau="), "got: {filter}");
    }

    #[test]
    fn build_filter_minus_vv_takes_precedence_over_minus_v_logic() {
        let _g = rust_log_lock();
        std::env::remove_var("RUST_LOG");
        let cli = make_cli(2, false, false);
        let filter = build_filter(&cli);
        assert!(filter.to_string().contains("tau=trace"));
        assert!(!filter.to_string().contains("tau=debug"));
    }
}
