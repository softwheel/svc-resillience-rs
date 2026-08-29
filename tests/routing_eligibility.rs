use std::num::NonZeroUsize;
use std::sync::Arc;

use softwheel_resilience::{
    Route, RouteAttemptBudget, RouteDecision, RouteEligibility, RouteFailover, RouteId, RouteOutcome,
    RoutePlanner, RouteTable, ShadowSampling,
};

fn id(value: &str) -> RouteId {
    RouteId::new(value).unwrap()
}

#[test]
fn one_eligibility_decision_governs_primary_shadow_and_failover() {
    let snapshot = Arc::new(
        RouteTable::new(
            41,
            vec![
                Route::new(id("a"), 1),
                Route::new(id("rejected"), 100),
                Route::new(id("c"), 2),
                Route::new(id("d"), 3),
            ],
        )
        .unwrap(),
    );
    let eligibility = RouteEligibility::from_predicate(&snapshot, |route| {
        route.id().as_str() != "rejected"
    });

    let plan = RoutePlanner::plan_eligible_with(
        &snapshot,
        &eligibility,
        ShadowSampling::always(),
        |range| {
            assert_eq!(range, 0..6);
            0
        },
        |_| unreachable!("always sampling performs no draw"),
        |range| {
            assert_eq!(range, 0..5);
            0
        },
    )
    .unwrap();

    assert_eq!(plan.generation(), 41);
    assert_eq!(plan.primary().as_str(), "a");
    assert_eq!(plan.shadow().unwrap().as_str(), "c");

    let mut failover = RouteFailover::from_primary_eligibility(
        Arc::clone(&snapshot),
        eligibility,
        RouteAttemptBudget::new(NonZeroUsize::new(3).unwrap()),
        plan.primary().clone(),
    )
    .unwrap();

    let RouteDecision::Failover(first) = failover
        .classify_with(RouteOutcome::Failover, |range| {
            assert_eq!(range, 0..5);
            0
        })
        .unwrap()
    else {
        panic!("expected first failover")
    };
    assert_eq!(first.route_id().as_str(), "c");

    let RouteDecision::Failover(second) = failover
        .classify_with(RouteOutcome::Failover, |range| {
            assert_eq!(range, 0..3);
            0
        })
        .unwrap()
    else {
        panic!("expected second failover")
    };
    assert_eq!(second.route_id().as_str(), "d");
    assert_ne!(first.route_id().as_str(), "rejected");
    assert_ne!(second.route_id().as_str(), "rejected");
}
