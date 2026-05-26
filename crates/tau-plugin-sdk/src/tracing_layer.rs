//! tracing-subscriber JSON layer that writes structured events to
//! stderr. The host (in `tau-runtime::plugin_host`) reads each line,
//! decodes the JSON, and re-emits as a `tracing::Event` on
//! `target = "plugin::<plugin_name>"`.
//!
//! Internals delegate to [`tau_observe::install`] so all tau crates
//! share one subscriber-init code path.

/// Install the SDK's stderr-JSON tracing layer.
///
/// Idempotent: subsequent calls are no-ops. Plugin authors should
/// call this once at the start of `main()`, OR call one of the
/// `run_*` runners (which install it internally).
///
/// The default filter level is `info`; override via `RUST_LOG` env var
/// (e.g., `RUST_LOG=tau_plugin_sdk=debug,my_plugin=trace`).
///
/// # Example
///
/// ```
/// // Calling install() more than once is safe — subsequent calls are no-ops.
/// tau_plugin_sdk::tracing_layer::install();
/// tau_plugin_sdk::tracing_layer::install();
/// // No panic means install is idempotent.
/// assert!(true);
/// ```
pub fn install() {
    let _guard = tau_observe::install::install(tau_observe::install::InstallOptions::plugin_sdk())
        .expect("tau_observe::install never returns Err in current impl");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_is_idempotent() {
        // Call twice; second call should be a no-op (no panic).
        install();
        install();
    }
}
