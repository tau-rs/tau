//! Turn-pacing controller for agent runs.
//!
//! Wraps a [`PidController`](crate::pid::PidController) to pace token
//! consumption across an agent's turn budget. Instead of hard-cutoff
//! budget enforcement (spend freely then crash), the pacer provides
//! continuous feedback about the run's resource trajectory.
//!
//! ## Non-determinism handling
//!
//! LLM outputs are stochastic — token counts and quality signals vary
//! between identical prompts. The pacer addresses this through:
//!
//! 1. **EWMA-filtered measurements** — the PID's derivative term uses an
//!    exponential weighted moving average to suppress noise (inherited
//!    from [`PidController`]).
//! 2. **Rate-based control** — instead of controlling absolute position
//!    (tokens used), the pacer controls the *rate* of consumption
//!    (tokens per turn), which is smoother across stochastic variation.
//! 3. **Dead zone** — small deviations from the target rate produce zero
//!    output, avoiding jitter from normal LLM variance.
//!
//! ## Architecture
//!
//! The pacer sits between the budget watchdog (hard cutoff) and the agent
//! loop (turn execution). It does not *enforce* — it *advises*:
//!
//! ```text
//!  ┌──────────────────────────────────────────────────────┐
//!  │ Agent loop (stream.rs)                               │
//!  │                                                      │
//!  │  ┌──────────┐    ┌──────────┐    ┌────────────────┐  │
//!  │  │ Budget   │    │ Turn     │    │ LLM call +     │  │
//!  │  │ watchdog │───▶│ pacer    │───▶│ tool dispatch  │  │
//!  │  │ (hard)   │    │ (soft)   │    │                │  │
//!  │  └──────────┘    └──────────┘    └────────────────┘  │
//!  └──────────────────────────────────────────────────────┘
//! ```
//!
//! References:
//! - "More with Less: An Empirical Study of Turn-Control Strategies for
//!   Efficient Coding Agents" (arXiv:2510.16786) — dynamic turn
//!   strategies outperform fixed limits by 12-24% cost reduction.
//! - "Adaptive Reasoning Depth in AI Agent Systems" (Zylos Research,
//!   2026) — adaptive allocation delivers 4x better efficiency than
//!   fixed-budget approaches.

use crate::pid::{Gains, PidConfig, PidController};

/// Decision produced by the turn pacer after each turn.
///
/// # Example
///
/// ```
/// use tau_runtime_core::pacing::PacingAdvice;
///
/// let advice = PacingAdvice::Continue { urgency: 0.3 };
/// assert!(!advice.should_stop());
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PacingAdvice {
    /// Continue executing turns. `urgency` ∈ [-1, 1] indicates how
    /// aggressively to consume resources:
    /// - Positive: underspending — can afford more tokens per turn.
    /// - Near zero: on track.
    /// - Negative: overspending — should reduce per-turn effort.
    Continue {
        /// How far off-pace the run is. Magnitude indicates severity.
        urgency: f64,
    },
    /// The pacer recommends stopping early. The run has either:
    /// - Consumed resources so fast that remaining turns would exceed
    ///   the budget even at minimum effort, or
    /// - Converged (quality signal plateaued with low error).
    StopEarly {
        /// Why the pacer recommends stopping.
        reason: StopReason,
    },
}

/// Reason for an early-stop recommendation.
///
/// # Example
///
/// ```
/// use tau_runtime_core::pacing::StopReason;
///
/// let r = StopReason::BudgetExhaustion { fraction_remaining: 0.05 };
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StopReason {
    /// Projected to exhaust the budget before the remaining turns.
    BudgetExhaustion {
        /// Fraction of budget remaining (0.0–1.0).
        fraction_remaining: f64,
    },
    /// Quality/progress signal has plateaued — further turns are
    /// unlikely to improve the outcome.
    Converged {
        /// Number of consecutive steps with below-threshold improvement.
        plateau_steps: u32,
    },
}

impl PacingAdvice {
    /// Whether the advice is to stop.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::pacing::{PacingAdvice, StopReason};
    ///
    /// assert!(!PacingAdvice::Continue { urgency: 0.0 }.should_stop());
    /// assert!(PacingAdvice::StopEarly {
    ///     reason: StopReason::Converged { plateau_steps: 3 }
    /// }.should_stop());
    /// ```
    pub fn should_stop(&self) -> bool {
        matches!(self, Self::StopEarly { .. })
    }
}

