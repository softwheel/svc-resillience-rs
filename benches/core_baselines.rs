use std::hint::black_box;
use std::num::NonZeroUsize;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use softwheel_resilience::{Bulkhead, Route, RouteId, RouteTable, RouteTableStore};

fn route_table(route_count: usize) -> RouteTable {
    let routes = (0..route_count)
        .map(|index| {
            Route::new(
                RouteId::new(format!("route-{index}")).expect("non-empty benchmark route id"),
                1,
            )
        })
        .collect();
    RouteTable::new(0, routes).expect("benchmark route table is valid")
}

fn routing_baselines(c: &mut Criterion) {
    let mut group = c.benchmark_group("routing");

    for route_count in [1_usize, 8, 64] {
        let table = route_table(route_count);
        group.bench_with_input(
            BenchmarkId::new("select_last_weighted_route", route_count),
            &route_count,
            |b, _| {
                b.iter(|| {
                    let selected = table
                        .select_with(|range| range.end - 1)
                        .expect("benchmark draw is in range");
                    black_box(selected.id());
                });
            },
        );
    }

    let store = RouteTableStore::new(route_table(8));
    group.bench_function("snapshot_arc_clone_8_routes", |b| {
        b.iter(|| black_box(store.snapshot()));
    });

    group.finish();
}

fn bulkhead_baselines(c: &mut Criterion) {
    let bulkhead = Bulkhead::new(NonZeroUsize::new(64).expect("non-zero benchmark capacity"));

    c.bench_function("bulkhead/try_acquire_and_drop", |b| {
        b.iter(|| {
            let permit = bulkhead
                .try_acquire()
                .expect("benchmark bulkhead has available capacity");
            black_box(&permit);
            drop(permit);
        });
    });
}

criterion_group!(benches, routing_baselines, bulkhead_baselines);
criterion_main!(benches);
