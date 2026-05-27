//! Configuration for the serve loop.

use std::path::PathBuf;
use std::time::Duration;

/// Configuration for [`super::run`].
///
/// All fields have safe defaults so callers can construct
/// `ServeOptions::default()` and override only what they need.
///
/// ```
/// use tau_app::serve::ServeOptions;
/// use std::time::Duration;
///
/// let opts = ServeOptions {
///     max_concurrent: 4,
///     ready_on_stderr: true,
///     shutdown_grace: Duration::from_secs(10),
///     ..ServeOptions::default()
/// };
/// assert_eq!(opts.max_concurrent, 4);
/// assert!(opts.ready_on_stderr);
/// assert_eq!(opts.shutdown_grace, Duration::from_secs(10));
/// assert!(opts.idle_timeout.is_none());
/// ```
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// Absolute path to the tau project directory. Defaults to cwd
    /// when constructed via [`ServeOptions::default`].
    pub project_path: PathBuf,

    /// Maximum number of concurrent in-flight runs. Default 8.
    /// New requests beyond this cap receive error -32004 immediately.
    pub max_concurrent: usize,

    /// If `Some(d)`, the server initiates graceful shutdown after no
    /// message has been received OR emitted for `d`. Default `None`
    /// (run until external shutdown signal).
    pub idle_timeout: Option<Duration>,

    /// If true, the server writes `"tau-serve ready\n"` to stderr
    /// after startup completes (runtime built, reader/dispatcher/writer
    /// tasks alive). Lets parent processes synchronize on readiness.
    pub ready_on_stderr: bool,

    /// Max duration to wait for in-flight tasks to drain on graceful
    /// shutdown before dropping the runtime and exiting. Default 5s.
    pub shutdown_grace: Duration,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            project_path: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            max_concurrent: 8,
            idle_timeout: None,
            ready_on_stderr: false,
            shutdown_grace: Duration::from_secs(5),
        }
    }
}
