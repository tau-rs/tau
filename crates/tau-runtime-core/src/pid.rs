//! PID controller with EWMA filtering for noisy (non-deterministic) signals.
//!
//! Designed for controlling LLM agent turn pacing, where the process
//! variable is inherently stochastic. Key adaptations for non-deterministic
//! environments:
//!
//! - **EWMA low-pass filter** on the derivative term to suppress noise
//!   amplification (the D term differentiates noise without filtering).
//! - **Integral anti-windup** via output clamping to prevent runaway
//!   accumulation in bounded systems (finite turns, finite budget).
//! - **Dead-zone** around the setpoint where the controller outputs zero,
//!   avoiding jitter from measurement noise near the target.
//!
//! References:
//! - Karn 2025, "Linear Feedback Control Systems for Iterative Prompt
//!   Optimization in Large Language Models" (arXiv:2501.11979)
//! - Li et al. 2025, "Activation Steering with a Feedback Controller"
//!   (arXiv:2510.04309) — PID-Steering for LLMs
//! - Chen et al. 2025, "Adaptive Activation Steering for Efficient LLM
//!   Reasoning via Closed-Loop PID Control" (arXiv:2506.18831)

/// PID gains. All three terms are optional: set a gain to 0.0 to disable it.
///
/// # Example
///
/// ```
/// use tau_runtime_core::pid::Gains;
///
/// let gains = Gains { kp: 1.0, ki: 0.1, kd: 0.05 };
/// assert!(gains.kp > 0.0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Gains {
    /// Proportional gain — responds to current error magnitude.
    pub kp: f64,
    /// Integral gain — corrects accumulated steady-state error.
    pub ki: f64,
    /// Derivative gain — dampens rate-of-change oscillations.
    pub kd: f64,
}

impl Default for Gains {
    /// Conservative default: proportional-only with no I or D.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::pid::Gains;
    ///
    /// let g = Gains::default();
    /// assert_eq!(g.kp, 1.0);
    /// assert_eq!(g.ki, 0.0);
    /// assert_eq!(g.kd, 0.0);
    /// ```
    fn default() -> Self {
        Self {
            kp: 1.0,
            ki: 0.0,
            kd: 0.0,
        }
    }
}

/// Configuration for the PID controller.
///
/// # Example
///
/// ```
/// use tau_runtime_core::pid::{Gains, PidConfig};
///
/// let config = PidConfig {
///     gains: Gains { kp: 0.8, ki: 0.1, kd: 0.05 },
///     output_min: -1.0,
///     output_max: 1.0,
///     dead_zone: 0.05,
///     ewma_alpha: 0.3,
/// };
/// assert_eq!(config.dead_zone, 0.05);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PidConfig {
    /// PID gains.
    pub gains: Gains,
    /// Minimum controller output (anti-windup clamp).
    pub output_min: f64,
    /// Maximum controller output (anti-windup clamp).
    pub output_max: f64,
    /// Error magnitude below which the controller outputs zero.
    /// Prevents jitter from measurement noise near the setpoint.
    pub dead_zone: f64,
    /// EWMA smoothing factor for the derivative filter (0 < alpha <= 1).
    /// Lower values = more smoothing (slower response to real changes).
    /// Higher values = less smoothing (more responsive but noisier).
    /// 0.3 is a reasonable default for LLM output variance.
    pub ewma_alpha: f64,
}

impl Default for PidConfig {
    /// Default config: moderate P, no I/D, symmetric output [-1, 1].
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::pid::PidConfig;
    ///
    /// let c = PidConfig::default();
    /// assert_eq!(c.output_min, -1.0);
    /// assert_eq!(c.output_max, 1.0);
    /// ```
    fn default() -> Self {
        Self {
            gains: Gains::default(),
            output_min: -1.0,
            output_max: 1.0,
            dead_zone: 0.0,
            ewma_alpha: 0.3,
        }
    }
}

/// A discrete-time PID controller with EWMA-filtered derivative and
/// integral anti-windup.
///
/// Designed to operate in non-deterministic environments where the
/// measured signal is noisy (e.g., LLM output quality scores, token
/// consumption rates). The EWMA filter on the derivative term prevents
/// noise amplification while preserving real trend detection.
///
/// # Example
///
/// ```
/// use tau_runtime_core::pid::{PidConfig, Gains, PidController};
///
/// let config = PidConfig {
///     gains: Gains { kp: 0.5, ki: 0.1, kd: 0.05 },
///     output_min: -1.0,
///     output_max: 1.0,
///     dead_zone: 0.02,
///     ewma_alpha: 0.3,
/// };
/// let mut pid = PidController::new(config);
///
/// // Setpoint = 0.8, measurement = 0.5 → positive error → positive output
/// let output = pid.update(0.8, 0.5);
/// assert!(output > 0.0, "should correct upward");
///
/// // After several steps converging, output should decrease
/// let _ = pid.update(0.8, 0.6);
/// let _ = pid.update(0.8, 0.7);
/// let output_near = pid.update(0.8, 0.78);
/// assert!(output_near < output, "correction should decrease as we converge");
/// ```
#[derive(Debug, Clone)]
pub struct PidController {
    config: PidConfig,
    integral: f64,
    prev_error: Option<f64>,
    filtered_derivative: f64,
}

