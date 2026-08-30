use std::time::Duration;

use crate::{CircuitState, Observer, ResilienceEvent};

/// `tracing` translation for the bounded core [`ResilienceEvent`] vocabulary.
///
/// The adapter emits structured events under the `softwheel_resilience` target. It records only
/// fields already present in the core event contract: stable route IDs, bounded enum labels,
/// numeric ordinals/generations, and durations. Request bodies, headers, raw errors, request IDs,
/// trace IDs, and other arbitrary request-derived strings are never accepted by this adapter.
///
/// This observer is deliberately translation-only: observing an event cannot alter retry,
/// failover, deadline, breaker, bulkhead, routing, or shadow semantics.
///
/// # Example
///
/// ```
/// # #[cfg(feature = "tracing")]
/// # {
/// use softwheel_resilience::{Observer, ResilienceEvent, TrafficRole, TracingObserver};
///
/// let observer = TracingObserver;
/// observer.observe(&ResilienceEvent::LogicalDeadlineExhausted {
///     role: TrafficRole::Primary,
///     stage: softwheel_resilience::BudgetStage::BeforeAttempt,
/// });
/// # }
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct TracingObserver;

impl Observer for TracingObserver {
    fn observe(&self, event: &ResilienceEvent) {
        match event {
            ResilienceEvent::AttemptAdmitted {
                role,
                route_id,
                attempt_ordinal,
            } => tracing::info!(
                target: "softwheel_resilience",
                event = "attempt_admitted",
                role = role.as_str(),
                route_id = route_id.as_str(),
                attempt_ordinal = *attempt_ordinal,
            ),
            ResilienceEvent::AttemptRejected {
                role,
                route_id,
                reason,
            } => tracing::info!(
                target: "softwheel_resilience",
                event = "attempt_rejected",
                role = role.as_str(),
                route_id = route_id.as_ref().map_or("", |id| id.as_str()),
                reason = reason.as_str(),
            ),
            ResilienceEvent::AttemptCompleted {
                role,
                route_id,
                attempt_ordinal,
                outcome,
                latency,
            } => tracing::info!(
                target: "softwheel_resilience",
                event = "attempt_completed",
                role = role.as_str(),
                route_id = route_id.as_str(),
                attempt_ordinal = *attempt_ordinal,
                outcome = outcome.as_str(),
                latency_us = duration_micros(*latency),
            ),
            ResilienceEvent::RetryScheduled {
                role,
                route_id,
                retry_ordinal,
                backoff,
            } => tracing::info!(
                target: "softwheel_resilience",
                event = "retry_scheduled",
                role = role.as_str(),
                route_id = route_id.as_str(),
                retry_ordinal = *retry_ordinal,
                backoff_us = duration_micros(*backoff),
            ),
            ResilienceEvent::RetrySuppressed {
                role,
                route_id,
                retry_ordinal,
                reason,
            } => tracing::info!(
                target: "softwheel_resilience",
                event = "retry_suppressed",
                role = role.as_str(),
                route_id = route_id.as_str(),
                retry_ordinal = *retry_ordinal,
                reason = reason.as_str(),
            ),
            ResilienceEvent::BreakerTransition {
                role,
                route_id,
                from,
                to,
            } => tracing::info!(
                target: "softwheel_resilience",
                event = "breaker_transition",
                role = role.as_str(),
                route_id = route_id.as_str(),
                from = circuit_state(*from),
                to = circuit_state(*to),
            ),
            ResilienceEvent::BulkheadAdmission {
                role,
                route_id,
                admitted,
            } => tracing::info!(
                target: "softwheel_resilience",
                event = "bulkhead_admission",
                role = role.as_str(),
                route_id = route_id.as_str(),
                admitted = *admitted,
            ),
            ResilienceEvent::RouteSelected {
                role,
                generation,
                route_id,
            } => tracing::info!(
                target: "softwheel_resilience",
                event = "route_selected",
                role = role.as_str(),
                generation = *generation,
                route_id = route_id.as_str(),
            ),
            ResilienceEvent::RouteFailover {
                generation,
                failover_ordinal,
                from_route_id,
                to_route_id,
                outcome,
            } => tracing::info!(
                target: "softwheel_resilience",
                event = "route_failover",
                generation = *generation,
                failover_ordinal = *failover_ordinal,
                from_route_id = from_route_id.as_str(),
                to_route_id = to_route_id.as_ref().map_or("", |id| id.as_str()),
                outcome = outcome.as_str(),
            ),
            ResilienceEvent::ShadowSampled {
                generation,
                outcome,
            } => tracing::info!(
                target: "softwheel_resilience",
                event = "shadow_sampled",
                generation = *generation,
                outcome = outcome.as_str(),
            ),
            ResilienceEvent::ShadowCompleted {
                generation,
                route_id,
                outcome,
            } => tracing::info!(
                target: "softwheel_resilience",
                event = "shadow_completed",
                generation = *generation,
                route_id = route_id.as_str(),
                outcome = outcome.as_str(),
            ),
            ResilienceEvent::LogicalDeadlineExhausted { role, stage } => tracing::info!(
                target: "softwheel_resilience",
                event = "logical_deadline_exhausted",
                role = role.as_str(),
                stage = stage.as_str(),
            ),
        }
    }
}

const fn circuit_state(state: CircuitState) -> &'static str {
    match state {
        CircuitState::Closed => "closed",
        CircuitState::Open => "open",
        CircuitState::HalfOpen => "half_open",
    }
}

fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BudgetStage, RouteId, TrafficRole};

    #[test]
    fn duration_translation_saturates() {
        assert_eq!(duration_micros(Duration::MAX), u64::MAX);
    }

    #[test]
    fn tracing_observer_accepts_bounded_events_without_subscriber() {
        let observer = TracingObserver;
        observer.observe(&ResilienceEvent::RouteSelected {
            role: TrafficRole::Shadow,
            generation: 11,
            route_id: RouteId::new("shadow-a").unwrap(),
        });
        observer.observe(&ResilienceEvent::LogicalDeadlineExhausted {
            role: TrafficRole::Primary,
            stage: BudgetStage::BeforeBackoff,
        });
    }

    #[test]
    fn circuit_state_labels_are_bounded() {
        assert_eq!(circuit_state(CircuitState::Closed), "closed");
        assert_eq!(circuit_state(CircuitState::Open), "open");
        assert_eq!(circuit_state(CircuitState::HalfOpen), "half_open");
    }
}
