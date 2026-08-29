use std::collections::HashSet;
use std::fmt;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::Arc;

use crate::eligibility::{RouteEligibility, select_weighted_with};
use crate::routing::{RouteId, RouteTable};

/// Maximum number of distinct routes a logical request may attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RouteAttemptBudget {
    max_routes: NonZeroUsize,
}

impl RouteAttemptBudget {
    pub const fn new(max_routes: NonZeroUsize) -> Self {
        Self { max_routes }
    }

    pub const fn max_routes(self) -> usize {
        self.max_routes.get()
    }
}

/// Caller classification of one route execution outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteOutcome {
    Succeeded,
    RetrySameRoute,
    Failover,
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteStopReason {
    CallerTerminal,
    RouteAttemptBudgetExhausted,
    NoRemainingEligibleRoute,
}

/// Immutable metadata for one distinct route attempt in a logical request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteAttempt {
    generation: u64,
    ordinal: usize,
    route_id: RouteId,
}

impl RouteAttempt {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Zero-based route-failover ordinal. The initial primary route is ordinal 0.
    pub fn ordinal(&self) -> usize {
        self.ordinal
    }

    pub fn route_id(&self) -> &RouteId {
        &self.route_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RouteDecision {
    Complete,
    RetryCurrent,
    Failover(RouteAttempt),
    Stop(RouteStopReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RouteFailoverError;

impl fmt::Display for RouteFailoverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("route-failover selection was inconsistent with its routing snapshot")
    }
}

impl std::error::Error for RouteFailoverError {}

/// Bounded, runtime-agnostic route-failover policy for one logical request.
///
/// The policy retains one immutable routing snapshot and one immutable eligibility decision for
/// its entire lifetime. It never revisits a previously attempted route and never executes
/// transport work. Physical-attempt retries remain the responsibility of the caller's retry
/// policy.
#[derive(Debug)]
pub struct RouteFailover {
    snapshot: Arc<RouteTable>,
    eligibility: RouteEligibility,
    budget: RouteAttemptBudget,
    attempted: HashSet<RouteId>,
    current: RouteAttempt,
}

impl RouteFailover {
    /// Construct a failover policy with every enabled, positive-weight route eligible.
    pub fn new(snapshot: Arc<RouteTable>, budget: RouteAttemptBudget) -> Self {
        let eligibility = RouteEligibility::all(&snapshot);
        let primary = snapshot.select().id().clone();
        Self::from_primary(snapshot, eligibility, budget, primary)
    }

    /// Deterministic constructor with every enabled, positive-weight route eligible.
    pub fn new_with<F>(
        snapshot: Arc<RouteTable>,
        budget: RouteAttemptBudget,
        draw: F,
    ) -> Result<Self, RouteFailoverError>
    where
        F: FnOnce(Range<u64>) -> u64,
    {
        let eligibility = RouteEligibility::all(&snapshot);
        let primary = select_remaining(&snapshot, &eligibility, &HashSet::new(), draw)?
            .expect("validated route table always has an eligible primary route")
            .id()
            .clone();
        Ok(Self::from_primary(snapshot, eligibility, budget, primary))
    }

    /// Continue failover from a primary route selected with the same immutable eligibility
    /// decision used by route planning.
    ///
    /// This is the preferred constructor when health/policy filtering is active. Cross-generation
    /// reuse and primaries rejected by the decision are rejected before any failover state exists.
    pub fn from_primary_eligibility(
        snapshot: Arc<RouteTable>,
        eligibility: RouteEligibility,
        budget: RouteAttemptBudget,
        primary: RouteId,
    ) -> Result<Self, RouteFailoverError> {
        if eligibility.generation() != snapshot.generation() || !eligibility.contains(&primary) {
            return Err(RouteFailoverError);
        }
        Ok(Self::from_primary(snapshot, eligibility, budget, primary))
    }

    fn from_primary(
        snapshot: Arc<RouteTable>,
        eligibility: RouteEligibility,
        budget: RouteAttemptBudget,
        primary: RouteId,
    ) -> Self {
        let mut attempted = HashSet::new();
        attempted.insert(primary.clone());
        let current = RouteAttempt {
            generation: snapshot.generation(),
            ordinal: 0,
            route_id: primary,
        };
        Self {
            snapshot,
            eligibility,
            budget,
            attempted,
            current,
        }
    }

    pub fn current(&self) -> &RouteAttempt {
        &self.current
    }

    pub fn attempted_routes(&self) -> usize {
        self.attempted.len()
    }

    pub fn classify(&mut self, outcome: RouteOutcome) -> RouteDecision {
        self.classify_with(outcome, fastrand::u64)
            .expect("fastrand draw is constrained to the requested range")
    }

    pub fn classify_with<F>(
        &mut self,
        outcome: RouteOutcome,
        draw: F,
    ) -> Result<RouteDecision, RouteFailoverError>
    where
        F: FnOnce(Range<u64>) -> u64,
    {
        match outcome {
            RouteOutcome::Succeeded => Ok(RouteDecision::Complete),
            RouteOutcome::RetrySameRoute => Ok(RouteDecision::RetryCurrent),
            RouteOutcome::Stop => Ok(RouteDecision::Stop(RouteStopReason::CallerTerminal)),
            RouteOutcome::Failover => {
                if self.attempted.len() >= self.budget.max_routes() {
                    return Ok(RouteDecision::Stop(
                        RouteStopReason::RouteAttemptBudgetExhausted,
                    ));
                }

                let Some(route) = select_remaining(
                    &self.snapshot,
                    &self.eligibility,
                    &self.attempted,
                    draw,
                )? else {
                    return Ok(RouteDecision::Stop(
                        RouteStopReason::NoRemainingEligibleRoute,
                    ));
                };

                let next = RouteAttempt {
                    generation: self.snapshot.generation(),
                    ordinal: self.attempted.len(),
                    route_id: route.id().clone(),
                };
                self.attempted.insert(next.route_id.clone());
                self.current = next.clone();
                Ok(RouteDecision::Failover(next))
            }
        }
    }
}

fn select_remaining<'a, F>(
    table: &'a RouteTable,
    eligibility: &RouteEligibility,
    attempted: &HashSet<RouteId>,
    draw: F,
) -> Result<Option<&'a crate::routing::Route>, RouteFailoverError>
where
    F: FnOnce(Range<u64>) -> u64,
{
    select_weighted_with(
        table,
        eligibility,
        |route| attempted.contains(route.id()),
        draw,
    )
    .map_err(|_| RouteFailoverError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::{Route, RouteId, RouteTable};

    fn id(value: &str) -> RouteId {
        RouteId::new(value).unwrap()
    }

    fn table() -> Arc<RouteTable> {
        Arc::new(
            RouteTable::new(
                17,
                vec![
                    Route::new(id("a"), 1),
                    Route::new(id("b"), 2),
                    Route::new(id("c"), 3),
                    Route::new(id("zero"), 0),
                    Route::new(id("disabled"), 100).disabled(),
                ],
            )
            .unwrap(),
        )
    }

    fn budget(max_routes: usize) -> RouteAttemptBudget {
        RouteAttemptBudget::new(NonZeroUsize::new(max_routes).unwrap())
    }

    #[test]
    fn retry_same_route_does_not_consume_route_budget() {
        let mut failover = RouteFailover::new_with(table(), budget(2), |_| 0).unwrap();
        assert_eq!(failover.current().route_id().as_str(), "a");

        for _ in 0..10 {
            assert_eq!(
                failover.classify(RouteOutcome::RetrySameRoute),
                RouteDecision::RetryCurrent
            );
        }

        assert_eq!(failover.attempted_routes(), 1);
        assert_eq!(failover.current().ordinal(), 0);
    }

    #[test]
    fn failover_is_bounded_and_never_revisits_a_route() {
        let mut failover = RouteFailover::new_with(table(), budget(3), |_| 0).unwrap();

        let RouteDecision::Failover(first) = failover
            .classify_with(RouteOutcome::Failover, |_| 0)
            .unwrap()
        else {
            panic!("expected first failover")
        };
        assert_eq!(first.route_id().as_str(), "b");
        assert_eq!(first.ordinal(), 1);

        let RouteDecision::Failover(second) = failover
            .classify_with(RouteOutcome::Failover, |_| 0)
            .unwrap()
        else {
            panic!("expected second failover")
        };
        assert_eq!(second.route_id().as_str(), "c");
        assert_eq!(second.ordinal(), 2);
        assert_eq!(second.generation(), 17);

        assert_eq!(
            failover.classify(RouteOutcome::Failover),
            RouteDecision::Stop(RouteStopReason::RouteAttemptBudgetExhausted)
        );
    }

    #[test]
    fn failover_weighting_uses_only_remaining_eligible_routes() {
        let mut failover = RouteFailover::new_with(table(), budget(3), |_| 0).unwrap();
        let RouteDecision::Failover(next) = failover
            .classify_with(RouteOutcome::Failover, |range| {
                assert_eq!(range, 0..5);
                4
            })
            .unwrap()
        else {
            panic!("expected failover")
        };
        assert_eq!(next.route_id().as_str(), "c");

        let RouteDecision::Failover(final_route) = failover
            .classify_with(RouteOutcome::Failover, |range| {
                assert_eq!(range, 0..2);
                1
            })
            .unwrap()
        else {
            panic!("expected failover")
        };
        assert_eq!(final_route.route_id().as_str(), "b");
    }

    #[test]
    fn shared_eligibility_excludes_rejected_routes_from_failover() {
        let snapshot = table();
        let eligibility = RouteEligibility::from_predicate(&snapshot, |route| {
            route.id().as_str() != "b"
        });
        let mut failover = RouteFailover::from_primary_eligibility(
            Arc::clone(&snapshot),
            eligibility,
            budget(3),
            id("a"),
        )
        .unwrap();

        let RouteDecision::Failover(next) = failover
            .classify_with(RouteOutcome::Failover, |range| {
                assert_eq!(range, 0..3);
                0
            })
            .unwrap()
        else {
            panic!("expected failover")
        };
        assert_eq!(next.route_id().as_str(), "c");

        assert_eq!(
            failover.classify(RouteOutcome::Failover),
            RouteDecision::Stop(RouteStopReason::NoRemainingEligibleRoute)
        );
    }

    #[test]
    fn shared_eligibility_rejects_cross_generation_and_rejected_primary() {
        let snapshot = table();
        let other = RouteTable::new(18, snapshot.routes().to_vec()).unwrap();
        let old_eligibility = RouteEligibility::all(&other);
        assert_eq!(
            RouteFailover::from_primary_eligibility(
                Arc::clone(&snapshot),
                old_eligibility,
                budget(2),
                id("a"),
            )
            .unwrap_err(),
            RouteFailoverError
        );

        let eligibility = RouteEligibility::from_predicate(&snapshot, |route| {
            route.id().as_str() != "a"
        });
        assert_eq!(
            RouteFailover::from_primary_eligibility(
                snapshot,
                eligibility,
                budget(2),
                id("a"),
            )
            .unwrap_err(),
            RouteFailoverError
        );
    }

    #[test]
    fn no_remaining_route_is_a_normal_stop_reason() {
        let snapshot = Arc::new(RouteTable::new(9, vec![Route::new(id("only"), 1)]).unwrap());
        let mut failover = RouteFailover::new(snapshot, budget(2));
        assert_eq!(
            failover.classify(RouteOutcome::Failover),
            RouteDecision::Stop(RouteStopReason::NoRemainingEligibleRoute)
        );
    }

    #[test]
    fn caller_terminal_and_success_do_not_select_another_route() {
        let mut failover = RouteFailover::new_with(table(), budget(3), |_| 0).unwrap();
        assert_eq!(
            failover.classify(RouteOutcome::Stop),
            RouteDecision::Stop(RouteStopReason::CallerTerminal)
        );
        assert_eq!(
            failover.classify(RouteOutcome::Succeeded),
            RouteDecision::Complete
        );
        assert_eq!(failover.attempted_routes(), 1);
    }

    #[test]
    fn invalid_deterministic_draw_fails_once_without_mutating_state() {
        let mut failover = RouteFailover::new_with(table(), budget(3), |_| 0).unwrap();
        let before = failover.current().clone();
        let mut calls = 0;

        let error = failover
            .classify_with(RouteOutcome::Failover, |range| {
                calls += 1;
                range.end
            })
            .unwrap_err();

        assert_eq!(error, RouteFailoverError);
        assert_eq!(calls, 1);
        assert_eq!(failover.current(), &before);
        assert_eq!(failover.attempted_routes(), 1);
    }
}
