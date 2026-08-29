use std::fmt;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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
        let mut current = self.inner.in_flight.load(Ordering::Acquire);
        loop {
            if current >= self.inner.capacity {
                return Err(BulkheadRejected);
            }

            match self.inner.in_flight.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(BulkheadPermit {
                        inner: Arc::clone(&self.inner),
                        released: false,
                    })
                }
                Err(observed) => current = observed,
            }
        }
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
            self.inner.in_flight.fetch_sub(1, Ordering::AcqRel);
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
}
