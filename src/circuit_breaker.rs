use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CircuitBreakerConfigError {
    ZeroFailureThreshold,
    ZeroOpenTimeout,
    ZeroHalfOpenSuccessThreshold,
}

impl fmt::Display for CircuitBreakerConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ZeroFailureThreshold => "failure threshold must be greater than zero",
            Self::ZeroOpenTimeout => "open timeout must be greater than zero",
            Self::ZeroHalfOpenSuccessThreshold => {
                "half-open success threshold must be greater than zero"
            }
        };
        f.write_str(message)
    }
}

impl std::error::Error for CircuitBreakerConfigError {}

#[derive(Clone, Debug)]
pub struct CircuitBreakerConfig {
    failure_threshold: u32,
    open_timeout: Duration,
    half_open_success_threshold: u32,
}

impl CircuitBreakerConfig {
    pub fn new(
        failure_threshold: u32,
        open_timeout: Duration,
        half_open_success_threshold: u32,
    ) -> Result<Self, CircuitBreakerConfigError> {
        if failure_threshold == 0 {
            return Err(CircuitBreakerConfigError::ZeroFailureThreshold);
        }
        if open_timeout.is_zero() {
            return Err(CircuitBreakerConfigError::ZeroOpenTimeout);
        }
        if half_open_success_threshold == 0 {
            return Err(CircuitBreakerConfigError::ZeroHalfOpenSuccessThreshold);
        }

        Ok(Self {
            failure_threshold,
            open_timeout,
            half_open_success_threshold,
        })
    }

    pub fn failure_threshold(&self) -> u32 {
        self.failure_threshold
    }

    pub fn open_timeout(&self) -> Duration {
        self.open_timeout
    }

    pub fn half_open_success_threshold(&self) -> u32 {
        self.half_open_success_threshold
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CircuitBreakerRejected;

impl fmt::Display for CircuitBreakerRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("circuit breaker rejected the call")
    }
}

impl std::error::Error for CircuitBreakerRejected {}

#[derive(Debug)]
pub enum CircuitBreakerCallError<E> {
    Rejected(CircuitBreakerRejected),
    Inner(E),
}

impl<E: fmt::Display> fmt::Display for CircuitBreakerCallError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(error) => error.fmt(f),
            Self::Inner(error) => error.fmt(f),
        }
    }
}

impl<E> std::error::Error for CircuitBreakerCallError<E> where E: std::error::Error + 'static {}

#[derive(Clone, Copy, Debug)]
enum PermitKind {
    Closed,
    HalfOpen,
}

#[derive(Debug)]
enum Mode {
    Closed { consecutive_failures: u32 },
    Open { until: Instant },
    HalfOpen { in_flight: u32, successes: u32 },
}

#[derive(Debug)]
struct Shared {
    generation: u64,
    mode: Mode,
}

impl Shared {
    fn transition_closed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.mode = Mode::Closed {
            consecutive_failures: 0,
        };
    }

    fn transition_open(&mut self, timeout: Duration) {
        self.generation = self.generation.wrapping_add(1);
        self.mode = Mode::Open {
            until: Instant::now() + timeout,
        };
    }

    fn transition_half_open(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.mode = Mode::HalfOpen {
            in_flight: 0,
            successes: 0,
        };
    }
}

#[derive(Debug)]
struct Inner {
    config: CircuitBreakerConfig,
    shared: Mutex<Shared>,
}

