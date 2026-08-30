use std::num::NonZeroUsize;

use softwheel_resilience::{Bulkhead, BulkheadCallError, BulkheadRejected};

#[test]
fn bounded_capacity_accounting_is_conservative() {
    for capacity in 1..=32 {
        let bulkhead = Bulkhead::new(NonZeroUsize::new(capacity).unwrap());
        let mut permits = Vec::with_capacity(capacity);

        for admitted in 0..capacity {
            assert_eq!(bulkhead.in_flight(), admitted);
            assert_eq!(bulkhead.available(), capacity - admitted);

            permits.push(bulkhead.try_acquire().unwrap());

            assert_eq!(bulkhead.in_flight(), admitted + 1);
            assert_eq!(bulkhead.available(), capacity - admitted - 1);
            assert_eq!(bulkhead.in_flight() + bulkhead.available(), capacity);
        }

        assert_eq!(bulkhead.try_acquire().unwrap_err(), BulkheadRejected);
        assert_eq!(bulkhead.in_flight(), capacity);
        assert_eq!(bulkhead.available(), 0);

        while let Some(permit) = permits.pop() {
            let before = bulkhead.in_flight();
            drop(permit);
            assert_eq!(bulkhead.in_flight(), before - 1);
            assert_eq!(bulkhead.in_flight() + bulkhead.available(), capacity);
        }

        assert_eq!(bulkhead.in_flight(), 0);
        assert_eq!(bulkhead.available(), capacity);
    }
}

#[test]
fn explicit_release_restores_exactly_one_slot() {
    for capacity in 1..=32 {
        let bulkhead = Bulkhead::new(NonZeroUsize::new(capacity).unwrap());
        let permit = bulkhead.try_acquire().unwrap();

        assert_eq!(bulkhead.in_flight(), 1);
        permit.release();
        assert_eq!(bulkhead.in_flight(), 0);
        assert_eq!(bulkhead.available(), capacity);

        let _replacement = bulkhead.try_acquire().unwrap();
        assert_eq!(bulkhead.in_flight(), 1);
    }
}

#[test]
fn call_releases_capacity_for_success_and_inner_error() {
    for capacity in 1..=32 {
        let bulkhead = Bulkhead::new(NonZeroUsize::new(capacity).unwrap());

        let success = bulkhead.call::<_, &'static str, _>(|| Ok(42));
        assert_eq!(success.unwrap(), 42);
        assert_eq!(bulkhead.in_flight(), 0);
        assert_eq!(bulkhead.available(), capacity);

        let failure = bulkhead.call::<(), _, _>(|| Err("boom"));
        assert!(matches!(failure, Err(BulkheadCallError::Inner("boom"))));
        assert_eq!(bulkhead.in_flight(), 0);
        assert_eq!(bulkhead.available(), capacity);
    }
}

#[test]
fn saturated_call_rejects_without_invoking_inner_operation() {
    for capacity in 1..=16 {
        let bulkhead = Bulkhead::new(NonZeroUsize::new(capacity).unwrap());
        let permits: Vec<_> = (0..capacity)
            .map(|_| bulkhead.try_acquire().unwrap())
            .collect();
        let mut invoked = false;

        let result = bulkhead.call::<(), (), _>(|| {
            invoked = true;
            Ok(())
        });

        assert!(matches!(result, Err(BulkheadCallError::Rejected(_))));
        assert!(!invoked);
        assert_eq!(bulkhead.in_flight(), capacity);
        assert_eq!(bulkhead.available(), 0);

        drop(permits);
        assert_eq!(bulkhead.in_flight(), 0);
        assert_eq!(bulkhead.available(), capacity);
    }
}