/// Configuration for the turn pacer.
///
/// # Example
///
/// ```
/// use tau_runtime_core::pacing::TurnPacerConfig;
///
/// let config = TurnPacerConfig::for_budget(10_000, 16);
/// assert_eq!(config.total_budget, 10_000);
/// assert_eq!(config.total_turns, 16);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TurnPacerConfig {
    /// Total token budget for the run.
    pub total_budget: u64,
    /// Total turns allocated.
    pub total_turns: u32,
    /// PID gains for the rate controller.
    pub pid_gains: Gains,
    /// Fraction of budget remaining below which early stop is advised.
    /// Default: 0.05 (5%).
    pub exhaustion_threshold: f64,
    /// Number of consecutive low-improvement steps before declaring
    /// convergence. Default: 3.
    pub plateau_patience: u32,
    /// Minimum improvement per step to avoid being counted as a plateau
    /// step. Default: 0.01 (1% of budget per turn).
    pub min_improvement: f64,
}

impl TurnPacerConfig {
    /// Create a config for a given budget and turn count with sensible
    /// defaults.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::pacing::TurnPacerConfig;
    ///
    /// let c = TurnPacerConfig::for_budget(50_000, 10);
    /// assert_eq!(c.total_budget, 50_000);
    /// assert_eq!(c.total_turns, 10);
    /// ```
    pub fn for_budget(total_budget: u64, total_turns: u32) -> Self {
        Self {
            total_budget,
            total_turns,
            pid_gains: Gains {
                kp: 0.6,
                ki: 0.15,
                kd: 0.05,
            },
            exhaustion_threshold: 0.05,
            plateau_patience: 3,
            min_improvement: 0.01,
        }
    }
}

/// Tracks token-consumption pace across an agent run using PID control.
///
/// Call [`observe`](TurnPacer::observe) after each turn with the
/// cumulative tokens used. The pacer returns a [`PacingAdvice`] that
/// the caller can use to adjust behavior (or ignore — the pacer is
/// advisory, not enforcing).
///
/// # Example
///
/// ```
/// use tau_runtime_core::pacing::{TurnPacer, TurnPacerConfig, PacingAdvice};
///
/// let config = TurnPacerConfig::for_budget(10_000, 10);
/// let mut pacer = TurnPacer::new(config);
///
/// // Turn 1: used 800 tokens (target pace = 1000/turn)
/// let advice = pacer.observe(1, 800);
/// assert!(!advice.should_stop());
///
/// // Turn 2: used 1900 total (pace = 950/turn ≈ on track)
/// let advice = pacer.observe(2, 1900);
/// assert!(!advice.should_stop());
/// ```
#[derive(Debug, Clone)]
pub struct TurnPacer {
    config: TurnPacerConfig,
    pid: PidController,
    target_rate: f64,
    prev_tokens: u64,
    plateau_count: u32,
    prev_rate: f64,
}

impl TurnPacer {
    /// Create a new turn pacer.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::pacing::{TurnPacer, TurnPacerConfig};
    ///
    /// let pacer = TurnPacer::new(TurnPacerConfig::for_budget(10_000, 10));
    /// ```
    pub fn new(config: TurnPacerConfig) -> Self {
        let target_rate = if config.total_turns > 0 {
            config.total_budget as f64 / config.total_turns as f64
        } else {
            0.0
        };
        let pid_config = PidConfig {
            gains: config.pid_gains,
            output_min: -1.0,
            output_max: 1.0,
            // 5% of target rate as dead zone — normal LLM variance
            dead_zone: target_rate * 0.05,
            ewma_alpha: 0.3,
        };
        Self {
            config,
            pid: PidController::new(pid_config),
            target_rate,
            prev_tokens: 0,
            plateau_count: 0,
            prev_rate: 0.0,
        }
    }

