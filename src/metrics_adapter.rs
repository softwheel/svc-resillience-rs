use crate::{CircuitState, Observer, ResilienceEvent};

/// `metrics` facade translation for the bounded core [`ResilienceEvent`] vocabulary.
///
/// The adapter emits counters and histograms without installing or selecting a recorder/exporter.
/// Labels are limited to stable route identity and closed core enums. Numeric generations,
/// attempt/retry/failover ordinals, request IDs, URLs, headers, raw errors, trace IDs, customer
/// IDs, and other request-derived values are deliberately not used as metric labels.
///
/// Timing observations use seconds as `f64`, matching the conventional unit expected by most
/// metrics backends. Observation remains translation-only and cannot alter retries, failover,
/// deadlines, routing, breaker/bulkhead state, or shadow isolation.
///
/// # Example
///
/// ```
/// # #[cfg(feature = "metrics")]
/// # {
/// use softwheel_resilience::{MetricsObserver, Observer, ResilienceEvent, TrafficRole};
///
/// let observer = MetricsObserver;
/// observer.observe(&ResilienceEvent::LogicalDeadlineExhausted {
///     role: TrafficRole::Primary,
///     stage: softwheel_resilience::BudgetStage::BeforeAttempt,
/// });
/// # }
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct MetricsObserver;

impl Observer for MetricsObserver {
    fn observe(&self, event: &ResilienceEvent) {
        match event {
            ResilienceEvent::AttemptAdmitted { role, route_id, .. } => {
                metrics::counter!(
                    "softwheel_resilience_attempt_admitted_total",
                    "role" => role.as_str(),
                    "route_id" => route_id.as_str().to_owned(),
                )
                .increment(1);
            }
            ResilienceEvent::AttemptRejected {
                role,
                route_id,
                reason,
            } => {
                metrics::counter!(
                    "softwheel_resilience_attempt_rejected_total",
                    "role" => role.as_str(),
                    "route_id" => route_label(route_id.as_ref()),
                    "reason" => reason.as_str(),
                )
                .increment(1);
            }
            ResilienceEvent::AttemptCompleted {
                role,
                route_id,
                outcome,
                latency,
                ..
            } => {
                metrics::counter!(
                    "softwheel_resilience_attempt_completed_total",
                    "role" => role.as_str(),
                    "route_id" => route_id.as_str().to_owned(),
                    "outcome" => outcome.as_str(),
                )
                .increment(1);
                metrics::histogram!(
                    "softwheel_resilience_attempt_latency_seconds",
                    "role" => role.as_str(),
                    "route_id" => route_id.as_str().to_owned(),
                    "outcome" => outcome.as_str(),
                )
                .record(latency.as_secs_f64());
            }
            ResilienceEvent::RetryScheduled {
                role,
                route_id,
                backoff,
                ..
            } => {
                metrics::counter!(
                    "softwheel_resilience_retry_scheduled_total",
                    "role" => role.as_str(),
                    "route_id" => route_id.as_str().to_owned(),
                )
                .increment(1);
                metrics::histogram!(
                    "softwheel_resilience_retry_backoff_seconds",
                    "role" => role.as_str(),
                    "route_id" => route_id.as_str().to_owned(),
                )
                .record(backoff.as_secs_f64());
            }
            ResilienceEvent::RetrySuppressed {
                role,
                route_id,
                reason,
                ..
            } => {
                metrics::counter!(
                    "softwheel_resilience_retry_suppressed_total",
                    "role" => role.as_str(),
                    "route_id" => route_id.as_str().to_owned(),
                    "reason" => reason.as_str(),
                )
                .increment(1);
            }
            ResilienceEvent::BreakerTransition {
                role,
                route_id,
                from,
                to,
            } => {
                metrics::counter!(
                    "softwheel_resilience_breaker_transition_total",
                    "role" => role.as_str(),
                    "route_id" => route_id.as_str().to_owned(),
                    "from" => circuit_state(*from),
                    "to" => circuit_state(*to),
                )
                .increment(1);
            }
            ResilienceEvent::BulkheadAdmission {
                role,
                route_id,
                admitted,
            } => {
                metrics::counter!(
                    "softwheel_resilience_bulkhead_admission_total",
                    "role" => role.as_str(),
                    "route_id" => route_id.as_str().to_owned(),
                    "admitted" => bool_label(*admitted),
                )
                .increment(1);
            }
            ResilienceEvent::RouteSelected { role, route_id, .. } => {
                metrics::counter!(
                    "softwheel_resilience_route_selected_total",
                    "role" => role.as_str(),
                    "route_id" => route_id.as_str().to_owned(),
                )
                .increment(1);
            }
            ResilienceEvent::RouteFailover {
                from_route_id,
                to_route_id,
                outcome,
                ..
            } => {
                metrics::counter!(
                    "softwheel_resilience_route_failover_total",
                    "from_route_id" => from_route_id.as_str().to_owned(),
                    "to_route_id" => route_label(to_route_id.as_ref()),
                    "outcome" => outcome.as_str(),
                )
                .increment(1);
            }
            ResilienceEvent::ShadowSampled { outcome, .. } => {
                metrics::counter!(
                    "softwheel_resilience_shadow_sampling_total",
                    "outcome" => outcome.as_str(),
                )
                .increment(1);
            }
            ResilienceEvent::ShadowCompleted {
                route_id, outcome, ..
            } => {
                metrics::counter!(
                    "softwheel_resilience_shadow_completed_total",
                    "route_id" => route_id.as_str().to_owned(),
                    "outcome" => outcome.as_str(),
                )
                .increment(1);
            }
            ResilienceEvent::LogicalDeadlineExhausted { role, stage } => {
                metrics::counter!(
                    "softwheel_resilience_logical_deadline_exhausted_total",
                    "role" => role.as_str(),
                    "stage" => stage.as_str(),
                )
                .increment(1);
            }
        }
    }
}

fn route_label(route_id: Option<&crate::RouteId>) -> String {
    route_id.map_or_else(|| "none".to_owned(), |id| id.as_str().to_owned())
}

const fn circuit_state(state: CircuitState) -> &'static str {
    match state {
        CircuitState::Closed => "closed",
        CircuitState::Open => "open",
        CircuitState::HalfOpen => "half_open",
    }
}

const fn bool_label(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BudgetStage, RouteId, TrafficRole};

    #[test]
    fn metrics_observer_accepts_bounded_events_without_recorder() {
        let observer = MetricsObserver;
        observer.observe(&ResilienceEvent::RouteSelected {
            role: TrafficRole::Primary,
            generation: 17,
            route_id: RouteId::new("primary-a").unwrap(),
        });
        observer.observe(&ResilienceEvent::LogicalDeadlineExhausted {
            role: TrafficRole::Shadow,
            stage: BudgetStage::BeforeShadow,
        });
    }

    #[test]
    fn helper_labels_are_bounded() {
        assert_eq!(circuit_state(CircuitState::Closed), "closed");
        assert_eq!(circuit_state(CircuitState::Open), "open");
        assert_eq!(circuit_state(CircuitState::HalfOpen), "half_open");
        assert_eq!(bool_label(true), "true");
        assert_eq!(bool_label(false), "false");
        assert_eq!(route_label(None), "none");
    }
}
