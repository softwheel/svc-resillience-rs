use std::collections::HashSet;
use std::fmt;
use std::ops::Range;

use crate::routing::{Route, RouteId, RouteTable};

/// Immutable eligibility decision derived from one routing snapshot.
///
/// Callers may derive this once from declarative health/policy state and reuse it for primary,
/// shadow, and failover selection throughout one logical request. This prevents a time-varying
/// predicate from being re-evaluated between routing decisions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteEligibility {
    generation: u64,
    eligible: HashSet<RouteId>,
}

impl RouteEligibility {
    /// Accept every enabled route with positive weight from this snapshot.
    pub fn all(table: &RouteTable) -> Self {
        Self::from_predicate(table, |_| true)
    }

    /// Evaluate caller-supplied declarative health/policy exactly once per route.
    ///
    /// Circuit-breaker admission is deliberately not part of this predicate. Breaker state is
    /// execution state and remains per physical attempt.
    pub fn from_predicate<F>(table: &RouteTable, mut predicate: F) -> Self
    where
        F: FnMut(&Route) -> bool,
    {
        let eligible = table
            .routes()
            .iter()
            .filter(|route| route.is_enabled() && route.weight() > 0 && predicate(route))
            .map(|route| route.id().clone())
            .collect();
        Self {
            generation: table.generation(),
            eligible,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn contains(&self, route_id: &RouteId) -> bool {
        self.eligible.contains(route_id)
    }

    pub fn is_empty(&self) -> bool {
        self.eligible.is_empty()
    }

    /// Deterministically select one eligible route after health/policy filtering.
    ///
    /// `Ok(None)` means no eligible positive-weight route remains. The draw is invoked at most
    /// once and selection performs no retry or execution-side mutation.
    pub fn select_with<'a, Draw>(
        &self,
        table: &'a RouteTable,
        draw: Draw,
    ) -> Result<Option<&'a Route>, RouteEligibilityError>
    where
        Draw: FnOnce(Range<u64>) -> u64,
    {
        select_weighted_with(table, self, |_| false, draw)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteEligibilityError {
    GenerationMismatch,
    DrawOutOfRange,
}

impl fmt::Display for RouteEligibilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::GenerationMismatch => {
                "route eligibility was derived from a different routing generation"
            }
            Self::DrawOutOfRange => "eligible-route weighted draw was outside the requested range",
        };
        f.write_str(message)
    }
}

impl std::error::Error for RouteEligibilityError {}

/// Select from one immutable eligibility decision, optionally excluding routes already consumed by
/// another planning decision. `Ok(None)` means no eligible positive-weight route remains.
pub(crate) fn select_weighted_with<'a, Exclude, Draw>(
    table: &'a RouteTable,
    eligibility: &RouteEligibility,
    mut excluded: Exclude,
    draw: Draw,
) -> Result<Option<&'a Route>, RouteEligibilityError>
where
    Exclude: FnMut(&Route) -> bool,
    Draw: FnOnce(Range<u64>) -> u64,
{
    if table.generation() != eligibility.generation {
        return Err(RouteEligibilityError::GenerationMismatch);
    }

    let total = table
        .routes()
        .iter()
        .filter(|route| eligibility.contains(route.id()) && !excluded(route))
        .try_fold(0_u64, |sum, route| sum.checked_add(route.weight()))
        .expect("eligible subset of a validated route table cannot overflow its total weight");

    if total == 0 {
        return Ok(None);
    }

    let selected = draw(0..total);
    if selected >= total {
        return Err(RouteEligibilityError::DrawOutOfRange);
    }

    let mut cursor = selected;
    for route in table
        .routes()
        .iter()
        .filter(|route| eligibility.contains(route.id()) && !excluded(route))
    {
        if cursor < route.weight() {
            return Ok(Some(route));
        }
        cursor -= route.weight();
    }

    unreachable!("validated eligible weight must map every in-range draw to a route")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::{Route, RouteId, RouteTable};

    fn id(value: &str) -> RouteId {
        RouteId::new(value).unwrap()
    }

    fn table(generation: u64) -> RouteTable {
        RouteTable::new(
            generation,
            vec![
                Route::new(id("healthy-a"), 2),
                Route::new(id("unhealthy"), 100),
                Route::new(id("healthy-b"), 3),
                Route::new(id("disabled"), 100).disabled(),
                Route::new(id("zero"), 0),
            ],
        )
        .unwrap()
    }

    #[test]
    fn predicate_is_evaluated_once_per_enabled_positive_weight_route() {
        let table = table(7);
        let mut visited = Vec::new();
        let eligibility = RouteEligibility::from_predicate(&table, |route| {
            visited.push(route.id().as_str().to_owned());
            route.id().as_str().starts_with("healthy")
        });

        assert_eq!(
            visited,
            vec![
                "healthy-a".to_owned(),
                "unhealthy".to_owned(),
                "healthy-b".to_owned()
            ]
        );
        assert!(eligibility.contains(&id("healthy-a")));
        assert!(eligibility.contains(&id("healthy-b")));
        assert!(!eligibility.contains(&id("unhealthy")));
        assert!(!eligibility.contains(&id("disabled")));
        assert!(!eligibility.contains(&id("zero")));
    }

    #[test]
    fn weighting_happens_after_health_policy_filtering() {
        let table = table(7);
        let eligibility = RouteEligibility::from_predicate(&table, |route| {
            route.id().as_str() != "unhealthy"
        });

        let first = eligibility
            .select_with(&table, |range| {
                assert_eq!(range, 0..5);
                0
            })
            .unwrap()
            .unwrap();
        let last = eligibility.select_with(&table, |_| 4).unwrap().unwrap();

        assert_eq!(first.id().as_str(), "healthy-a");
        assert_eq!(last.id().as_str(), "healthy-b");
    }

    #[test]
    fn empty_policy_result_is_not_a_transport_failure() {
        let table = table(7);
        let eligibility = RouteEligibility::from_predicate(&table, |_| false);
        assert!(eligibility.is_empty());
        assert!(
            eligibility
                .select_with(&table, |_| unreachable!())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn eligibility_cannot_cross_snapshot_generations() {
        let old = table(7);
        let new = table(8);
        let eligibility = RouteEligibility::all(&old);

        assert_eq!(
            eligibility.select_with(&new, |_| 0).unwrap_err(),
            RouteEligibilityError::GenerationMismatch
        );
    }
}
