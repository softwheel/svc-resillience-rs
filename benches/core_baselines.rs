use std::hint::black_box;
use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use softwheel_resilience::{
    Bulkhead, CircuitBreaker, CircuitBreakerConfig, NoopObserver, Observer, OutcomeClass,
    PrimaryRetryBudget, ResilienceEvent, RetryBudgetConfig, Route, RouteId, RouteTable,
    RouteTableStore, TrafficRole,
};

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

    let store = RouteTableStore::new(route_table(8));
    group.bench_function("replace_8_routes", |b| {
        b.iter(|| {
            let routes = (0..8)
                .map(|index| {
                    Route::new(
                        RouteId::new(format!("replacement-{index}"))
                            .expect("non-empty benchmark route id"),
                        1,
                    )
                })
                .collect();
            black_box(
                store
                    .replace(routes)
                    .expect("benchmark replacement is valid"),
            );
        });
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

fn breaker_baselines(c: &mut Criterion) {
    let breaker = CircuitBreaker::new(
        CircuitBreakerConfig::new(8, Duration::from_secs(1), 1)
            .expect("benchmark breaker config is valid"),
    );

    c.bench_function("circuit_breaker/closed_successful_call", |b| {
        b.iter(|| {
            black_box(breaker.call(|| Ok::<(), ()>(())))
                .expect("closed benchmark breaker admits calls");
        });
    });
}

fn retry_budget_baselines(c: &mut Criterion) {
    let budget = PrimaryRetryBudget::new(RetryBudgetConfig::new(
        NonZeroU32::new(64).expect("non-zero benchmark capacity"),
        NonZeroU32::new(1).expect("non-zero benchmark replenishment"),
    ));

    c.bench_function("retry_budget/acquire_and_replenish", |b| {
        b.iter(|| {
            black_box(budget.try_acquire_retry());
            black_box(budget.record_success());
        });
    });
}

#[derive(Default)]
struct CountingObserver {
    count: AtomicU64,
}

impl Observer for CountingObserver {
    fn observe(&self, _event: &ResilienceEvent) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }
}

fn observability_baselines(c: &mut Criterion) {
    let route_id = RouteId::new("benchmark-route").expect("non-empty benchmark route id");
    let event = ResilienceEvent::AttemptCompleted {
        role: TrafficRole::Primary,
        route_id,
        attempt_ordinal: 1,
        outcome: OutcomeClass::Succeeded,
        latency: Duration::from_micros(250),
    };

    let noop = NoopObserver;
    c.bench_function("observability/noop_observer", |b| {
        b.iter(|| noop.observe(black_box(&event)));
    });

    let counting = CountingObserver::default();
    c.bench_function("observability/counting_observer", |b| {
        b.iter(|| counting.observe(black_box(&event)));
    });
    black_box(counting.count.load(Ordering::Relaxed));
}

criterion_group!(
    benches,
    routing_baselines,
    bulkhead_baselines,
    breaker_baselines,
    retry_budget_baselines,
    observability_baselines
);
criterion_main!(benches);
