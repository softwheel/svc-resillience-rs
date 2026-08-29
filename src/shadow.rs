use std::fmt;
use std::ops::Range;

use crate::routing::{Route, RouteId, RouteTable, RouteTableError};

/// Exact denominator used by [`ShadowSampling`].
pub const SHADOW_PARTS_PER_MILLION: u32 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShadowSamplingError {
    PartsPerMillionOutOfRange,
    DrawOutOfRange,
}

impl fmt::Display for ShadowSamplingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::PartsPerMillionOutOfRange => {
                "shadow sampling parts-per-million must be at most 1,000,000"
            }
            Self::DrawOutOfRange => "shadow sampling draw was outside the requested range",
        };
        f.write_str(message)
    }
}

impl std::error::Error for ShadowSamplingError {}

/// Exact integer shadow-sampling policy.
///
/// A value of `0` disables sampling and `1_000_000` samples every request. Intermediate values
/// sample exactly that many slots from a one-million-slot integer space, avoiding floating-point
/// boundary ambiguity.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ShadowSampling {
    parts_per_million: u32,
}

impl ShadowSampling {
    pub fn new(parts_per_million: u32) -> Result<Self, ShadowSamplingError> {
        if parts_per_million > SHADOW_PARTS_PER_MILLION {
            return Err(ShadowSamplingError::PartsPerMillionOutOfRange);
        }
        Ok(Self { parts_per_million })
    }

    pub fn disabled() -> Self {
        Self {
            parts_per_million: 0,
        }
    }

    pub fn always() -> Self {
        Self {
            parts_per_million: SHADOW_PARTS_PER_MILLION,
        }
    }

    pub fn parts_per_million(&self) -> u32 {
        self.parts_per_million
    }

    pub fn sample(&self) -> bool {
        self.sample_with(fastrand::u32)
            .expect("fastrand draw is constrained to the requested range")
    }

    pub fn sample_with<F>(&self, draw: F) -> Result<bool, ShadowSamplingError>
    where
        F: FnOnce(Range<u32>) -> u32,
    {
        if self.parts_per_million == 0 {
            return Ok(false);
        }
        if self.parts_per_million == SHADOW_PARTS_PER_MILLION {
            return Ok(true);
        }

        let selected = draw(0..SHADOW_PARTS_PER_MILLION);
        if selected >= SHADOW_PARTS_PER_MILLION {
            return Err(ShadowSamplingError::DrawOutOfRange);
        }
        Ok(selected < self.parts_per_million)
    }
}

/// Immutable primary/shadow routing decision produced from one routing snapshot.
///
/// The plan contains route identity and observability metadata only. It owns no runtime handles,
/// futures, sockets, breakers, bulkheads, or timers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoutePlan {
    generation: u64,
    primary: RouteId,
    shadow: Option<RouteId>,
    shadow_sampled: bool,
}

impl RoutePlan {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn primary(&self) -> &RouteId {
        &self.primary
    }

    pub fn shadow(&self) -> Option<&RouteId> {
        self.shadow.as_ref()
    }

    /// Returns the sampling decision independently of whether a distinct shadow route existed.
    ///
    /// `true` with `shadow() == None` means the request was sampled but no eligible distinct
    /// shadow destination could be selected. That is observable but never a primary-planning
    /// failure.
    pub fn shadow_sampled(&self) -> bool {
        self.shadow_sampled
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoutePlanError {
    PrimarySelection(RouteTableError),
}

impl fmt::Display for RoutePlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PrimarySelection(error) => write!(f, "primary route selection failed: {error}"),
        }
    }
}

impl std::error::Error for RoutePlanError {}

/// Stateless runtime-agnostic planner for a primary route plus optional shadow route.
#[derive(Clone, Copy, Debug, Default)]
pub struct RoutePlanner;

impl RoutePlanner {
    pub fn plan(table: &RouteTable, sampling: ShadowSampling) -> RoutePlan {
        let primary = table.select();
        let sampled = sampling.sample();
        let shadow = sampled
            .then(|| select_shadow(table, primary, fastrand::u64))
            .flatten();
        build_plan(table, primary, sampled, shadow)
    }

