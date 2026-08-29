use std::fmt;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

trait AtomicCounter {
    fn load(&self, order: Ordering) -> usize;

    fn compare_exchange_weak(
        &self,
        current: usize,
        new: usize,
        success: Ordering,
        failure: Ordering,
    ) -> Result<usize, usize>;

    fn fetch_sub(&self, value: usize, order: Ordering) -> usize;
}

impl AtomicCounter for AtomicUsize {
    fn load(&self, order: Ordering) -> usize {
        AtomicUsize::load(self, order)
    }

    fn compare_exchange_weak(
        &self,
        current: usize,
        new: usize,
        success: Ordering,
        failure: Ordering,
    ) -> Result<usize, usize> {
        AtomicUsize::compare_exchange_weak(self, current, new, success, failure)
    }

    fn fetch_sub(&self, value: usize, order: Ordering) -> usize {
        AtomicUsize::fetch_sub(self, value, order)
    }
}

fn try_reserve<C: AtomicCounter>(in_flight: &C, capacity: usize) -> bool {
    let mut current = in_flight.load(Ordering::Acquire);
    loop {
        if current >= capacity {
            return false;
        }

        match in_flight.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

fn release_slot<C: AtomicCounter>(in_flight: &C) {
    let previous = in_flight.fetch_sub(1, Ordering::AcqRel);
    debug_assert!(previous > 0, "bulkhead permit released without a held slot");
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BulkheadRejected;

impl fmt::Display for BulkheadRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bulkhead rejected the call")
    }
}

impl std::error::Error for BulkheadRejected {}

#[derive(Debug)]
pub enum BulkheadCallError<E> {
    Rejected(BulkheadRejected),
    Inner(E),
}

impl<E: fmt::Display> fmt::Display for BulkheadCallError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(error) => error.fmt(f),
            Self::Inner(error) => error.fmt(f),
        }
    }
}

impl<E> std::error::Error for BulkheadCallError<E> where E: std::error::Error + 'static {}

#[derive(Debug)]
struct Inner {
    capacity: usize,
    in_flight: AtomicUsize,
}

/// Non-blocking concurrency bulkhead.
///
/// The bulkhead intentionally rejects excess work instead of queueing it. Queueing belongs at a
/// layer with an explicit latency/deadline budget; silently waiting here would turn overload into
/// unbounded tail latency.
#[derive(Clone, Debug)]
pub struct Bulkhead {
    inner: Arc<Inner>,
}

impl Bulkhead {
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self {
            inner: Arc::new(Inner {
                capacity: capacity.get(),
                in_flight: AtomicUsize::new(0),
            }),
        }
    }

    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }

    pub fn in_flight(&self) -> usize {
        self.inner.in_flight.load(Ordering::Acquire)
    }

    pub fn available(&self) -> usize {
        self.capacity().saturating_sub(self.in_flight())
    }

    pub fn try_acquire(&self) -> Result<BulkheadPermit, BulkheadRejected> {
        if !try_reserve(&self.inner.in_flight, self.inner.capacity) {
            return Err(BulkheadRejected);
        }

        Ok(BulkheadPermit {
            inner: Arc::clone(&self.inner),
            released: false,
        })
    }

    pub fn call<T, E, Operation>(&self, operation: Operation) -> Result<T, BulkheadCallError<E>>
    where
        Operation: FnOnce() -> Result<T, E>,
    {
        let _permit = self.try_acquire().map_err(BulkheadCallError::Rejected)?;
        operation().map_err(BulkheadCallError::Inner)
    }
}

#[derive(Debug)]
pub struct BulkheadPermit {
    inner: Arc<Inner>,
    released: bool,
}

impl BulkheadPermit {
    pub fn release(mut self) {
        self.release_inner();
    }

    fn release_inner(&mut self) {
        if !self.released {
            release_slot(&self.inner.in_flight);
            self.released = true;
        }
    }
}

impl Drop for BulkheadPermit {
    fn drop(&mut self) {
        self.release_inner();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    #[test]
    fn rejects_above_capacity_and_recovers_on_drop() {
        let bulkhead = Bulkhead::new(NonZeroUsize::new(1).unwrap());
        let permit = bulkhead.try_acquire().unwrap();
        assert_eq!(bulkhead.try_acquire().unwrap_err(), BulkheadRejected);
        drop(permit);
        assert!(bulkhead.try_acquire().is_ok());
    }

    #[test]
    fn concurrent_admission_never_exceeds_capacity() {
        const CAPACITY: usize = 4;
        const THREADS: usize = 32;
        const ITERATIONS: usize = 500;

        let bulkhead = Bulkhead::new(NonZeroUsize::new(CAPACITY).unwrap());
        let start = Arc::new(Barrier::new(THREADS));
        let peak = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let bulkhead = bulkhead.clone();
                let start = Arc::clone(&start);
                let peak = Arc::clone(&peak);

                std::thread::spawn(move || {
                    start.wait();

                    for _ in 0..ITERATIONS {
                        if let Ok(permit) = bulkhead.try_acquire() {
                            let observed = bulkhead.in_flight();
                            assert!(
                                observed <= CAPACITY,
                                "bulkhead admitted {observed} calls with capacity {CAPACITY}"
                            );
                            peak.fetch_max(observed, Ordering::AcqRel);
                            std::thread::yield_now();
                            drop(permit);
                        } else {
                            std::thread::yield_now();
                        }
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        assert!(peak.load(Ordering::Acquire) <= CAPACITY);
        assert_eq!(bulkhead.in_flight(), 0);
        assert_eq!(bulkhead.available(), CAPACITY);
    }

    impl AtomicCounter for loom::sync::atomic::AtomicUsize {
        fn load(&self, order: Ordering) -> usize {
            loom::sync::atomic::AtomicUsize::load(self, order)
        }

        fn compare_exchange_weak(
            &self,
            current: usize,
            new: usize,
            success: Ordering,
            failure: Ordering,
        ) -> Result<usize, usize> {
            loom::sync::atomic::AtomicUsize::compare_exchange_weak(
                self, current, new, success, failure,
            )
        }

        fn fetch_sub(&self, value: usize, order: Ordering) -> usize {
            loom::sync::atomic::AtomicUsize::fetch_sub(self, value, order)
        }
    }

    #[test]
    #[ignore = "exhaustive Loom model; run in the dedicated CI job"]
    fn loom_models_reservation_and_release() {
        loom::model(|| {
            use loom::sync::atomic::AtomicUsize as LoomAtomicUsize;
            use loom::sync::Arc as LoomArc;
            use loom::thread;

            let in_flight = LoomArc::new(LoomAtomicUsize::new(0));
            let first = LoomArc::clone(&in_flight);
            let second = LoomArc::clone(&in_flight);

            let first_handle = thread::spawn(move || {
                if try_reserve(&*first, 1) {
                    assert!(first.load(Ordering::Acquire) <= 1);
                    thread::yield_now();
                    release_slot(&*first);
                }
            });

            let second_handle = thread::spawn(move || {
                if try_reserve(&*second, 1) {
                    assert!(second.load(Ordering::Acquire) <= 1);
                    thread::yield_now();
                    release_slot(&*second);
                }
            });

            first_handle.join().unwrap();
            second_handle.join().unwrap();

            assert_eq!(in_flight.load(Ordering::Acquire), 0);
        });
    }
}
