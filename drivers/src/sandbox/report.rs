//! The report the shim leaves for the driver: a JSON file in the scratch
//! directory, written twice (ADR-0009 §4, §5).
//!
//! Once at spawn, with the interpreter's pid and no outcome, so the driver's
//! last resort can reach the interpreter's process group even if the shim
//! is gone. Once at the end, whole. Both writes go through a rename, so the
//! driver never reads half a report.
//!
//! This file is compiled into both the library and the `tau-sandbox-shim`
//! binary by path, so the binary depends on neither the library nor the
//! kernel crate. It is the private contract between the two halves of one
//! crate, versioned with the crate, not a wire format anyone else reads.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// What the shim knows about the run, at spawn and at the end.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Report {
    /// The interpreter's pid, which is also its process group id: the shim
    /// spawned it into a group of its own.
    pub(crate) interpreter_pid: u32,
    /// How the run ended. `None` in the report written at spawn.
    #[serde(default)]
    pub(crate) outcome: Option<Outcome>,
}

/// How a run ended, and what it used.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Outcome {
    /// The way it stopped.
    pub(crate) end: End,
    /// `getrusage(RUSAGE_CHILDREN)` after the shim reaped the interpreter.
    pub(crate) usage: RawUsage,
}

/// The shim's own view of a stop, before the driver turns it into the wire
/// `stop` of ADR-0009 §2.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum End {
    /// Exited with this status.
    Exit(i32),
    /// Died of a signal it did not ask for, named (`SIGSEGV`).
    Signal(String),
    /// The host kernel killed it at `RLIMIT_CPU`.
    CpuLimit,
    /// The shim killed it at the wall bound.
    WallLimit,
    /// The shim killed it on the driver's `SIGTERM`.
    Abandoned,
    /// The shim could not start the interpreter: nothing ran.
    HostError(String),
}

/// CPU time and peak memory in the interpreter's native units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RawUsage {
    /// User CPU time, microseconds.
    pub(crate) cpu_user_us: u64,
    /// System CPU time, microseconds.
    pub(crate) cpu_sys_us: u64,
    /// Peak resident set size, bytes.
    pub(crate) max_rss_bytes: u64,
}

impl Report {
    /// Writes the report to `path` atomically: a sibling temporary file,
    /// then a rename over `path`.
    ///
    /// # Errors
    ///
    /// Whatever the filesystem refuses.
    pub(crate) fn write(&self, path: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec(self).map_err(io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)
    }

    /// Reads a report from `path`.
    ///
    /// # Errors
    ///
    /// The file is missing or unreadable, or is not a report.
    pub(crate) fn read(path: &Path) -> io::Result<Self> {
        let bytes = std::fs::read(path)?;
        serde_json::from_slice(&bytes).map_err(io::Error::other)
    }
}