    /// Deterministic planning hook for exhaustive tests and callers that own randomness.
    ///
    /// Primary selection is the only fallible part of planning. An invalid sampling draw degrades
    /// to `not sampled`; an invalid/missing shadow draw degrades to `shadow = None`. This makes a
    /// shadow-planning fault incapable of rejecting an otherwise-valid primary plan.
    pub fn plan_with<PrimaryDraw, SampleDraw, ShadowDraw>(
        table: &RouteTable,
        sampling: ShadowSampling,
        primary_draw: PrimaryDraw,
        sample_draw: SampleDraw,
        shadow_draw: ShadowDraw,
    ) -> Result<RoutePlan, RoutePlanError>
    where
        PrimaryDraw: FnOnce(Range<u64>) -> u64,
        SampleDraw: FnOnce(Range<u32>) -> u32,
        ShadowDraw: FnOnce(Range<u64>) -> u64,
    {
        let primary = table
            .select_with(primary_draw)
            .map_err(RoutePlanError::PrimarySelection)?;
        let sampled = sampling.sample_with(sample_draw).unwrap_or(false);
        let shadow = sampled
            .then(|| select_shadow(table, primary, shadow_draw))
            .flatten();
        Ok(build_plan(table, primary, sampled, shadow))
    }
}

fn build_plan(
    table: &RouteTable,
    primary: &Route,
    shadow_sampled: bool,
    shadow: Option<&Route>,
) -> RoutePlan {
    RoutePlan {
        generation: table.generation(),
        primary: primary.id().clone(),
        shadow: shadow.map(|route| route.id().clone()),
        shadow_sampled,
    }
}

