use std::collections::HashSet;
use std::fmt;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::Arc;

use crate::routing::{Route, RouteId, RouteTable};

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
///
/// This deliberately separates a retryable physical-attempt failure from a route-terminal
/// failure. `RetrySameRoute` does not consume route-attempt budget; `Failover` may select one new
/// route when budget and eligible routes remain.
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
        f.write_str("route-failover weighted-selection draw was outside the requested range")
    }
}

impl std::error::Error for RouteFailoverError {}

/// Bounded, runtime-agnostic route-failover policy for one logical request.
///
/// The policy retains one immutable routing snapshot for its entire lifetime, never revisits a
/// previously attempted route, and never executes transport work. Physical-attempt retries remain
/// the responsibility of the retry policy used by the caller.
#[derive(Debug)]
pub struct RouteFailover {
    snapshot: Arc<RouteTable>,
    budget: RouteAttemptBudget,
    attempted: HashSet<RouteId>,
    current: RouteAttempt,
}

impl RouteFailover {
    pub fn new(snapshot: Arc<RouteTable>, budget: RouteAttemptBudget) -> Self {
        let primary = snapshot.select();
        Self::from_primary(snapshot, budget, primary.id().clone())
    }

    pub fn new_with<F>(
        snapshot: Arc<RouteTable>,
        budget: RouteAttemptBudget,
        draw: F,
    ) -> Result<Self, RouteFailoverError>
    where
        F: FnOnce(Range<u64>) -> u64,
    {
        let primary = select_remaining(&snapshot, &HashSet::new(), draw)?
            .expect("validated route table always has an eligible primary route");
        Ok(Self::from_primary(snapshot, budget, primary.id().clone()))
    }

    fn from_primary(
        snapshot: Arc<RouteTable>,
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

                let Some(route) = select_remaining(&self.snapshot, &self.attempted, draw)? else {
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
    attempted: &HashSet<RouteId>,
    draw: F,
) -> Result<Option<&'a Route>, RouteFailoverError>
where
    F: FnOnce(Range<u64>) -> u64,
{
    let total = table
        .routes()
        .iter()
        .filter(|route| is_remaining_eligible(route, attempted))
        .try_fold(0_u64, |sum, route| sum.checked_add(route.weight()))
        .expect("subset of a validated route table cannot overflow its validated total weight");

    if total == 0 {
        return Ok(None);
    }

    let selected = draw(0..total);
    if selected >= total {
        return Err(RouteFailoverError);
    }

    let mut cursor = selected;
    for route in table
        .routes()
        .iter()
        .filter(|route| is_remaining_eligible(route, attempted))
    {
        if cursor < route.weight() {
            return Ok(Some(route));
        }
        cursor -= route.weight();
    }

    unreachable!("validated remaining weight must map every in-range draw to a route")
}

fn is_remaining_eligible(route: &Route, attempted: &HashSet<RouteId>) -> bool {
    route.is_enabled() && route.weight() > 0 && !attempted.contains(route.id())
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

        let first = failover.classify_with(RouteOutcome::Failover, |_| 0).unwrap();
        let RouteDecision::Failover(first) = first else {
            panic!("expected first failover")
        };
        assert_eq!(first.route_id().as_str(), "b");
        assert_eq!(first.ordinal(), 1);

        let second = failover.classify_with(RouteOutcome::Failover, |_| 0).unwrap();
        let RouteDecision::Failover(second) = second else {
            panic!("expected second failover")
        };
        assert_eq!(second.route_id().as_str(), "c");
        assert_eq!(second.ordinal(), 2);
        assert_eq!(second.generation(), 17);

        assert_eq!(
            failover.classify(RouteOutcome::Failover),
            RouteDecision::Stop(RouteStopReason::RouteAttemptBudgetExhausted)
        );
        assert_eq!(failover.attempted_routes(), 3);
    }

    #[test]
    fn failover_weighting_uses_only_remaining_eligible_routes() {
        let mut failover = RouteFailover::new_with(table(), budget(3), |_| 0).unwrap();

        let next = failover.classify_with(RouteOutcome::Failover, |range| {
            assert_eq!(range, 0..5);
            4
        });
        let RouteDecision::Failover(next) = next.unwrap() else {
            panic!("expected failover")
        };
        assert_eq!(next.route_id().as_str(), "c");

        let final_route = failover.classify_with(RouteOutcome::Failover, |range| {
            assert_eq!(range, 0..2);
            1
        });
        let RouteDecision::Failover(final_route) = final_route.unwrap() else {
            panic!("expected failover")
        };
        assert_eq!(final_route.route_id().as_str(), "b");
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
