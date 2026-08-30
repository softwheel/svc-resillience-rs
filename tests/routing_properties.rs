use softwheel_resilience::{Route, RouteId, RouteTable};

fn route_id(index: usize) -> RouteId {
    RouteId::new(format!("route-{index}")).unwrap()
}

#[test]
fn bounded_weighted_selection_partitions_every_valid_draw_exactly() {
    const ROUTES: usize = 3;
    const MAX_WEIGHT: u64 = 4;

    for enabled_mask in 0_u8..(1 << ROUTES) {
        for first_weight in 0..=MAX_WEIGHT {
            for second_weight in 0..=MAX_WEIGHT {
                for third_weight in 0..=MAX_WEIGHT {
                    let weights = [first_weight, second_weight, third_weight];
                    let routes: Vec<_> = weights
                        .into_iter()
                        .enumerate()
                        .map(|(index, weight)| {
                            let route = Route::new(route_id(index), weight);
                            if enabled_mask & (1 << index) == 0 {
                                route.disabled()
                            } else {
                                route
                            }
                        })
                        .collect();

                    let expected_weight: u64 = routes
                        .iter()
                        .filter(|route| route.is_enabled())
                        .map(Route::weight)
                        .sum();

                    if expected_weight == 0 {
                        assert!(RouteTable::new(7, routes).is_err());
                        continue;
                    }

                    let table = RouteTable::new(7, routes).unwrap();
                    assert_eq!(table.eligible_weight(), expected_weight);

                    let mut selected_counts = [0_u64; ROUTES];
                    for draw in 0..table.eligible_weight() {
                        let selected = table.select_with(|range| {
                            assert_eq!(range, 0..expected_weight);
                            draw
                        });
                        let selected = selected.unwrap();
                        assert!(selected.is_enabled());
                        assert!(selected.weight() > 0);

                        let index = selected
                            .id()
                            .as_str()
                            .strip_prefix("route-")
                            .unwrap()
                            .parse::<usize>()
                            .unwrap();
                        selected_counts[index] += 1;
                    }

                    for (index, route) in table.routes().iter().enumerate() {
                        let expected = if route.is_enabled() { route.weight() } else { 0 };
                        assert_eq!(selected_counts[index], expected);
                    }
                }
            }
        }
    }
}

#[test]
fn bounded_selection_never_calls_draw_more_than_once() {
    for weight in 1_u64..=32 {
        let table = RouteTable::new(0, vec![Route::new(route_id(0), weight)]).unwrap();
        let mut calls = 0_u8;

        let selected = table
            .select_with(|range| {
                calls += 1;
                range.end - 1
            })
            .unwrap();

        assert_eq!(calls, 1);
        assert_eq!(selected.id().as_str(), "route-0");
    }
}
