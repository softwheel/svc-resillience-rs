use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::{
    Bulkhead, CircuitBreaker, CircuitBreakerConfig, PrimaryRetryBudget, RetryBudgetConfig, RouteId,
    ShadowRetryBudget,
};

/// Construction policy for one execution namespace of a route.
///
/// Primary and shadow policies are supplied separately to [`RouteResourceRegistry::new`]. The
/// registry constructs independent breaker, bulkhead, and retry-budget state for each namespace,
/// making accidental primary/shadow state sharing impossible through the registry API.
#[derive(Clone, Debug)]
pub struct RouteResourcePolicy {
    circuit_breaker: CircuitBreakerConfig,
    bulkhead_capacity: NonZeroUsize,
    retry_budget: RetryBudgetConfig,
}

impl RouteResourcePolicy {
    pub const fn new(
        circuit_breaker: CircuitBreakerConfig,
        bulkhead_capacity: NonZeroUsize,
        retry_budget: RetryBudgetConfig,
    ) -> Self {
        Self {
            circuit_breaker,
            bulkhead_capacity,
            retry_budget,
        }
    }

    pub fn circuit_breaker(&self) -> &CircuitBreakerConfig {
        &self.circuit_breaker
    }

    pub const fn bulkhead_capacity(&self) -> NonZeroUsize {
        self.bulkhead_capacity
    }

    pub const fn retry_budget(&self) -> RetryBudgetConfig {
        self.retry_budget
    }
}

/// Primary execution resources associated with one stable route identity.
#[derive(Clone, Debug)]
pub struct PrimaryRouteResources {
    circuit_breaker: CircuitBreaker,
    bulkhead: Bulkhead,
    retry_budget: PrimaryRetryBudget,
}

impl PrimaryRouteResources {
    fn new(policy: &RouteResourcePolicy) -> Self {
        Self {
            circuit_breaker: CircuitBreaker::new(policy.circuit_breaker.clone()),
            bulkhead: Bulkhead::new(policy.bulkhead_capacity),
            retry_budget: PrimaryRetryBudget::new(policy.retry_budget),
        }
    }

    pub fn circuit_breaker(&self) -> &CircuitBreaker {
        &self.circuit_breaker
    }

    pub fn bulkhead(&self) -> &Bulkhead {
        &self.bulkhead
    }

    pub fn retry_budget(&self) -> &PrimaryRetryBudget {
        &self.retry_budget
    }
}

/// Shadow execution resources associated with one stable route identity.
///
/// This type deliberately exposes [`ShadowRetryBudget`] rather than [`PrimaryRetryBudget`]. It is
/// constructed independently from [`PrimaryRouteResources`], including independent breaker and
/// bulkhead state.
#[derive(Clone, Debug)]
pub struct ShadowRouteResources {
    circuit_breaker: CircuitBreaker,
    bulkhead: Bulkhead,
    retry_budget: ShadowRetryBudget,
}

impl ShadowRouteResources {
    fn new(policy: &RouteResourcePolicy) -> Self {
        Self {
            circuit_breaker: CircuitBreaker::new(policy.circuit_breaker.clone()),
            bulkhead: Bulkhead::new(policy.bulkhead_capacity),
            retry_budget: ShadowRetryBudget::new(policy.retry_budget),
        }
    }

    pub fn circuit_breaker(&self) -> &CircuitBreaker {
        &self.circuit_breaker
    }

    pub fn bulkhead(&self) -> &Bulkhead {
        &self.bulkhead
    }

    pub fn retry_budget(&self) -> &ShadowRetryBudget {
        &self.retry_budget
    }
}

/// Runtime resources for one route, split into strict primary and shadow namespaces.
#[derive(Clone, Debug)]
pub struct RouteResources {
    primary: PrimaryRouteResources,
    shadow: ShadowRouteResources,
}

impl RouteResources {
    fn new(primary: &RouteResourcePolicy, shadow: &RouteResourcePolicy) -> Self {
        Self {
            primary: PrimaryRouteResources::new(primary),
            shadow: ShadowRouteResources::new(shadow),
        }
    }

    pub fn primary(&self) -> &PrimaryRouteResources {
        &self.primary
    }

    pub fn shadow(&self) -> &ShadowRouteResources {
        &self.shadow
    }
}

/// Lazily-created runtime resources keyed by stable [`RouteId`].
///
/// Routing snapshots remain immutable and never own these mutable execution resources. Removing a
/// route from this registry only retires the registry's reference: any already-created
/// [`Arc<RouteResources>`] remains valid for an in-flight logical request. If the same route ID is
/// later requested again, a fresh resource set is created.
#[derive(Debug)]
pub struct RouteResourceRegistry {
    primary_policy: RouteResourcePolicy,
    shadow_policy: RouteResourcePolicy,
    routes: RwLock<HashMap<RouteId, Arc<RouteResources>>>,
}

impl RouteResourceRegistry {
    /// Create an empty registry with independent construction policy for primary and shadow work.
    pub fn new(primary_policy: RouteResourcePolicy, shadow_policy: RouteResourcePolicy) -> Self {
        Self {
            primary_policy,
            shadow_policy,
            routes: RwLock::new(HashMap::new()),
        }
    }

