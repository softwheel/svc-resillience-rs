use softwheel_resilience::{
    Route, RouteId, RoutePlanner, RouteTable, SHADOW_PARTS_PER_MILLION, ShadowSampling,
    ShadowSamplingError,
};

fn id(value: &str) -> RouteId {
    RouteId::new(value).unwrap()
}

fn table() -> RouteTable {
    RouteTable::new(
        41,
        vec![
            Route::new(id("a"), 2),
            Route::new(id("b"), 3),
            Route::new(id("c"), 5),
        ],
    )
    .unwrap()
}

#[test]
fn bounded_sampling_thresholds_match_exact_integer_model() {
    for parts_per_million in 1..=1024_u32 {
        let sampling = ShadowSampling::new(parts_per_million).unwrap();

        assert!(sampling.sample_with(|_| 0).unwrap());
        assert!(sampling
            .sample_with(|_| parts_per_million - 1)
            .unwrap());
        assert!(!sampling.sample_with(|_| parts_per_million).unwrap());
        assert!(!sampling
            .sample_with(|_| SHADOW_PARTS_PER_MILLION - 1)
            .unwrap());
        assert_eq!(
            sampling.sample_with(|range| range.end).unwrap_err(),
            ShadowSamplingError::DrawOutOfRange
        );
    }
}

#[test]
fn shadow_sampling_never_changes_primary_selection() {
    let table = table();

    for primary_draw in 0..10_u64 {
        let expected_primary = table
            .select_with(|_| primary_draw)
            .unwrap()
            .id()
            .clone();

        let disabled = RoutePlanner::plan_with(
            &table,
            ShadowSampling::disabled(),
            |_| primary_draw,
            |_| unreachable!("disabled sampling must not draw"),
            |_| unreachable!("unsampled plans must not select a shadow"),
        )
        .unwrap();
        assert_eq!(disabled.primary(), &expected_primary);

        let sampled = RoutePlanner::plan_with(
            &table,
            ShadowSampling::new(500_000).unwrap(),
            |_| primary_draw,
            |_| 0,
            |_| 0,
        )
        .unwrap();
        assert_eq!(sampled.primary(), &expected_primary);

        let unsampled = RoutePlanner::plan_with(
            &table,
            ShadowSampling::new(500_000).unwrap(),
            |_| primary_draw,
            |_| 500_000,
            |_| unreachable!("unsampled plans must not select a shadow"),
        )
        .unwrap();
        assert_eq!(unsampled.primary(), &expected_primary);

        let invalid_sampling_draw = RoutePlanner::plan_with(
            &table,
            ShadowSampling::new(500_000).unwrap(),
            |_| primary_draw,
            |range| range.end,
            |_| unreachable!("invalid sampling draws degrade to unsampled"),
        )
        .unwrap();
        assert_eq!(invalid_sampling_draw.primary(), &expected_primary);
    }
}

#[test]
fn every_bounded_shadow_draw_selects_a_distinct_route() {
    let table = table();

    for primary_draw in 0..10_u64 {
        let primary = table.select_with(|_| primary_draw).unwrap();
        let shadow_total = 10 - primary.weight();

        for shadow_draw in 0..shadow_total {
            let plan = RoutePlanner::plan_with(
                &table,
                ShadowSampling::always(),
                |_| primary_draw,
                |_| unreachable!("always sampling must not draw"),
                |_| shadow_draw,
            )
            .unwrap();

            assert!(plan.shadow_sampled());
            assert!(plan.shadow().is_some());
            assert_ne!(plan.primary(), plan.shadow().unwrap());
        }
    }
}
