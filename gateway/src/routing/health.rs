use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::config::CircuitBreakerConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

struct ProviderCircuit {
    state: CircuitState,
    consecutive_failures: u32,
    last_failure_at: Option<Instant>,
    half_open_successes: u32,
    failure_threshold: u32,
    cooldown: Duration,
    half_open_max: u32,
}

impl ProviderCircuit {
    fn new(config: &CircuitBreakerConfig) -> Self {
        Self {
            state: CircuitState::Closed,
            consecutive_failures: 0,
            last_failure_at: None,
            half_open_successes: 0,
            failure_threshold: config.failure_threshold,
            cooldown: Duration::from_secs(config.cooldown_seconds),
            half_open_max: config.half_open_max_requests,
        }
    }

    fn is_available(&mut self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if let Some(last) = self.last_failure_at {
                    if last.elapsed() >= self.cooldown {
                        self.state = CircuitState::HalfOpen;
                        self.half_open_successes = 0;
                        tracing::info!("circuit breaker transitioning to half-open");
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    fn record_success(&mut self) {
        match self.state {
            CircuitState::Closed => {
                self.consecutive_failures = 0;
            }
            CircuitState::HalfOpen => {
                self.half_open_successes += 1;
                if self.half_open_successes >= self.half_open_max {
                    self.state = CircuitState::Closed;
                    self.consecutive_failures = 0;
                    tracing::info!("circuit breaker closed after successful probes");
                }
            }
            CircuitState::Open => {}
        }
    }

    fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        self.last_failure_at = Some(Instant::now());

        if self.state == CircuitState::HalfOpen {
            self.state = CircuitState::Open;
            tracing::warn!("circuit breaker re-opened from half-open after failure");
        } else if self.consecutive_failures >= self.failure_threshold {
            self.state = CircuitState::Open;
            tracing::warn!(
                failures = self.consecutive_failures,
                "circuit breaker opened"
            );
        }
    }
}

/// Thread-safe circuit breaker registry for all providers.
#[derive(Clone)]
pub struct HealthTracker {
    circuits: Arc<RwLock<HashMap<String, ProviderCircuit>>>,
    config: CircuitBreakerConfig,
}

impl HealthTracker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            circuits: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    fn ensure_circuit(&self, provider: &str) {
        let read = self.circuits.read().unwrap();
        if read.contains_key(provider) {
            return;
        }
        drop(read);
        let mut write = self.circuits.write().unwrap();
        write
            .entry(provider.to_string())
            .or_insert_with(|| ProviderCircuit::new(&self.config));
    }

    pub fn is_available(&self, provider: &str) -> bool {
        self.ensure_circuit(provider);

        // Fast path: read lock only — Closed and HalfOpen don't need mutation
        {
            let circuits = self.circuits.read().unwrap();
            if let Some(c) = circuits.get(provider) {
                match c.state {
                    CircuitState::Closed | CircuitState::HalfOpen => return true,
                    CircuitState::Open => {
                        // Check if cooldown has NOT elapsed — still unavailable
                        if let Some(last) = c.last_failure_at {
                            if last.elapsed() < c.cooldown {
                                return false;
                            }
                        } else {
                            return false;
                        }
                        // Cooldown elapsed — fall through to write lock for state transition
                    }
                }
            }
        }

        // Slow path: write lock to transition Open → HalfOpen
        let mut circuits = self.circuits.write().unwrap();
        circuits
            .get_mut(provider)
            .map(|c| c.is_available())
            .unwrap_or(false)
    }

    pub fn record_success(&self, provider: &str) {
        self.ensure_circuit(provider);
        let mut circuits = self.circuits.write().unwrap();
        if let Some(c) = circuits.get_mut(provider) {
            c.record_success();
        }
    }

    pub fn record_failure(&self, provider: &str) {
        self.ensure_circuit(provider);
        let mut circuits = self.circuits.write().unwrap();
        if let Some(c) = circuits.get_mut(provider) {
            c.record_failure();
        }
    }

    #[allow(dead_code)]
    pub fn state(&self, provider: &str) -> CircuitState {
        let read = self.circuits.read().unwrap();
        read.get(provider)
            .map(|c| c.state)
            .unwrap_or(CircuitState::Closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CircuitBreakerConfig;

    fn config() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: 3,
            cooldown_seconds: 1,
            half_open_max_requests: 2,
        }
    }

    #[test]
    fn test_new_provider_is_available() {
        let tracker = HealthTracker::new(config());
        assert!(tracker.is_available("new-provider"));
        assert_eq!(tracker.state("new-provider"), CircuitState::Closed);
    }

    #[test]
    fn test_opens_after_threshold() {
        let tracker = HealthTracker::new(config());
        tracker.record_failure("p");
        tracker.record_failure("p");
        assert_eq!(tracker.state("p"), CircuitState::Closed); // 2 < 3
        tracker.record_failure("p");
        assert_eq!(tracker.state("p"), CircuitState::Open); // 3 >= 3
        assert!(!tracker.is_available("p"));
    }

    #[test]
    fn test_success_resets_failures() {
        let tracker = HealthTracker::new(config());
        tracker.record_failure("p");
        tracker.record_failure("p");
        tracker.record_success("p");
        tracker.record_failure("p"); // only 1 consecutive now
        assert_eq!(tracker.state("p"), CircuitState::Closed);
    }

    #[test]
    fn test_half_open_after_cooldown() {
        let tracker = HealthTracker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            cooldown_seconds: 0, // instant cooldown
            half_open_max_requests: 2,
        });
        tracker.record_failure("p");
        assert_eq!(tracker.state("p"), CircuitState::Open);

        // is_available triggers transition to HalfOpen after cooldown
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(tracker.is_available("p"));
        assert_eq!(tracker.state("p"), CircuitState::HalfOpen);
    }

    #[test]
    fn test_half_open_closes_after_successes() {
        let tracker = HealthTracker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            cooldown_seconds: 0,
            half_open_max_requests: 2,
        });
        tracker.record_failure("p");
        std::thread::sleep(std::time::Duration::from_millis(10));
        tracker.is_available("p"); // → HalfOpen

        tracker.record_success("p");
        assert_eq!(tracker.state("p"), CircuitState::HalfOpen);
        tracker.record_success("p");
        assert_eq!(tracker.state("p"), CircuitState::Closed); // 2 successes
    }

    #[test]
    fn test_half_open_reopens_on_failure() {
        let tracker = HealthTracker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            cooldown_seconds: 0,
            half_open_max_requests: 3,
        });
        tracker.record_failure("p");
        std::thread::sleep(std::time::Duration::from_millis(10));
        tracker.is_available("p"); // → HalfOpen

        tracker.record_failure("p"); // fail in half-open → re-open
        assert_eq!(tracker.state("p"), CircuitState::Open);
    }

    #[test]
    fn test_independent_providers() {
        let tracker = HealthTracker::new(config());
        tracker.record_failure("a");
        tracker.record_failure("a");
        tracker.record_failure("a"); // a → Open
        assert!(!tracker.is_available("a"));
        assert!(tracker.is_available("b")); // b unaffected
    }
}
