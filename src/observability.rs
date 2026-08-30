use std::time::Duration;

use crate::{CircuitState, RouteId};

/// Whether an observation belongs to primary or isolated shadow execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrafficRole {
    Primary,
    Shadow,
}

impl TrafficRole {
    /// A stable, bounded label value suitable for metrics adapters.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Shadow => "shadow",
        }
    }
}

/// Bounded reasons for policy/admission rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RejectionReason {
    CircuitOpen,
    BulkheadFull,
    RetryBudgetExhausted,
    LogicalDeadlineExhausted,
    RouteAttemptBudgetExhausted,
    NoEligibleRoute,
}

impl RejectionReason {
    /// A stable, bounded label value suitable for metrics adapters.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CircuitOpen => "circuit_open",
            Self::BulkheadFull => "bulkhead_full",
            Self::RetryBudgetExhausted => "retry_budget_exhausted",
            Self::LogicalDeadlineExhausted => "logical_deadline_exhausted",
            Self::RouteAttemptBudgetExhausted => "route_attempt_budget_exhausted",
            Self::NoEligibleRoute => "no_eligible_route",
        }
    }
}

/// Why a retry did not proceed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetrySuppressionReason {
    RetryPolicyExhausted,
    RetryBudgetExhausted,
    LogicalDeadlineExhausted,
}

impl RetrySuppressionReason {
    /// A stable, bounded label value suitable for metrics adapters.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RetryPolicyExhausted => "retry_policy_exhausted",
            Self::RetryBudgetExhausted => "retry_budget_exhausted",
            Self::LogicalDeadlineExhausted => "logical_deadline_exhausted",
        }
    }
}

/// Stable outcome classes. Raw errors are deliberately excluded from built-in labels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutcomeClass {
    Succeeded,
    Failed,
    Cancelled,
    DeadlineExceeded,
    Overloaded,
}

impl OutcomeClass {
    /// A stable, bounded label value suitable for metrics adapters.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::Overloaded => "overloaded",
        }
    }
}

/// Where logical-request budget exhaustion was observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetStage {
    BeforeAttempt,
    BeforeBackoff,
    BeforeFailover,
    BeforeShadow,
}

impl BudgetStage {
    /// A stable, bounded label value suitable for metrics adapters.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BeforeAttempt => "before_attempt",
            Self::BeforeBackoff => "before_backoff",
            Self::BeforeFailover => "before_failover",
            Self::BeforeShadow => "before_shadow",
        }
    }
}

/// Whether shadow execution was selected for a logical request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShadowSamplingOutcome {
    Sampled,
    NotSampled,
}

impl ShadowSamplingOutcome {
    /// A stable, bounded label value suitable for metrics adapters.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sampled => "sampled",
            Self::NotSampled => "not_sampled",
        }
    }
}

/// Zero-dependency core observation vocabulary.
///
/// The event payload intentionally accepts only stable route identity, numeric ordinals,
/// durations, generations, and closed enums. Request IDs, URLs, raw errors, headers, user IDs,
/// and other request-derived strings have no field in this API, preventing built-in adapters
/// from accidentally turning unbounded data into metric labels.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResilienceEvent {
    AttemptAdmitted {
        role: TrafficRole,
        route_id: RouteId,
        attempt_ordinal: u32,
    },
    AttemptRejected {
        role: TrafficRole,
        route_id: Option<RouteId>,
        reason: RejectionReason,
    },
    AttemptCompleted {
        role: TrafficRole,
        route_id: RouteId,
        attempt_ordinal: u32,
        outcome: OutcomeClass,
        latency: Duration,
    },
    RetryScheduled {
        role: TrafficRole,
        route_id: RouteId,
        retry_ordinal: u32,
        backoff: Duration,
    },
    RetrySuppressed {
        role: TrafficRole,
        route_id: RouteId,
        retry_ordinal: u32,
        reason: RetrySuppressionReason,
    },
    BreakerTransition {
        role: TrafficRole,
        route_id: RouteId,
        from: CircuitState,
        to: CircuitState,
    },
    BulkheadAdmission {
        role: TrafficRole,
        route_id: RouteId,
        admitted: bool,
    },
    RouteSelected {
        role: TrafficRole,
        generation: u64,
        route_id: RouteId,
    },
    RouteFailover {
        generation: u64,
        failover_ordinal: u32,
        from_route_id: RouteId,
        to_route_id: Option<RouteId>,
        outcome: OutcomeClass,
    },
    ShadowSampled {
        generation: u64,
        outcome: ShadowSamplingOutcome,
    },
    ShadowCompleted {
        generation: u64,
        route_id: RouteId,
        outcome: OutcomeClass,
    },
    LogicalDeadlineExhausted {
        role: TrafficRole,
        stage: BudgetStage,
    },
}

/// Sink implemented by adapters that translate core observations to metrics, traces, or logs.
///
/// Observation is synchronous and must not control execution. Implementations should keep this
/// callback non-blocking; adapters that need asynchronous export should enqueue or aggregate
/// outside the core policy path.
pub trait Observer {
    fn observe(&self, event: &ResilienceEvent);
}

/// Observer that intentionally discards every event.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopObserver;

impl Observer for NoopObserver {
    fn observe(&self, _event: &ResilienceEvent) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingObserver {
        events: Mutex<Vec<ResilienceEvent>>,
    }

    impl Observer for RecordingObserver {
        fn observe(&self, event: &ResilienceEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
    }

    #[test]
    fn observer_receives_owned_bounded_event_payload() {
        let observer = RecordingObserver::default();
        let event = ResilienceEvent::RouteSelected {
            role: TrafficRole::Primary,
            generation: 7,
            route_id: RouteId::new("route-a").unwrap(),
        };

        observer.observe(&event);

        assert_eq!(observer.events.lock().unwrap().as_slice(), &[event]);
    }

    #[test]
    fn bounded_dimensions_have_static_label_values() {
        assert_eq!(TrafficRole::Shadow.as_str(), "shadow");
        assert_eq!(RejectionReason::CircuitOpen.as_str(), "circuit_open");
        assert_eq!(
            RetrySuppressionReason::RetryBudgetExhausted.as_str(),
            "retry_budget_exhausted"
        );
        assert_eq!(OutcomeClass::DeadlineExceeded.as_str(), "deadline_exceeded");
        assert_eq!(BudgetStage::BeforeFailover.as_str(), "before_failover");
        assert_eq!(ShadowSamplingOutcome::NotSampled.as_str(), "not_sampled");
    }

    #[test]
    fn noop_observer_accepts_events_without_side_effects() {
        let observer = NoopObserver;
        observer.observe(&ResilienceEvent::LogicalDeadlineExhausted {
            role: TrafficRole::Shadow,
            stage: BudgetStage::BeforeShadow,
        });
    }
}
