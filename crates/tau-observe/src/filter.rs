//! Shared `EnvFilter` builders.
//!
//! Every tau binary or library that initializes a tracing subscriber
//! goes through these helpers so the resolution order (RUST_LOG > caller
//! directive > default) is identical everywhere.

use tracing_subscriber::filter::EnvFilter;

/// Build an `EnvFilter` from the `RUST_LOG` env var if set, otherwise
/// from the `fallback` directive (e.g. `"tau=info"`).
///
/// `RUST_LOG` is parsed verbatim. The fallback is *not* a default for a
/// missing var key — it is the entire filter, used only when `RUST_LOG`
/// is unset.
///
/// ```
/// use tau_observe::filter::env_or_directive;
///
/// // When RUST_LOG is unset, the fallback directive is used.
/// std::env::remove_var("RUST_LOG");
/// let filter = env_or_directive("tau=info");
/// assert!(filter.to_string().contains("tau=info"));
/// ```
pub fn env_or_directive(fallback: &str) -> EnvFilter {
    if let Ok(env) = std::env::var("RUST_LOG") {
        return EnvFilter::new(env);
    }
    EnvFilter::new(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn rust_log_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn fallback_used_when_rust_log_unset() {
        let _g = rust_log_lock();
        std::env::remove_var("RUST_LOG");
        let f = env_or_directive("tau=info");
        assert!(f.to_string().contains("tau=info"), "got: {f}");
    }

    #[test]
    fn rust_log_overrides_fallback() {
        let _g = rust_log_lock();
        std::env::set_var("RUST_LOG", "my_plugin=trace");
        let f = env_or_directive("tau=info");
        assert!(f.to_string().contains("my_plugin=trace"), "got: {f}");
        std::env::remove_var("RUST_LOG");
    }

    #[test]
    fn empty_rust_log_still_overrides() {
        let _g = rust_log_lock();
        std::env::set_var("RUST_LOG", "");
        let f = env_or_directive("tau=info");
        // EnvFilter::new("") yields an empty filter — that's the intent
        // when the user explicitly clears RUST_LOG.
        assert!(!f.to_string().contains("tau=info"), "got: {f}");
        std::env::remove_var("RUST_LOG");
    }
}