    /// Observe the state after a turn completes.
    ///
    /// - `turn`: 1-indexed turn number that just completed.
    /// - `total_tokens_used`: cumulative tokens used so far.
    ///
    /// Returns advice for the caller.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::pacing::{TurnPacer, TurnPacerConfig, PacingAdvice};
    ///
    /// let mut pacer = TurnPacer::new(TurnPacerConfig::for_budget(1000, 10));
    /// let advice = pacer.observe(1, 100);
    /// // Used exactly the target rate (100 = 1000/10), should be on track
    /// if let PacingAdvice::Continue { urgency } = advice {
    ///     assert!(urgency.abs() < 0.5, "should be near zero urgency");
    /// }
    /// ```
    pub fn observe(&mut self, turn: u32, total_tokens_used: u64) -> PacingAdvice {
        if self.config.total_turns == 0 || self.config.total_budget == 0 {
            return PacingAdvice::Continue { urgency: 0.0 };
        }

        // Check for budget exhaustion
        let fraction_remaining =
            1.0 - (total_tokens_used as f64 / self.config.total_budget as f64);
        if fraction_remaining < self.config.exhaustion_threshold {
            return PacingAdvice::StopEarly {
                reason: StopReason::BudgetExhaustion {
                    fraction_remaining,
                },
            };
        }

        // Compute current consumption rate (tokens this turn)
        let tokens_this_turn = total_tokens_used.saturating_sub(self.prev_tokens);
        let current_rate = tokens_this_turn as f64;
        self.prev_tokens = total_tokens_used;

        // Adaptive target: adjust for remaining budget across remaining turns
        let turns_remaining = self.config.total_turns.saturating_sub(turn);
        let tokens_remaining = self.config.total_budget.saturating_sub(total_tokens_used);
        let adaptive_target = if turns_remaining > 0 {
            tokens_remaining as f64 / turns_remaining as f64
        } else {
            self.target_rate
        };

        // PID update: setpoint = adaptive target rate, measurement = current rate
        // Invert sign: if current_rate > target, error is negative (overspending)
        let urgency = self.pid.update(adaptive_target, current_rate);

        // Convergence detection: track plateau in the *error* signal, not
        // the rate signal. A steady rate that matches the target is good
        // (on-pace), not a plateau. A plateau is when the error stops
        // improving — the controller is stuck.
        let current_error = (adaptive_target - current_rate).abs() / self.target_rate.max(1.0);
        let prev_error = (self.target_rate - self.prev_rate).abs() / self.target_rate.max(1.0);
        let error_improvement = prev_error - current_error;
        // Only count as plateau when: (a) we're off-target (error is
        // significant), AND (b) the error isn't improving.
        let off_target = current_error > self.config.min_improvement;
        if off_target && error_improvement < self.config.min_improvement && turn > 1 {
            self.plateau_count += 1;
        } else {
            self.plateau_count = 0;
        }
        self.prev_rate = current_rate;

        if self.plateau_count >= self.config.plateau_patience {
            return PacingAdvice::StopEarly {
                reason: StopReason::Converged {
                    plateau_steps: self.plateau_count,
                },
            };
        }

        PacingAdvice::Continue { urgency }
    }

    /// Read-only access to the current configuration.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::pacing::{TurnPacer, TurnPacerConfig};
    ///
    /// let pacer = TurnPacer::new(TurnPacerConfig::for_budget(1000, 10));
    /// assert_eq!(pacer.config().total_budget, 1000);
    /// ```
    pub fn config(&self) -> &TurnPacerConfig {
        &self.config
    }

    /// Target tokens-per-turn rate.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::pacing::{TurnPacer, TurnPacerConfig};
    ///
    /// let pacer = TurnPacer::new(TurnPacerConfig::for_budget(1000, 10));
    /// assert!((pacer.target_rate() - 100.0).abs() < 1e-10);
    /// ```
    pub fn target_rate(&self) -> f64 {
        self.target_rate
    }