    /// Return existing resources for `route_id`, or atomically publish one newly-created set.
    ///
    /// Concurrent first lookups for the same route converge on one shared resource set.
    pub fn get_or_create(&self, route_id: &RouteId) -> Arc<RouteResources> {
        if let Some(resources) = self.read_routes().get(route_id).cloned() {
            return resources;
        }

        let mut routes = self.write_routes();
        routes
            .entry(route_id.clone())
            .or_insert_with(|| {
                Arc::new(RouteResources::new(
                    &self.primary_policy,
                    &self.shadow_policy,
                ))
            })
            .clone()
    }

    /// Return currently registered resources without creating them.
    pub fn get(&self, route_id: &RouteId) -> Option<Arc<RouteResources>> {
        self.read_routes().get(route_id).cloned()
    }

    /// Retire one route from future registry lookups.
    ///
    /// Existing `Arc` handles remain valid. This prevents route-table removal from invalidating an
    /// already-planned logical request that still needs to execute or finish accounting.
    pub fn retire(&self, route_id: &RouteId) -> Option<Arc<RouteResources>> {
        self.write_routes().remove(route_id)
    }

    pub fn len(&self) -> usize {
        self.read_routes().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn read_routes(&self) -> RwLockReadGuard<'_, HashMap<RouteId, Arc<RouteResources>>> {
        self.routes
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_routes(&self) -> RwLockWriteGuard<'_, HashMap<RouteId, Arc<RouteResources>>> {
        self.routes
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RetryBudgetDecision;
    use std::num::NonZeroU32;
    use std::sync::Barrier;
    use std::thread;
    use std::time::Duration;

    fn policy(bulkhead_capacity: usize, retry_capacity: u32) -> RouteResourcePolicy {
        RouteResourcePolicy::new(
            CircuitBreakerConfig::new(3, Duration::from_secs(1), 1).unwrap(),
            NonZeroUsize::new(bulkhead_capacity).unwrap(),
            RetryBudgetConfig::new(
                NonZeroU32::new(retry_capacity).unwrap(),
                NonZeroU32::new(1).unwrap(),
            ),
        )
    }

    fn route_id() -> RouteId {
        RouteId::new("route-a").unwrap()
    }

    #[test]
    fn repeated_lookup_shares_one_resource_set() {
        let registry = RouteResourceRegistry::new(policy(2, 2), policy(1, 1));
        let route = route_id();

        let first = registry.get_or_create(&route);
        let second = registry.get_or_create(&route);

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn primary_and_shadow_retry_budgets_are_independent() {
        let registry = RouteResourceRegistry::new(policy(2, 1), policy(2, 1));
        let resources = registry.get_or_create(&route_id());

        assert_eq!(
            resources.primary().retry_budget().try_acquire_retry(),
            RetryBudgetDecision::Admitted { remaining: 0 }
        );
        assert_eq!(
            resources.primary().retry_budget().try_acquire_retry(),
            RetryBudgetDecision::Suppressed
        );
        assert_eq!(
            resources.shadow().retry_budget().try_acquire_retry(),
            RetryBudgetDecision::Admitted { remaining: 0 }
        );
    }

    #[test]
    fn primary_and_shadow_bulkheads_are_independent() {
        let registry = RouteResourceRegistry::new(policy(1, 1), policy(1, 1));
        let resources = registry.get_or_create(&route_id());

        let primary_permit = resources.primary().bulkhead().try_acquire().unwrap();
        assert!(resources.primary().bulkhead().try_acquire().is_err());

        let shadow_permit = resources.shadow().bulkhead().try_acquire().unwrap();
        assert!(resources.shadow().bulkhead().try_acquire().is_err());

        drop(primary_permit);
        drop(shadow_permit);
    }

    #[test]
    fn retiring_keeps_in_flight_handle_alive_and_recreates_fresh_state() {
        let registry = RouteResourceRegistry::new(policy(1, 1), policy(1, 1));
        let route = route_id();
        let in_flight = registry.get_or_create(&route);

        let retired = registry.retire(&route).unwrap();
        assert!(Arc::ptr_eq(&in_flight, &retired));
        assert!(registry.get(&route).is_none());

        let replacement = registry.get_or_create(&route);
        assert!(!Arc::ptr_eq(&in_flight, &replacement));
    }

    #[test]
    fn concurrent_first_lookup_converges_on_one_resource_set() {
        let registry = Arc::new(RouteResourceRegistry::new(policy(2, 2), policy(2, 2)));
        let barrier = Arc::new(Barrier::new(8));
        let mut threads = Vec::new();

        for _ in 0..8 {
            let registry = Arc::clone(&registry);
            let barrier = Arc::clone(&barrier);
            threads.push(thread::spawn(move || {
                let route = route_id();
                barrier.wait();
                registry.get_or_create(&route)
            }));
        }

        let resources: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        let first = &resources[0];
        let all_shared = resources
            .iter()
            .all(|resources| Arc::ptr_eq(first, resources));
        assert!(all_shared);
        assert_eq!(registry.len(), 1);
    }
}