impl PidController {
    /// Create a new PID controller with the given configuration.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::pid::{PidConfig, PidController};
    ///
    /// let pid = PidController::new(PidConfig::default());
    /// ```
    pub fn new(config: PidConfig) -> Self {
        Self {
            config,
            integral: 0.0,
            prev_error: None,
            filtered_derivative: 0.0,
        }
    }

    /// Compute one control step.
    ///
    /// Returns the clamped controller output given the current setpoint
    /// and measurement. Call once per discrete time step (e.g., once per
    /// agent turn or once per loop iteration).
    ///
    /// The error is `setpoint - measurement`. Positive output means
    /// "increase effort"; negative means "reduce effort".
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::pid::{PidConfig, Gains, PidController};
    ///
    /// let mut pid = PidController::new(PidConfig {
    ///     gains: Gains { kp: 1.0, ki: 0.0, kd: 0.0 },
    ///     ..PidConfig::default()
    /// });
    /// // Pure P controller: output = error * kp
    /// let out = pid.update(1.0, 0.6);
    /// assert!((out - 0.4).abs() < 1e-10);
    /// ```
    pub fn update(&mut self, setpoint: f64, measurement: f64) -> f64 {
        let error = setpoint - measurement;

        if error.abs() < self.config.dead_zone {
            return 0.0;
        }

        // P term
        let p = self.config.gains.kp * error;

        // I term with anti-windup clamping
        self.integral += error;
        let i_raw = self.config.gains.ki * self.integral;
        let i = clamp(i_raw, self.config.output_min, self.config.output_max);
        if (i - i_raw).abs() > f64::EPSILON {
            // Back-calculate: undo the excess accumulation that caused clamping
            self.integral = i / self.config.gains.ki.max(f64::EPSILON);
        }

        // D term with EWMA low-pass filter for noise rejection
        let raw_derivative = match self.prev_error {
            Some(prev) => error - prev,
            None => 0.0,
        };
        self.filtered_derivative = self.config.ewma_alpha * raw_derivative
            + (1.0 - self.config.ewma_alpha) * self.filtered_derivative;
        let d = self.config.gains.kd * self.filtered_derivative;

        self.prev_error = Some(error);

        clamp(p + i + d, self.config.output_min, self.config.output_max)
    }

    /// Reset the controller state (integral accumulator, derivative
    /// history). Useful when the setpoint changes discontinuously.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::pid::{PidConfig, PidController};
    ///
    /// let mut pid = PidController::new(PidConfig::default());
    /// let _ = pid.update(1.0, 0.5);
    /// pid.reset();
    /// ```
    pub fn reset(&mut self) {
        self.integral = 0.0;
        self.prev_error = None;
        self.filtered_derivative = 0.0;
    }

    /// Read-only access to the current configuration.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::pid::{PidConfig, PidController};
    ///
    /// let pid = PidController::new(PidConfig::default());
    /// assert_eq!(pid.config().gains.kp, 1.0);
    /// ```
    pub fn config(&self) -> &PidConfig {
        &self.config
    }

    /// Current integral accumulator value (for diagnostics/tracing).
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::pid::{PidConfig, Gains, PidController};
    ///
    /// let mut pid = PidController::new(PidConfig {
    ///     gains: Gains { kp: 0.0, ki: 1.0, kd: 0.0 },
    ///     ..PidConfig::default()
    /// });
    /// let _ = pid.update(1.0, 0.5); // error = 0.5
    /// assert!((pid.integral() - 0.5).abs() < 1e-10);
    /// ```
    pub fn integral(&self) -> f64 {
        self.integral
    }

    /// Current EWMA-filtered derivative value (for diagnostics/tracing).
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::pid::{PidConfig, PidController};
    ///
    /// let pid = PidController::new(PidConfig::default());
    /// assert_eq!(pid.filtered_derivative(), 0.0);
    /// ```
    pub fn filtered_derivative(&self) -> f64 {
        self.filtered_derivative
    }
}

fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn make_pid(kp: f64, ki: f64, kd: f64) -> PidController {
        PidController::new(PidConfig {
            gains: Gains { kp, ki, kd },
            output_min: -10.0,
            output_max: 10.0,
            dead_zone: 0.0,
            ewma_alpha: 0.3,
        })
    }

    #[test]
    fn pure_p_controller_outputs_proportional_to_error() {
        let mut pid = make_pid(2.0, 0.0, 0.0);
        let out = pid.update(1.0, 0.5);
        assert!((out - 1.0).abs() < 1e-10, "2.0 * 0.5 = 1.0");
    }

    #[test]
    fn pure_i_controller_accumulates_error() {
        let mut pid = make_pid(0.0, 1.0, 0.0);
        let _ = pid.update(1.0, 0.5); // integral = 0.5
        let out = pid.update(1.0, 0.5); // integral = 1.0
        assert!((out - 1.0).abs() < 1e-10);
    }

    #[test]
    fn pure_d_controller_responds_to_error_change() {
        let mut pid = make_pid(0.0, 0.0, 1.0);
        let out1 = pid.update(1.0, 0.5); // first step: no prev, derivative = 0
        assert!(out1.abs() < 1e-10);

        // error stays same → derivative ≈ 0
        let out2 = pid.update(1.0, 0.5);
        assert!(out2.abs() < 0.01);

        // error increases → positive derivative
        let out3 = pid.update(1.0, 0.2);
        assert!(out3 > 0.0, "increasing error → positive derivative");
    }

    #[test]
    fn output_clamping_prevents_runaway() {
        let mut pid = PidController::new(PidConfig {
            gains: Gains {
                kp: 100.0,
                ki: 0.0,
                kd: 0.0,
            },
            output_min: -1.0,
            output_max: 1.0,
            dead_zone: 0.0,
            ewma_alpha: 0.3,
        });
        let out = pid.update(1.0, 0.0); // error=1.0, P=100.0, clamped to 1.0
        assert!((out - 1.0).abs() < 1e-10);
    }

    #[test]
    fn integral_anti_windup_limits_accumulation() {
        let mut pid = PidController::new(PidConfig {
            gains: Gains {
                kp: 0.0,
                ki: 1.0,
                kd: 0.0,
            },
            output_min: -2.0,
            output_max: 2.0,
            dead_zone: 0.0,
            ewma_alpha: 0.3,
        });
        // Pump error repeatedly — integral should not grow past output_max
        for _ in 0..100 {
            pid.update(1.0, 0.0);
        }
        assert!(
            pid.integral() <= 2.0 + f64::EPSILON,
            "integral should be clamped"
        );
    }

    #[test]
    fn dead_zone_suppresses_small_errors() {
        let mut pid = PidController::new(PidConfig {
            gains: Gains {
                kp: 10.0,
                ki: 0.0,
                kd: 0.0,
            },
            output_min: -10.0,
            output_max: 10.0,
            dead_zone: 0.1,
            ewma_alpha: 0.3,
        });
        let out = pid.update(1.0, 0.95); // error = 0.05 < dead_zone = 0.1
        assert!(out.abs() < 1e-10, "within dead zone → zero output");

        let out = pid.update(1.0, 0.8); // error = 0.2 > dead_zone
        assert!(out > 0.0, "outside dead zone → nonzero output");
    }

    #[test]
    fn ewma_filter_smooths_derivative_noise() {
        let mut pid = make_pid(0.0, 0.0, 1.0);
        // Simulate noisy signal oscillating around 0.5 error
        let measurements = [0.5, 0.48, 0.52, 0.49, 0.51, 0.5, 0.48, 0.52];
        let mut outputs = Vec::new();
        for &m in &measurements {
            outputs.push(pid.update(1.0, m));
        }
        // Derivative outputs should be small since the signal is stationary
        // (oscillating around the same value). Without EWMA filtering, the
        // raw derivative would swing wildly.
        let max_abs = outputs.iter().map(|o| o.abs()).fold(0.0f64, f64::max);
        assert!(
            max_abs < 0.1,
            "EWMA should dampen derivative noise: max_abs={max_abs}"
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut pid = make_pid(1.0, 1.0, 1.0);
        let _ = pid.update(1.0, 0.0);
        let _ = pid.update(1.0, 0.0);
        pid.reset();
        assert!(pid.integral().abs() < 1e-10);
        assert!(pid.filtered_derivative().abs() < 1e-10);
    }

    #[test]
    fn convergence_reduces_output_over_time() {
        let mut pid = make_pid(1.0, 0.1, 0.05);
        let mut measurement = 0.0;
        let setpoint = 1.0;
        let mut outputs = Vec::new();
        // Simulate a converging system: measurement approaches setpoint
        for _ in 0..20 {
            let out = pid.update(setpoint, measurement);
            outputs.push(out.abs());
            measurement += 0.05; // steady improvement
        }
        // Output magnitude should generally decrease as we approach setpoint
        let first_half_avg: f64 =
            outputs[..10].iter().sum::<f64>() / 10.0;
        let second_half_avg: f64 =
            outputs[10..].iter().sum::<f64>() / 10.0;
        assert!(
            second_half_avg < first_half_avg,
            "output should decrease as measurement converges: first={first_half_avg}, second={second_half_avg}"
        );
    }
}
