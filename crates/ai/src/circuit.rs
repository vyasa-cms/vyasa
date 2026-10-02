//! Minimal consecutive-failure circuit breaker.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct State {
    failures: u32,
    opened_at: Option<Instant>,
}

/// Opens after `threshold` consecutive failures; probes again after
/// `cooldown` elapses.
///
/// Cloning shares the state: every clone sees the same failure count and
/// the same open/closed position. The registry hands each request a clone
/// of one long-lived breaker per model, which is the only way a breaker
/// can ever open — built fresh per request, as it was, the count reset to
/// zero on every call and three failures in a row never happened.
#[derive(Clone)]
pub struct CircuitBreaker {
    threshold: u32,
    cooldown: Duration,
    state: Arc<Mutex<State>>,
}

impl CircuitBreaker {
    /// New closed breaker.
    #[must_use]
    pub fn new(threshold: u32, cooldown: Duration) -> Self {
        Self {
            threshold,
            cooldown,
            state: Arc::new(Mutex::new(State {
                failures: 0,
                opened_at: None,
            })),
        }
    }

    /// Whether a call may proceed right now.
    #[must_use]
    pub fn allows(&self) -> bool {
        let st = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match st.opened_at {
            None => true,
            Some(at) => at.elapsed() >= self.cooldown,
        }
    }

    /// Records a success: resets the failure count and closes the breaker.
    pub fn record_success(&self) {
        let mut st = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        st.failures = 0;
        st.opened_at = None;
    }

    /// Records a failure; opens the breaker once `threshold` consecutive
    /// failures accumulate.
    pub fn record_failure(&self) {
        let mut st = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        st.failures += 1;
        if st.failures >= self.threshold && st.opened_at.is_none() {
            st.opened_at = Some(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_after_threshold_and_cools_down() {
        let breaker = CircuitBreaker::new(3, Duration::from_millis(20));
        assert!(breaker.allows());
        breaker.record_failure();
        breaker.record_failure();
        assert!(breaker.allows(), "below threshold still closed");
        breaker.record_failure();
        assert!(!breaker.allows(), "open after 3 consecutive failures");
        std::thread::sleep(Duration::from_millis(25));
        assert!(breaker.allows(), "cooldown elapsed -> probe allowed");
    }

    #[test]
    fn success_resets() {
        let breaker = CircuitBreaker::new(2, Duration::from_secs(60));
        breaker.record_failure();
        breaker.record_success();
        breaker.record_failure();
        assert!(breaker.allows(), "success resets the consecutive count");
    }
}

#[cfg(test)]
mod shared_state_tests {
    use super::*;

    #[test]
    fn a_clone_is_the_same_breaker() {
        let a = CircuitBreaker::new(2, Duration::from_secs(60));
        let b = a.clone();
        a.record_failure();
        b.record_failure();
        assert!(
            !a.allows() && !b.allows(),
            "two failures across clones open it"
        );
        a.record_success();
        assert!(b.allows(), "and a success on one closes it for the other");
    }
}
