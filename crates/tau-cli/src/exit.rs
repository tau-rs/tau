//! Process exit code taxonomy for tau-cli.
//!
//! Four buckets per the Outcome/Error dichotomy from ADR-0006:
//! - [`ExitCode::Success`] (0) — operation completed successfully.
//! - [`ExitCode::AgentFailed`] (1) — `tau run` only: agent ran but
//!   couldn't accomplish the task (`RunOutcome::Failed`).
//! - [`ExitCode::Error`] (2) — kernel/CLI broke (`RuntimeError`,
//!   `InstallError`, parse error, argument error, etc.).
//! - [`ExitCode::Suspended`] (3) — `tau run` only: a pipeline paused at
//!   a `Suspend` step (HITL). Not a failure — resumable with `--resume`.
//!
//! Other subcommands (`install`, `list`, `init`, `chat`) only ever
//! produce 0 or 2; buckets 1 and 3 are reserved for `tau run`'s
//! graceful agent failures / pipeline suspensions.

use tau_runtime_tokio::RunOutcome;

/// Process exit code mapped from `RunOutcome` / errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// Operation completed successfully (`RunOutcome::Completed` or
    /// non-`run` success).
    Success,
    /// `tau run` only: agent ran but failed gracefully.
    AgentFailed,
    /// Kernel error, CLI argument error, install failure, etc.
    Error,
    /// `tau run` only: a pipeline paused at a `Suspend` step (HITL). Not
    /// a failure — the run can be resumed with `--resume`.
    Suspended,
}

impl From<&RunOutcome> for ExitCode {
    fn from(outcome: &RunOutcome) -> Self {
        match outcome {
            RunOutcome::Completed { .. } => ExitCode::Success,
            RunOutcome::Failed { .. } => ExitCode::AgentFailed,
            // RunOutcome is `#[non_exhaustive]`; cross-crate matches require
            // a wildcard arm even when every current variant is named. Any
            // future variant added to tau-runtime should be classified
            // explicitly via an ADR amendment; until then, treat unknown
            // outcomes as a kernel error so we exit non-zero rather than
            // silently claim success.
            _ => ExitCode::Error,
        }
    }
}

impl From<&anyhow::Error> for ExitCode {
    fn from(_: &anyhow::Error) -> Self {
        ExitCode::Error
    }
}

impl From<ExitCode> for std::process::ExitCode {
    fn from(code: ExitCode) -> Self {
        match code {
            ExitCode::Success => Self::SUCCESS,
            ExitCode::AgentFailed => Self::from(1),
            ExitCode::Error => Self::from(2),
            // Code 3 is shared with spec §C.3's `BundleVerifyFailed` (routed
            // via `bvf.code`, not this enum). The overload is unambiguous:
            // the two never co-occur in one invocation — a `--bundle` run
            // that hits a `Suspend` step errors out at exit 2 before any
            // suspension is emitted, so exit 3 means "verify failed" for a
            // `--bundle` run and "suspended" for a plain `tau run`.
            ExitCode::Suspended => Self::from(3),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anyhow_error_maps_to_error_bucket() {
        let err = anyhow::anyhow!("boom");
        let code = ExitCode::from(&err);
        assert_eq!(code, ExitCode::Error);
    }

    #[test]
    fn success_maps_to_process_success() {
        let process_code: std::process::ExitCode = ExitCode::Success.into();
        // std::process::ExitCode doesn't implement PartialEq, but it does Debug.
        // Compare via debug formatting (stable in stdlib).
        assert_eq!(
            format!("{:?}", process_code),
            format!("{:?}", std::process::ExitCode::SUCCESS)
        );
    }

    #[test]
    fn agent_failed_maps_to_process_one() {
        let process_code: std::process::ExitCode = ExitCode::AgentFailed.into();
        assert_eq!(
            format!("{:?}", process_code),
            format!("{:?}", std::process::ExitCode::from(1))
        );
    }

    #[test]
    fn error_maps_to_process_two() {
        let process_code: std::process::ExitCode = ExitCode::Error.into();
        assert_eq!(
            format!("{:?}", process_code),
            format!("{:?}", std::process::ExitCode::from(2))
        );
    }

    #[test]
    fn suspended_maps_to_process_three() {
        let process_code: std::process::ExitCode = ExitCode::Suspended.into();
        assert_eq!(
            format!("{:?}", process_code),
            format!("{:?}", std::process::ExitCode::from(3))
        );
    }
}
