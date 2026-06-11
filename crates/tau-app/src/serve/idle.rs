//! Idle-timeout tracking for serve mode.
//!
//! [`Activity`] is a shared last-activity clock. The reader and writer
//! tasks `touch()` it on every inbound/outbound message, so an in-flight
//! run that keeps streaming output holds the timer open (see serve-mode
//! design spec §8.5: "timer resets on every incoming OR outgoing message").
//!
//! [`wait_for_idle`] resolves once no `touch()` has happened for the
//! configured duration; `lifecycle::run` selects on it to drive graceful
//! shutdown.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Shared last-activity clock. Cheap to clone (one `Arc`).
#[derive(Clone)]
pub struct Activity {
    last: Arc<Mutex<Instant>>,
}

impl Activity {
    /// Construct a clock whose last-activity instant is now.
    pub fn new() -> Self {
        Self {
            last: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// Record activity: reset the last-activity instant to now.
    pub fn touch(&self) {
        *self.last.lock().expect("activity mutex poisoned") = Instant::now();
    }

    /// The most recent activity instant.
    fn last(&self) -> Instant {
        *self.last.lock().expect("activity mutex poisoned")
    }
}

impl Default for Activity {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve once no activity has occurred on `activity` for `timeout`.
///
/// Each `touch()` pushes the deadline forward, so this only resolves
/// after a full `timeout` of silence. Never resolves while activity keeps
/// arriving faster than `timeout`.
pub async fn wait_for_idle(activity: Activity, timeout: Duration) {
    loop {
        let idle = activity.last().elapsed();
        if idle >= timeout {
            return;
        }
        // Sleep only for the remaining time; if a `touch()` landed since we
        // last checked, the next iteration recomputes a longer remainder.
        tokio::time::sleep(timeout - idle).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fires_after_timeout_with_no_activity() {
        let activity = Activity::new();
        let res = tokio::time::timeout(
            Duration::from_secs(2),
            wait_for_idle(activity, Duration::from_millis(50)),
        )
        .await;
        assert!(res.is_ok(), "wait_for_idle should resolve when idle");
    }

    #[tokio::test]
    async fn does_not_fire_while_activity_continues() {
        let activity = Activity::new();
        let toucher = activity.clone();
        // Touch every 10ms — a 10× margin under the 100ms timeout, so a CI
        // scheduler stall has to swallow ~90ms between two touches before this
        // could spuriously fire.
        let handle = tokio::spawn(async move {
            for _ in 0..30 {
                tokio::time::sleep(Duration::from_millis(10)).await;
                toucher.touch();
            }
        });

        let res = tokio::time::timeout(
            Duration::from_millis(200),
            wait_for_idle(activity, Duration::from_millis(100)),
        )
        .await;
        assert!(
            res.is_err(),
            "wait_for_idle must NOT resolve while activity continues"
        );
        handle.abort();
    }
}
