//! Circuit breaker pattern implementation for resilient connection handling.
//!
//! The circuit breaker prevents cascade failures by fast-failing requests when
//! the system is under stress (e.g., database connection pool exhausted).
//!
//! States:
//! - Closed: Normal operation, requests flow through
//! - Open: Failing fast, requests rejected immediately
//! - HalfOpen: Testing if system recovered, allowing one probe request

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Normal operation - requests flow through
    Closed,
    /// Circuit tripped - requests fail immediately
    Open,
    /// Testing recovery - one request allowed through
    HalfOpen,
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            State::Closed => write!(f, "closed"),
            State::Open => write!(f, "open"),
            State::HalfOpen => write!(f, "half_open"),
        }
    }
}

/// Configuration for the circuit breaker
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of failures before circuit opens
    pub failure_threshold: u32,
    /// Time window for counting failures
    pub failure_window: Duration,
    /// How long to wait before trying half-open
    pub recovery_timeout: Duration,
    /// Number of successes in half-open to close circuit
    pub success_threshold: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            failure_window: Duration::from_secs(10),
            recovery_timeout: Duration::from_secs(30),
            success_threshold: 2,
        }
    }
}

/// Thread-safe circuit breaker implementation
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    /// Current state
    state: RwLock<State>,
    /// Failure count in current window
    failure_count: AtomicU32,
    /// Success count in half-open state
    half_open_successes: AtomicU32,
    /// Timestamp when failure window started (as epoch millis)
    window_start: AtomicU64,
    /// Timestamp when circuit opened (as epoch millis)
    opened_at: AtomicU64,
    /// Reference instant for time calculations
    epoch: Instant,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the given configuration
    pub fn new(config: CircuitBreakerConfig) -> Self {
        let epoch = Instant::now();
        Self {
            config,
            state: RwLock::new(State::Closed),
            failure_count: AtomicU32::new(0),
            half_open_successes: AtomicU32::new(0),
            window_start: AtomicU64::new(0),
            opened_at: AtomicU64::new(0),
            epoch,
        }
    }

    /// Get current time as milliseconds since epoch
    fn now_millis(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// Get the current state of the circuit breaker
    pub async fn state(&self) -> State {
        *self.state.read().await
    }

    /// Check if a request should be allowed through.
    /// Returns Ok(()) if allowed, Err with state if rejected.
    pub async fn check(&self) -> Result<(), State> {
        let mut state = self.state.write().await;
        let now = self.now_millis();

        match *state {
            State::Closed => {
                // Check if failure window expired, reset counter if so
                let window_start = self.window_start.load(Ordering::Relaxed);
                let window_ms = self.config.failure_window.as_millis() as u64;

                if now.saturating_sub(window_start) > window_ms {
                    self.failure_count.store(0, Ordering::Relaxed);
                    self.window_start.store(now, Ordering::Relaxed);
                }
                Ok(())
            }
            State::Open => {
                // Check if recovery timeout has passed
                let opened_at = self.opened_at.load(Ordering::Relaxed);
                let recovery_ms = self.config.recovery_timeout.as_millis() as u64;

                if now.saturating_sub(opened_at) >= recovery_ms {
                    // Transition to half-open
                    log::info!("Circuit breaker transitioning from Open to HalfOpen");
                    *state = State::HalfOpen;
                    self.half_open_successes.store(0, Ordering::Relaxed);
                    crate::metrics::record_circuit_breaker_state("half_open");
                    Ok(())
                } else {
                    Err(State::Open)
                }
            }
            State::HalfOpen => {
                // Allow the request through for probing
                Ok(())
            }
        }
    }

    /// Record a successful operation
    pub async fn record_success(&self) {
        let mut state = self.state.write().await;

        match *state {
            State::HalfOpen => {
                let successes = self.half_open_successes.fetch_add(1, Ordering::Relaxed) + 1;
                if successes >= self.config.success_threshold {
                    log::info!(
                        "Circuit breaker closing after {} successful probes",
                        successes
                    );
                    *state = State::Closed;
                    self.failure_count.store(0, Ordering::Relaxed);
                    self.window_start
                        .store(self.now_millis(), Ordering::Relaxed);
                    crate::metrics::record_circuit_breaker_state("closed");
                }
            }
            State::Closed => {
                // Success in closed state, nothing to do
            }
            State::Open => {
                // Shouldn't happen, but ignore
            }
        }
    }

    /// Record a failed operation
    pub async fn record_failure(&self) {
        let mut state = self.state.write().await;
        let now = self.now_millis();

        match *state {
            State::Closed => {
                // Check if we need to reset the window
                let window_start = self.window_start.load(Ordering::Relaxed);
                let window_ms = self.config.failure_window.as_millis() as u64;

                if now.saturating_sub(window_start) > window_ms {
                    // Window expired, start fresh
                    self.failure_count.store(1, Ordering::Relaxed);
                    self.window_start.store(now, Ordering::Relaxed);
                } else {
                    let failures = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
                    if failures >= self.config.failure_threshold {
                        log::warn!(
                            "Circuit breaker opening after {} failures in {}ms window",
                            failures,
                            window_ms
                        );
                        *state = State::Open;
                        self.opened_at.store(now, Ordering::Relaxed);
                        crate::metrics::record_circuit_breaker_state("open");
                    }
                }
            }
            State::HalfOpen => {
                // Probe failed, back to open
                log::warn!("Circuit breaker probe failed, returning to Open state");
                *state = State::Open;
                self.opened_at.store(now, Ordering::Relaxed);
                crate::metrics::record_circuit_breaker_state("open");
            }
            State::Open => {
                // Already open, nothing to do
            }
        }
    }

    /// Get current failure count (for monitoring)
    pub fn failure_count(&self) -> u32 {
        self.failure_count.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_initial_state_is_closed() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig::default());
        assert_eq!(cb.state().await, State::Closed);
    }

    #[tokio::test]
    async fn test_allows_requests_when_closed() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig::default());
        assert!(cb.check().await.is_ok());
    }

    #[tokio::test]
    async fn test_opens_after_threshold_failures() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            failure_window: Duration::from_secs(60),
            ..Default::default()
        };
        let cb = CircuitBreaker::new(config);

        // Record failures below threshold
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, State::Closed);

        // Third failure should trip the breaker
        cb.record_failure().await;
        assert_eq!(cb.state().await, State::Open);
    }

    #[tokio::test]
    async fn test_rejects_when_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_secs(60),
            ..Default::default()
        };
        let cb = CircuitBreaker::new(config);

        cb.record_failure().await;
        assert_eq!(cb.state().await, State::Open);

        let result = cb.check().await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), State::Open);
    }

    #[tokio::test]
    async fn test_transitions_to_half_open_after_timeout() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_millis(10),
            ..Default::default()
        };
        let cb = CircuitBreaker::new(config);

        cb.record_failure().await;
        assert_eq!(cb.state().await, State::Open);

        // Wait for recovery timeout
        tokio::time::sleep(Duration::from_millis(15)).await;

        // Check should transition to half-open and allow request
        assert!(cb.check().await.is_ok());
        assert_eq!(cb.state().await, State::HalfOpen);
    }

    #[tokio::test]
    async fn test_closes_after_successful_probes() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_millis(1),
            success_threshold: 2,
            ..Default::default()
        };
        let cb = CircuitBreaker::new(config);

        // Trip the breaker
        cb.record_failure().await;
        tokio::time::sleep(Duration::from_millis(5)).await;

        // Transition to half-open
        cb.check().await.ok();
        assert_eq!(cb.state().await, State::HalfOpen);

        // First success
        cb.record_success().await;
        assert_eq!(cb.state().await, State::HalfOpen);

        // Second success should close
        cb.record_success().await;
        assert_eq!(cb.state().await, State::Closed);
    }

    #[tokio::test]
    async fn test_returns_to_open_on_half_open_failure() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_millis(1),
            ..Default::default()
        };
        let cb = CircuitBreaker::new(config);

        // Trip and transition to half-open
        cb.record_failure().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        cb.check().await.ok();
        assert_eq!(cb.state().await, State::HalfOpen);

        // Failure in half-open returns to open
        cb.record_failure().await;
        assert_eq!(cb.state().await, State::Open);
    }

    #[tokio::test]
    async fn test_failure_window_resets() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            failure_window: Duration::from_millis(10),
            ..Default::default()
        };
        let cb = CircuitBreaker::new(config);

        // Two failures
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.failure_count(), 2);

        // Wait for window to expire
        tokio::time::sleep(Duration::from_millis(15)).await;

        // Next failure should reset the counter
        cb.record_failure().await;
        assert_eq!(cb.failure_count(), 1);
        assert_eq!(cb.state().await, State::Closed);
    }
}
