use std::collections::HashSet;
use std::fmt;
use std::ops::Range;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RouteId(String);

impl RouteId {
    pub fn new(id: impl Into<String>) -> Result<Self, RouteTableError> {
        let id = id.into();
        if id.is_empty() {
            return Err(RouteTableError::EmptyRouteId);
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RouteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Route {
    id: RouteId,
    weight: u64,
    enabled: bool,
}

impl Route {
    pub fn new(id: RouteId, weight: u64) -> Self {
        Self {
            id,
            weight,
            enabled: true,
        }
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    pub fn id(&self) -> &RouteId {
        &self.id
    }

    pub fn weight(&self) -> u64 {
        self.weight
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteTableError {
    EmptyRouteTable,
    EmptyRouteId,
    DuplicateRouteId,
    NoWeightedEligibleRoute,
    WeightOverflow,
    DrawOutOfRange,
    GenerationExhausted,
}

impl fmt::Display for RouteTableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EmptyRouteTable => "route table must contain at least one route",
            Self::EmptyRouteId => "route id must not be empty",
            Self::DuplicateRouteId => "route table contains a duplicate route id",
            Self::NoWeightedEligibleRoute => {
                "route table has no enabled route with positive weight"
            }
            Self::WeightOverflow => "total eligible route weight overflowed u64",
            Self::DrawOutOfRange => "weighted-selection draw was outside the requested range",
            Self::GenerationExhausted => "route-table generation exhausted u64",
        };
        f.write_str(message)
    }
}

impl std::error::Error for RouteTableError {}

/// Immutable, validated routing snapshot.
///
/// Route ordering is preserved exactly as supplied. Weighted selection first excludes disabled
/// and zero-weight routes, then draws once from the checked total weight. No retries or execution
/// side effects occur during selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteTable {
    generation: u64,
    routes: Vec<Route>,
    eligible_weight: u64,
}

impl RouteTable {
    pub fn new(generation: u64, routes: Vec<Route>) -> Result<Self, RouteTableError> {
        if routes.is_empty() {
            return Err(RouteTableError::EmptyRouteTable);
        }

        let mut ids = HashSet::with_capacity(routes.len());
        let mut eligible_weight = 0_u64;
        for route in &routes {
            if !ids.insert(route.id.clone()) {
                return Err(RouteTableError::DuplicateRouteId);
            }
            if route.enabled && route.weight > 0 {
                eligible_weight = eligible_weight
                    .checked_add(route.weight)
                    .ok_or(RouteTableError::WeightOverflow)?;
            }
        }

        if eligible_weight == 0 {
            return Err(RouteTableError::NoWeightedEligibleRoute);
        }

        Ok(Self {
            generation,
            routes,
            eligible_weight,
        })
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn routes(&self) -> &[Route] {
        &self.routes
    }

    pub fn eligible_weight(&self) -> u64 {
        self.eligible_weight
    }

    pub fn select(&self) -> &Route {
        self.select_with(fastrand::u64)
            .expect("fastrand draw is constrained to the requested range")
    }

    pub fn select_with<F>(&self, draw: F) -> Result<&Route, RouteTableError>
    where
        F: FnOnce(Range<u64>) -> u64,
    {
        let selected = draw(0..self.eligible_weight);
        if selected >= self.eligible_weight {
            return Err(RouteTableError::DrawOutOfRange);
        }

        let mut cursor = selected;
        for route in &self.routes {
            if !route.enabled || route.weight == 0 {
                continue;
            }
            if cursor < route.weight {
                return Ok(route);
            }
            cursor -= route.weight;
        }

        unreachable!("validated eligible weight must map every in-range draw to a route")
    }
}

/// Concurrent holder for one complete immutable routing snapshot.
///
/// Readers clone a single `Arc<RouteTable>` under a read lock and then plan entirely from that
/// immutable snapshot. Replacements are serialized, validated before publication, and increment
/// generation exactly once. Existing snapshots remain valid after publication of a newer table.
#[derive(Debug)]
pub struct RouteTableStore {
    current: RwLock<Arc<RouteTable>>,
}

impl RouteTableStore {
    pub fn new(initial: RouteTable) -> Self {
        Self {
            current: RwLock::new(Arc::new(initial)),
        }
    }

    pub fn snapshot(&self) -> Arc<RouteTable> {
        Arc::clone(&self.read())
    }

    pub fn replace(&self, routes: Vec<Route>) -> Result<Arc<RouteTable>, RouteTableError> {
        let mut current = self.write();
        let generation = current
            .generation()
            .checked_add(1)
            .ok_or(RouteTableError::GenerationExhausted)?;
        let next = Arc::new(RouteTable::new(generation, routes)?);
        *current = Arc::clone(&next);
        Ok(next)
    }

    fn read(&self) -> RwLockReadGuard<'_, Arc<RouteTable>> {
        self.current
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, Arc<RouteTable>> {
        self.current
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    fn id(value: &str) -> RouteId {
        RouteId::new(value).unwrap()
    }

    fn single_route(name: &str, weight: u64) -> Vec<Route> {
        vec![Route::new(id(name), weight)]
    }

    #[test]
    fn rejects_structurally_invalid_tables() {
        assert_eq!(
            RouteTable::new(0, Vec::new()).unwrap_err(),
            RouteTableError::EmptyRouteTable
        );
        assert_eq!(RouteId::new("").unwrap_err(), RouteTableError::EmptyRouteId);
        assert_eq!(
            RouteTable::new(0, vec![Route::new(id("a"), 1), Route::new(id("a"), 2)]).unwrap_err(),
            RouteTableError::DuplicateRouteId
        );
        assert_eq!(
            RouteTable::new(
                0,
                vec![Route::new(id("a"), 0), Route::new(id("b"), 9).disabled()]
            )
            .unwrap_err(),
            RouteTableError::NoWeightedEligibleRoute
        );
        assert_eq!(
            RouteTable::new(
                0,
                vec![Route::new(id("a"), u64::MAX), Route::new(id("b"), 1)],
            )
            .unwrap_err(),
            RouteTableError::WeightOverflow
        );
    }

    #[test]
    fn deterministic_draws_cover_exact_weight_boundaries() {
        let table = RouteTable::new(
            42,
            vec![
                Route::new(id("zero"), 0),
                Route::new(id("a"), 2),
                Route::new(id("disabled"), 100).disabled(),
                Route::new(id("b"), 3),
            ],
        )
        .unwrap();

        let expected = ["a", "a", "b", "b", "b"];
        for (draw, expected_id) in expected.into_iter().enumerate() {
            let selected = table.select_with(|range| {
                assert_eq!(range, 0..5);
                draw as u64
            });
            assert_eq!(selected.unwrap().id().as_str(), expected_id);
        }

        assert_eq!(table.generation(), 42);
        assert_eq!(table.eligible_weight(), 5);
    }

    #[test]
    fn rejects_an_invalid_deterministic_source_without_retrying() {
        let table = RouteTable::new(7, vec![Route::new(id("a"), 1)]).unwrap();
        let mut calls = 0;
        let result = table.select_with(|_| {
            calls += 1;
            1
        });
        assert_eq!(result.unwrap_err(), RouteTableError::DrawOutOfRange);
        assert_eq!(calls, 1);
    }

    #[test]
    fn equal_relative_weights_have_equal_deterministic_distribution() {
        let one_one =
            RouteTable::new(1, vec![Route::new(id("a"), 1), Route::new(id("b"), 1)]).unwrap();
        let fifty_fifty =
            RouteTable::new(1, vec![Route::new(id("a"), 50), Route::new(id("b"), 50)]).unwrap();

        let samples = 10_000_u64;
        let one_one_a = (0..samples)
            .filter(|sample| {
                one_one
                    .select_with(|_| sample % one_one.eligible_weight())
                    .unwrap()
                    .id()
                    .as_str()
                    == "a"
            })
            .count();
        let fifty_fifty_a = (0..samples)
            .filter(|sample| {
                fifty_fifty
                    .select_with(|_| sample % fifty_fifty.eligible_weight())
                    .unwrap()
                    .id()
                    .as_str()
                    == "a"
            })
            .count();

        assert_eq!(one_one_a, samples as usize / 2);
        assert_eq!(fifty_fifty_a, samples as usize / 2);
    }

    #[test]
    fn replacement_increments_generation_and_preserves_old_snapshot() {
        let store = RouteTableStore::new(RouteTable::new(9, single_route("old", 1)).unwrap());
        let old = store.snapshot();
        let new = store.replace(single_route("new", 2)).unwrap();

        assert_eq!(old.generation(), 9);
        assert_eq!(old.routes()[0].id().as_str(), "old");
        assert_eq!(new.generation(), 10);
        assert_eq!(new.routes()[0].id().as_str(), "new");
        assert!(Arc::ptr_eq(&new, &store.snapshot()));
    }

    #[test]
    fn invalid_replacement_leaves_current_snapshot_unchanged() {
        let store = RouteTableStore::new(RouteTable::new(3, single_route("stable", 1)).unwrap());
        let before = store.snapshot();

        assert_eq!(
            store.replace(Vec::new()).unwrap_err(),
            RouteTableError::EmptyRouteTable
        );

        let after = store.snapshot();
        assert!(Arc::ptr_eq(&before, &after));
        assert_eq!(after.generation(), 3);
    }

    #[test]
    fn replacement_rejects_generation_exhaustion_without_publication() {
        let store = RouteTableStore::new(
            RouteTable::new(u64::MAX, single_route("last", 1)).unwrap(),
        );
        let before = store.snapshot();

        assert_eq!(
            store.replace(single_route("never", 1)).unwrap_err(),
            RouteTableError::GenerationExhausted
        );
        assert!(Arc::ptr_eq(&before, &store.snapshot()));
    }

    #[test]
    fn concurrent_writers_serialize_generation_updates() {
        const WRITERS: usize = 4;
        const REPLACEMENTS: usize = 50;

        let store = Arc::new(RouteTableStore::new(
            RouteTable::new(0, single_route("initial", 1)).unwrap(),
        ));
        let start = Arc::new(Barrier::new(WRITERS));
        let handles: Vec<_> = (0..WRITERS)
            .map(|writer| {
                let store = Arc::clone(&store);
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    for replacement in 0..REPLACEMENTS {
                        let name = format!("writer-{writer}-{replacement}");
                        store.replace(single_route(&name, 1)).unwrap();
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(
            store.snapshot().generation(),
            (WRITERS * REPLACEMENTS) as u64
        );
    }

    #[test]
    fn concurrent_readers_observe_only_complete_snapshots() {
        const READERS: usize = 8;
        const REPLACEMENTS: usize = 200;

        let initial = RouteTable::new(
            0,
            vec![Route::new(id("left"), 1), Route::new(id("right"), 1)],
        )
        .unwrap();
        let store = Arc::new(RouteTableStore::new(initial));
        let start = Arc::new(Barrier::new(READERS + 1));

        let readers: Vec<_> = (0..READERS)
            .map(|_| {
                let store = Arc::clone(&store);
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    for _ in 0..(REPLACEMENTS * 4) {
                        let snapshot = store.snapshot();
                        let routes = snapshot.routes();
                        assert_eq!(routes.len(), 2);
                        let expected = snapshot.generation() + 1;
                        assert_eq!(routes[0].weight(), expected);
                        assert_eq!(routes[1].weight(), expected);
                        std::thread::yield_now();
                    }
                })
            })
            .collect();

        start.wait();
        for replacement in 1..=REPLACEMENTS {
            let weight = replacement as u64 + 1;
            store
                .replace(vec![
                    Route::new(id("left"), weight),
                    Route::new(id("right"), weight),
                ])
                .unwrap();
            std::thread::yield_now();
        }

        for reader in readers {
            reader.join().unwrap();
        }

        assert_eq!(store.snapshot().generation(), REPLACEMENTS as u64);
    }
}
