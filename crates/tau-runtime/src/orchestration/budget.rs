//! Budget tracking + breach detection.
//!
//! The runtime calls `BudgetWatchdog::tick(&state)` at every turn
//! boundary + after each tool result. On breach, returns
//! `BudgetExceeded` — the caller is responsible for aborting agents.

use chrono::{DateTime, Utc};

use crate::orchestration::error::OrchestrationError;
use crate::orchestration::run_state::RunState;

/// Watchdog handle. Stateless; checks are pure functions of RunState + now.
///
/// # Example
///
/// ```
/// use chrono::Utc;
/// use tau_runtime::orchestration::budget::BudgetWatchdog;
/// use tau_runtime::orchestration::run_state::RunState;
/// use tau_ports::RunBudget;
///
/// let state = RunState::new("run-1".into(), "agent-1".into(), RunBudget::default(), Utc::now());
/// // Default budget has no limits — tick always succeeds.
/// BudgetWatchdog.tick(&state, Utc::now()).expect("within default budget");
/// ```
pub struct BudgetWatchdog;

impl BudgetWatchdog {
    /// New watchdog.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime::orchestration::budget::BudgetWatchdog;
    ///
    /// let _watchdog = BudgetWatchdog::new();
    /// // Stateless — construction always succeeds.
    /// assert!(true, "BudgetWatchdog::new() never fails");
    /// ```
    pub fn new() -> Self {
        Self
    }

    /// Returns `Ok(())` if within budget, `Err(BudgetExceeded { ... })` otherwise.
    /// Caller emits a TraceEventKind::BudgetExceeded + aborts.
    ///
    /// # Example
    ///
    /// ```
    /// use chrono::Utc;
    /// use tau_runtime::orchestration::budget::BudgetWatchdog;
    /// use tau_runtime::orchestration::run_state::RunState;
    /// use tau_runtime::orchestration::error::OrchestrationError;
    /// use tau_ports::RunBudget;
    ///
    /// let state = RunState::new(
    ///     "run-1".into(),
    ///     "agent-1".into(),
    ///     RunBudget { max_total_tokens: Some(100), ..Default::default() },
    ///     Utc::now(),
    /// );
    /// state.add_tokens(50);
    /// BudgetWatchdog.tick(&state, Utc::now()).expect("50 < 100: within budget");
    ///
    /// state.add_tokens(60); // now 110 > 100
    /// let err = BudgetWatchdog.tick(&state, Utc::now()).unwrap_err();
    /// assert!(matches!(err, OrchestrationError::BudgetExceeded { .. }));
    /// ```
    pub fn tick(&self, state: &RunState, now: DateTime<Utc>) -> Result<(), OrchestrationError> {
        if let Some(limit) = state.budget.max_total_tokens {
            let used = state.tokens_used.load(std::sync::atomic::Ordering::Relaxed);
            if used > limit {
                return Err(OrchestrationError::BudgetExceeded {
                    budget: "max_total_tokens".into(),
                    value: used,
                    limit,
                });
            }
        }
        if let Some(limit) = state.budget.max_total_duration_secs {
            let elapsed = (now - state.started_at).num_seconds().max(0) as u64;
            if elapsed > limit {
                return Err(OrchestrationError::BudgetExceeded {
                    budget: "max_total_duration_secs".into(),
                    value: elapsed,
                    limit,
                });
            }
        }
        if let Some(limit) = state.budget.max_total_agents {
            let spawned = state
                .agents_spawned
                .load(std::sync::atomic::Ordering::Relaxed);
            if spawned > limit {
                return Err(OrchestrationError::BudgetExceeded {
                    budget: "max_total_agents".into(),
                    value: spawned as u64,
                    limit: limit as u64,
                });
            }
        }
        Ok(())
    }
}

impl Default for BudgetWatchdog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::RunBudget;

    fn now_at(secs: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn tick_ok_when_within_budget() {
        let state = RunState::new(
            "r".into(),
            "a".into(),
            RunBudget {
                max_total_tokens: Some(1000),
                ..Default::default()
            },
            now_at(0),
        );
        state.add_tokens(500);
        BudgetWatchdog.tick(&state, now_at(0)).unwrap();
    }

    #[test]
    fn tick_fails_when_tokens_exceeded() {
        let state = RunState::new(
            "r".into(),
            "a".into(),
            RunBudget {
                max_total_tokens: Some(100),
                ..Default::default()
            },
            now_at(0),
        );
        state.add_tokens(101);
        let err = BudgetWatchdog.tick(&state, now_at(0)).unwrap_err();
        assert!(matches!(err, OrchestrationError::BudgetExceeded { .. }));
    }

    #[test]
    fn tick_fails_when_duration_exceeded() {
        let state = RunState::new(
            "r".into(),
            "a".into(),
            RunBudget {
                max_total_duration_secs: Some(60),
                ..Default::default()
            },
            now_at(0),
        );
        let err = BudgetWatchdog.tick(&state, now_at(120)).unwrap_err();
        assert!(matches!(err, OrchestrationError::BudgetExceeded { .. }));
    }

    #[test]
    fn tick_fails_when_agents_exceeded() {
        let state = RunState::new(
            "r".into(),
            "a".into(),
            RunBudget {
                max_total_agents: Some(2),
                ..Default::default()
            },
            now_at(0),
        );
        state.record_agent_spawn();
        state.record_agent_spawn();
        state.record_agent_spawn();
        let err = BudgetWatchdog.tick(&state, now_at(0)).unwrap_err();
        assert!(matches!(err, OrchestrationError::BudgetExceeded { .. }));
    }
}