/// Thread-safe circuit breaker using Closed -> Open -> HalfOpen transitions.
///
/// Half-open calls are explicitly limited. Each successful probe contributes toward closing the
/// circuit; any failed probe immediately re-opens it. A generation number ensures outcomes from
/// stale in-flight calls cannot mutate a newer breaker state.
#[derive(Clone, Debug)]
pub struct CircuitBreaker {
    inner: Arc<Inner>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                shared: Mutex::new(Shared {
                    generation: 0,
                    mode: Mode::Closed {
                        consecutive_failures: 0,
                    },
                }),
            }),
        }
    }

    pub fn state(&self) -> CircuitState {
        let mut shared = self.lock();
        Self::refresh_open_state(&mut shared);
        match &shared.mode {
            Mode::Closed { .. } => CircuitState::Closed,
            Mode::Open { .. } => CircuitState::Open,
            Mode::HalfOpen { .. } => CircuitState::HalfOpen,
        }
    }

    /// Acquire permission for one downstream call.
    ///
    /// In HalfOpen, at most `half_open_success_threshold` probes may be in flight. This prevents
    /// a thundering herd when an open circuit starts probing recovery.
    pub fn try_acquire(&self) -> Result<CircuitBreakerPermit, CircuitBreakerRejected> {
        let mut shared = self.lock();
        Self::refresh_open_state(&mut shared);

        let kind = match &mut shared.mode {
            Mode::Closed { .. } => PermitKind::Closed,
            Mode::Open { .. } => return Err(CircuitBreakerRejected),
            Mode::HalfOpen { in_flight, .. } => {
                if *in_flight >= self.inner.config.half_open_success_threshold {
                    return Err(CircuitBreakerRejected);
                }
                *in_flight += 1;
                PermitKind::HalfOpen
            }
        };

        let generation = shared.generation;
        Ok(CircuitBreakerPermit {
            breaker: self.clone(),
            generation,
            kind,
            completed: false,
        })
    }

    pub fn call<T, E, Operation>(
        &self,
        operation: Operation,
    ) -> Result<T, CircuitBreakerCallError<E>>
    where
        Operation: FnOnce() -> Result<T, E>,
    {
        let permit = self
            .try_acquire()
            .map_err(CircuitBreakerCallError::Rejected)?;

        match operation() {
            Ok(value) => {
                permit.success();
                Ok(value)
            }
            Err(error) => {
                permit.failure();
                Err(CircuitBreakerCallError::Inner(error))
            }
        }
    }

    fn refresh_open_state(shared: &mut Shared) {
        let should_half_open =
            matches!(&shared.mode, Mode::Open { until } if Instant::now() >= *until);
        if should_half_open {
            shared.transition_half_open();
        }
    }

    fn record_success(&self, generation: u64, kind: PermitKind) {
        let mut shared = self.lock();
        if shared.generation != generation {
            return;
        }

        let mut should_close = false;
        match (&mut shared.mode, kind) {
            (
                Mode::Closed {
                    consecutive_failures,
                },
                PermitKind::Closed,
            ) => {
                *consecutive_failures = 0;
            }
            (
                Mode::HalfOpen {
                    in_flight,
                    successes,
                },
                PermitKind::HalfOpen,
            ) => {
                *in_flight = in_flight.saturating_sub(1);
                *successes = successes.saturating_add(1);
                should_close = *successes >= self.inner.config.half_open_success_threshold;
            }
            _ => {}
        }

        if should_close {
            shared.transition_closed();
        }
    }

    fn record_failure(&self, generation: u64, kind: PermitKind) {
        let mut shared = self.lock();
        if shared.generation != generation {
            return;
        }

        let mut should_open = false;
        match (&mut shared.mode, kind) {
            (
                Mode::Closed {
                    consecutive_failures,
                },
                PermitKind::Closed,
            ) => {
                *consecutive_failures = consecutive_failures.saturating_add(1);
                should_open = *consecutive_failures >= self.inner.config.failure_threshold;
            }
            (Mode::HalfOpen { in_flight, .. }, PermitKind::HalfOpen) => {
                *in_flight = in_flight.saturating_sub(1);
                should_open = true;
            }
            _ => {}
        }

        if should_open {
            shared.transition_open(self.inner.config.open_timeout);
        }
    }

    fn abandon(&self, generation: u64, kind: PermitKind) {
        if !matches!(kind, PermitKind::HalfOpen) {
            return;
        }

        let mut shared = self.lock();
        if shared.generation != generation {
            return;
        }

        if let Mode::HalfOpen { in_flight, .. } = &mut shared.mode {
            *in_flight = in_flight.saturating_sub(1);
        }
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.inner
            .shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug)]
pub struct CircuitBreakerPermit {
    breaker: CircuitBreaker,
    generation: u64,
    kind: PermitKind,
    completed: bool,
}

impl CircuitBreakerPermit {
    pub fn success(mut self) {
        self.breaker.record_success(self.generation, self.kind);
        self.completed = true;
    }

    pub fn failure(mut self) {
        self.breaker.record_failure(self.generation, self.kind);
        self.completed = true;
    }
}

impl Drop for CircuitBreakerPermit {
    fn drop(&mut self) {
        if !self.completed {
            self.breaker.abandon(self.generation, self.kind);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn breaker() -> CircuitBreaker {
        CircuitBreaker::new(CircuitBreakerConfig::new(2, Duration::from_millis(20), 1).unwrap())
    }

    #[test]
    fn opens_after_threshold_and_rejects_calls() {
        let breaker = breaker();
        breaker.try_acquire().unwrap().failure();
        assert_eq!(breaker.state(), CircuitState::Closed);
        breaker.try_acquire().unwrap().failure();
        assert_eq!(breaker.state(), CircuitState::Open);
        assert_eq!(breaker.try_acquire().unwrap_err(), CircuitBreakerRejected);
    }

    #[test]
    fn success_resets_consecutive_failures() {
        let breaker = breaker();
        breaker.try_acquire().unwrap().failure();
        breaker.try_acquire().unwrap().success();
        breaker.try_acquire().unwrap().failure();
        assert_eq!(breaker.state(), CircuitState::Closed);
    }

    #[test]
    fn half_open_success_closes_circuit() {
        let breaker =
            CircuitBreaker::new(CircuitBreakerConfig::new(1, Duration::from_millis(1), 1).unwrap());
        breaker.try_acquire().unwrap().failure();
        std::thread::sleep(Duration::from_millis(2));
        assert_eq!(breaker.state(), CircuitState::HalfOpen);
        breaker.try_acquire().unwrap().success();
        assert_eq!(breaker.state(), CircuitState::Closed);
    }
}