    /// Current plateau counter (consecutive low-improvement steps).
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::pacing::{TurnPacer, TurnPacerConfig};
    ///
    /// let pacer = TurnPacer::new(TurnPacerConfig::for_budget(1000, 10));
    /// assert_eq!(pacer.plateau_count(), 0);
    /// ```
    pub fn plateau_count(&self) -> u32 {
        self.plateau_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn on_pace_consumption_gives_low_urgency() {
        let mut pacer = TurnPacer::new(TurnPacerConfig::for_budget(10_000, 10));
        // Each turn uses exactly 1000 tokens (target rate)
        for turn in 1..=5 {
            let advice = pacer.observe(turn, turn as u64 * 1000);
            match advice {
                PacingAdvice::Continue { urgency } => {
                    assert!(
                        urgency.abs() < 0.5,
                        "on-pace should produce low urgency, got {urgency} at turn {turn}"
                    );
                }
                other => panic!("expected Continue, got {other:?} at turn {turn}"),
            }
        }
    }

    #[test]
    fn overspending_gives_negative_urgency() {
        let mut pacer = TurnPacer::new(TurnPacerConfig::for_budget(10_000, 10));
        // Turn 1: use 3000 tokens (3x the target of 1000)
        let advice = pacer.observe(1, 3000);
        match advice {
            PacingAdvice::Continue { urgency } => {
                assert!(urgency < 0.0, "overspending should give negative urgency, got {urgency}");
            }
            other => panic!("expected Continue, got {other:?}"),
        }
    }

    #[test]
    fn underspending_gives_positive_urgency() {
        let mut pacer = TurnPacer::new(TurnPacerConfig::for_budget(10_000, 10));
        // Turn 1: use only 200 tokens (target is ~1000)
        let advice = pacer.observe(1, 200);
        match advice {
            PacingAdvice::Continue { urgency } => {
                assert!(urgency > 0.0, "underspending should give positive urgency, got {urgency}");
            }
            other => panic!("expected Continue, got {other:?}"),
        }
    }

    #[test]
    fn budget_exhaustion_triggers_early_stop() {
        let mut pacer = TurnPacer::new(TurnPacerConfig::for_budget(1000, 10));
        // Blow 96% of budget in one turn
        let advice = pacer.observe(1, 960);
        assert!(
            advice.should_stop(),
            "should recommend stop when budget nearly exhausted"
        );
        match advice {
            PacingAdvice::StopEarly {
                reason: StopReason::BudgetExhaustion { fraction_remaining },
            } => {
                assert!(fraction_remaining < 0.05);
            }
            other => panic!("expected BudgetExhaustion, got {other:?}"),
        }
    }

    #[test]
    fn plateau_detection_triggers_convergence_stop() {
        let config = TurnPacerConfig {
            plateau_patience: 3,
            min_improvement: 0.01,
            ..TurnPacerConfig::for_budget(100_000, 20)
        };
        let mut pacer = TurnPacer::new(config);
        // Target rate = 100_000 / 20 = 5000 per turn.
        // Simulate consistently overspending at 8000/turn — the error
        // stays large and doesn't improve, which is a plateau.
        pacer.observe(1, 8000);
        pacer.observe(2, 16_000);
        pacer.observe(3, 24_000);
        pacer.observe(4, 32_000);
        pacer.observe(5, 40_000);
        let advice = pacer.observe(6, 48_000);
        assert!(
            advice.should_stop(),
            "should detect plateau when error is stuck (consistently off-target \
             with no improvement), got {advice:?}"
        );
    }

    #[test]
    fn adaptive_target_adjusts_for_remaining_budget() {
        // Verify that the pacer adapts to remaining budget: after an
        // overspend, spending the *new* adaptive rate should eventually
        // produce near-zero urgency (the integral unwinds). We use a
        // P-only controller (ki=0, kd=0) so output == Kp * error, making
        // the clamping and integral dynamics predictable.
        let config = TurnPacerConfig {
            pid_gains: Gains {
                kp: 0.001,
                ki: 0.0,
                kd: 0.0,
            },
            // High patience so convergence detection doesn't interfere
            // with the adaptive-target test.
            plateau_patience: 100,
            ..TurnPacerConfig::for_budget(100_000, 10)
        };
        let mut pacer = TurnPacer::new(config);
        // Turn 1: overspend 15000 vs target 10000
        let advice1 = pacer.observe(1, 15_000);
        // Turns 2-5: spend exactly the new adaptive target each time.
        // Adaptive target = remaining / remaining_turns, so the error
        // should shrink toward zero.
        let mut total = 15_000u64;
        let mut last_urgency = match advice1 {
            PacingAdvice::Continue { urgency } => urgency.abs(),
            other => panic!("expected Continue, got {other:?}"),
        };
        for turn in 2..=5 {
            let remaining = 100_000u64.saturating_sub(total);
            let turns_left = 10u32.saturating_sub(turn);
            let adaptive = remaining / turns_left as u64;
            total += adaptive;
            let advice = pacer.observe(turn, total);
            match advice {
                PacingAdvice::Continue { urgency } => {
                    assert!(
                        urgency.abs() <= last_urgency + 0.01,
                        "urgency should not grow when matching adaptive target: \
                         turn={turn}, urgency={urgency}, prev={last_urgency}"
                    );
                    last_urgency = urgency.abs();
                }
                other => panic!("expected Continue at turn {turn}, got {other:?}"),
            }
        }
    }

    #[test]
    fn zero_budget_returns_neutral() {
        let mut pacer = TurnPacer::new(TurnPacerConfig::for_budget(0, 10));
        let advice = pacer.observe(1, 0);
        assert_eq!(advice, PacingAdvice::Continue { urgency: 0.0 });
    }

    #[test]
    fn zero_turns_returns_neutral() {
        let mut pacer = TurnPacer::new(TurnPacerConfig::for_budget(1000, 0));
        let advice = pacer.observe(0, 0);
        assert_eq!(advice, PacingAdvice::Continue { urgency: 0.0 });
    }

    #[test]
    fn noisy_signal_does_not_cause_oscillation() {
        let mut pacer = TurnPacer::new(TurnPacerConfig::for_budget(10_000, 10));
        // Simulate noisy token consumption oscillating around the target
        let consumptions = [980, 1020, 970, 1030, 990, 1010, 985, 1015];
        let mut total = 0u64;
        let mut urgencies = Vec::new();
        for (i, &tokens) in consumptions.iter().enumerate() {
            total += tokens;
            if let PacingAdvice::Continue { urgency } = pacer.observe(i as u32 + 1, total) {
                urgencies.push(urgency);
            }
        }
        // All urgency values should be small (near zero) since we're on pace
        let max_urgency = urgencies.iter().map(|u| u.abs()).fold(0.0f64, f64::max);
        assert!(
            max_urgency < 0.5,
            "noisy but on-pace signal should not cause large urgency swings: max={max_urgency}"
        );
    }
}