fn select_shadow<'a, F>(table: &'a RouteTable, primary: &Route, draw: F) -> Option<&'a Route>
where
    F: FnOnce(Range<u64>) -> u64,
{
    let total = table
        .routes()
        .iter()
        .filter(|route| route.id() != primary.id() && route.is_enabled() && route.weight() > 0)
        .try_fold(0_u64, |sum, route| sum.checked_add(route.weight()))?;

    if total == 0 {
        return None;
    }

    let selected = draw(0..total);
    if selected >= total {
        return None;
    }

    let mut cursor = selected;
    for route in table.routes() {
        if route.id() == primary.id() || !route.is_enabled() || route.weight() == 0 {
            continue;
        }
        if cursor < route.weight() {
            return Some(route);
        }
        cursor -= route.weight();
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> RouteId {
        RouteId::new(value).unwrap()
    }

    fn table() -> RouteTable {
        RouteTable::new(
            17,
            vec![
                Route::new(id("primary-a"), 2),
                Route::new(id("shadow-b"), 3),
                Route::new(id("shadow-c"), 5),
                Route::new(id("disabled"), 100).disabled(),
            ],
        )
        .unwrap()
    }

    #[test]
    fn sampling_validates_exact_integer_bounds() {
        assert_eq!(ShadowSampling::disabled().parts_per_million(), 0);
        assert_eq!(
            ShadowSampling::always().parts_per_million(),
            SHADOW_PARTS_PER_MILLION
        );
        assert_eq!(
            ShadowSampling::new(SHADOW_PARTS_PER_MILLION + 1).unwrap_err(),
            ShadowSamplingError::PartsPerMillionOutOfRange
        );
    }

    #[test]
    fn zero_and_maximum_sampling_do_not_consume_a_draw() {
        let mut calls = 0;
        assert!(
            !ShadowSampling::disabled()
                .sample_with(|_| {
                    calls += 1;
                    0
                })
                .unwrap()
        );
        assert!(
            ShadowSampling::always()
                .sample_with(|_| {
                    calls += 1;
                    0
                })
                .unwrap()
        );
        assert_eq!(calls, 0);
    }

    #[test]
    fn intermediate_sampling_has_exact_boundary() {
        let sampling = ShadowSampling::new(250_000).unwrap();
        assert!(sampling.sample_with(|_| 249_999).unwrap());
        assert!(!sampling.sample_with(|_| 250_000).unwrap());
        assert_eq!(
            sampling
                .sample_with(|_| SHADOW_PARTS_PER_MILLION)
                .unwrap_err(),
            ShadowSamplingError::DrawOutOfRange
        );
    }

    #[test]
    fn planning_preserves_generation_and_selects_distinct_shadow() {
        let plan = RoutePlanner::plan_with(
            &table(),
            ShadowSampling::always(),
            |_| 0,
            |_| unreachable!("always sampling must not draw"),
            |range| {
                assert_eq!(range, 0..8);
                0
            },
        )
        .unwrap();

        assert_eq!(plan.generation(), 17);
        assert_eq!(plan.primary().as_str(), "primary-a");
        assert_eq!(plan.shadow().unwrap().as_str(), "shadow-b");
        assert!(plan.shadow_sampled());
        assert_ne!(plan.primary(), plan.shadow().unwrap());
    }

    #[test]
    fn unsampled_plan_never_attempts_shadow_selection() {
        let plan = RoutePlanner::plan_with(
            &table(),
            ShadowSampling::disabled(),
            |_| 0,
            |_| unreachable!("disabled sampling must not draw"),
            |_| panic!("shadow selection must not run when unsampled"),
        )
        .unwrap();

        assert_eq!(plan.primary().as_str(), "primary-a");
        assert!(!plan.shadow_sampled());
        assert!(plan.shadow().is_none());
    }

    #[test]
    fn sampled_request_without_distinct_route_keeps_primary_plan() {
        let single = RouteTable::new(9, vec![Route::new(id("only"), 1)]).unwrap();
        let plan = RoutePlanner::plan_with(
            &single,
            ShadowSampling::always(),
            |_| 0,
            |_| unreachable!("always sampling must not draw"),
            |_| panic!("no distinct shadow route means no shadow draw"),
        )
        .unwrap();

        assert_eq!(plan.primary().as_str(), "only");
        assert!(plan.shadow_sampled());
        assert!(plan.shadow().is_none());
    }

    #[test]
    fn invalid_shadow_draw_cannot_fail_primary_planning() {
        let plan = RoutePlanner::plan_with(
            &table(),
            ShadowSampling::always(),
            |_| 0,
            |_| unreachable!("always sampling must not draw"),
            |range| range.end,
        )
        .unwrap();

        assert_eq!(plan.primary().as_str(), "primary-a");
        assert!(plan.shadow_sampled());
        assert!(plan.shadow().is_none());
    }

    #[test]
    fn invalid_sampling_draw_degrades_to_unsampled_primary_plan() {
        let plan = RoutePlanner::plan_with(
            &table(),
            ShadowSampling::new(500_000).unwrap(),
            |_| 0,
            |range| range.end,
            |_| panic!("invalid sampling draw must not dispatch shadow selection"),
        )
        .unwrap();

        assert_eq!(plan.primary().as_str(), "primary-a");
        assert!(!plan.shadow_sampled());
        assert!(plan.shadow().is_none());
    }

    #[test]
    fn primary_selection_failure_is_not_hidden_by_shadow_logic() {
        let error = RoutePlanner::plan_with(
            &table(),
            ShadowSampling::always(),
            |range| range.end,
            |_| unreachable!(),
            |_| unreachable!(),
        )
        .unwrap_err();

        assert_eq!(
            error,
            RoutePlanError::PrimarySelection(RouteTableError::DrawOutOfRange)
        );
    }

    #[test]
    fn shadow_weight_boundaries_exclude_primary_and_ineligible_routes() {
        let table = table();
        for (draw, expected) in [
            (0, "shadow-b"),
            (2, "shadow-b"),
            (3, "shadow-c"),
            (7, "shadow-c"),
        ] {
            let plan = RoutePlanner::plan_with(
                &table,
                ShadowSampling::always(),
                |_| 0,
                |_| unreachable!(),
                |_| draw,
            )
            .unwrap();
            assert_eq!(plan.shadow().unwrap().as_str(), expected);
        }
    }
}
