//! Runtime-agnostic shadow execution outcomes for adapter observability.
//!
//! The core crate never returns a shadow failure as the primary result. Runtime adapters can use
//! these values to report why isolated shadow work finished, was rejected, or was cancelled while
//! preserving that non-propagation contract.

/// Why isolated shadow work did not run to successful completion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShadowOutcome<E> {
    /// Shadow work completed successfully.
    Succeeded,
    /// The isolated shadow bulkhead could not admit work immediately.
    Overloaded,
    /// The adapter cancelled already-dispatched shadow work.
    Cancelled,
    /// The bounded shadow deadline expired.
    DeadlineExceeded,
    /// Shadow execution completed with an adapter/transport error.
    Failed(E),
}

impl<E> ShadowOutcome<E> {
    /// Returns `true` only when shadow execution completed successfully.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Succeeded)
    }

    /// Returns the contained shadow error, if execution completed with an error.
    ///
    /// This gives adapters access to the error for logs, metrics, or tracing without coupling the
    /// error to the primary call result.
    pub fn error(&self) -> Option<&E> {
        match self {
            Self::Failed(error) => Some(error),
            _ => None,
        }
    }

    /// Maps only the contained shadow error while preserving the outcome classification.
    pub fn map_error<F, M>(self, map: M) -> ShadowOutcome<F>
    where
        M: FnOnce(E) -> F,
    {
        match self {
            Self::Succeeded => ShadowOutcome::Succeeded,
            Self::Overloaded => ShadowOutcome::Overloaded,
            Self::Cancelled => ShadowOutcome::Cancelled,
            Self::DeadlineExceeded => ShadowOutcome::DeadlineExceeded,
            Self::Failed(error) => ShadowOutcome::Failed(map(error)),
        }
    }
}

/// Adapter-facing context for one planned shadow execution.
///
/// This value intentionally contains only bounded routing metadata and no runtime handles. It can
/// be paired with [`ShadowOutcome`] to emit telemetry without making the crate depend on a
/// telemetry implementation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShadowObservation<'a> {
    generation: u64,
    route_id: &'a crate::routing::RouteId,
}

impl<'a> ShadowObservation<'a> {
    /// Creates observation metadata from a route plan that actually selected a shadow route.
    ///
    /// Returns `None` for unsampled plans and sampled plans where no distinct shadow route could be
    /// selected.
    ///
    /// ```
    /// use svc_resilience::{
    ///     Route, RouteId, RoutePlanner, RouteTable, ShadowObservation, ShadowSampling,
    /// };
    ///
    /// let table = RouteTable::new(
    ///     7,
    ///     vec![
    ///         Route::new(RouteId::new("primary").unwrap(), 1),
    ///         Route::new(RouteId::new("shadow").unwrap(), 1),
    ///     ],
    /// )
    /// .unwrap();
    /// let plan = RoutePlanner::plan_with(
    ///     &table,
    ///     ShadowSampling::always(),
    ///     |_| 0,
    ///     |_| unreachable!(),
    ///     |_| 0,
    /// )
    /// .unwrap();
    ///
    /// let observation = ShadowObservation::from_plan(&plan).unwrap();
    /// assert_eq!(observation.generation(), 7);
    /// assert_eq!(observation.route_id().as_str(), "shadow");
    /// ```
    pub fn from_plan(plan: &'a crate::shadow::RoutePlan) -> Option<Self> {
        Some(Self {
            generation: plan.generation(),
            route_id: plan.shadow()?,
        })
    }

    /// Routing generation used to produce the immutable route plan.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Selected shadow route identifier.
    pub fn route_id(&self) -> &crate::routing::RouteId {
        self.route_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Route, RouteId, RoutePlanner, RouteTable, ShadowSampling};

    #[test]
    fn shadow_failures_remain_observable_without_becoming_primary_errors() {
        let outcome = ShadowOutcome::Failed("shadow transport failed");
        assert!(!outcome.is_success());
        assert_eq!(outcome.error(), Some(&"shadow transport failed"));

        let mapped = outcome.map_error(str::len);
        assert_eq!(mapped, ShadowOutcome::Failed(23));
    }

    #[test]
    fn all_non_success_terminal_states_are_explicit() {
        let outcomes: [ShadowOutcome<()>; 4] = [
            ShadowOutcome::Overloaded,
            ShadowOutcome::Cancelled,
            ShadowOutcome::DeadlineExceeded,
            ShadowOutcome::Failed(()),
        ];
        assert!(outcomes.iter().all(|outcome| !outcome.is_success()));
    }

    #[test]
    fn observation_uses_immutable_plan_generation_and_shadow_route() {
        let table = RouteTable::new(
            42,
            vec![
                Route::new(RouteId::new("primary").unwrap(), 1),
                Route::new(RouteId::new("shadow").unwrap(), 1),
            ],
        )
        .unwrap();
        let plan = RoutePlanner::plan_with(
            &table,
            ShadowSampling::always(),
            |_| 0,
            |_| unreachable!(),
            |_| 0,
        )
        .unwrap();

        let observation = ShadowObservation::from_plan(&plan).unwrap();
        assert_eq!(observation.generation(), 42);
        assert_eq!(observation.route_id().as_str(), "shadow");
    }

    #[test]
    fn observation_is_absent_when_no_shadow_route_was_planned() {
        let table =
            RouteTable::new(9, vec![Route::new(RouteId::new("primary").unwrap(), 1)]).unwrap();
        let plan = RoutePlanner::plan_with(
            &table,
            ShadowSampling::always(),
            |_| 0,
            |_| unreachable!(),
            |_| unreachable!(),
        )
        .unwrap();

        assert!(ShadowObservation::from_plan(&plan).is_none());
    }
}
