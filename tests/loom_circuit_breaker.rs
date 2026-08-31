use loom::sync::atomic::{AtomicBool, Ordering};
use loom::sync::{Arc, Mutex};
use loom::thread;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug)]
struct Shared {
    generation: u64,
    mode: Mode,
    half_open_in_flight: u32,
    half_open_successes: u32,
}

impl Shared {
    fn half_open() -> Self {
        Self {
            generation: 1,
            mode: Mode::HalfOpen,
            half_open_in_flight: 0,
            half_open_successes: 0,
        }
    }

    fn transition_open(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.mode = Mode::Open;
        self.half_open_in_flight = 0;
        self.half_open_successes = 0;
    }

    fn transition_closed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.mode = Mode::Closed;
        self.half_open_in_flight = 0;
        self.half_open_successes = 0;
    }
}

#[test]
#[ignore = "exhaustive Loom model; run in the dedicated CI job"]
fn loom_models_breaker_stale_generation_outcome() {
    loom::model(|| {
        let shared = Arc::new(Mutex::new(Shared::half_open()));
        let transitioned = Arc::new(AtomicBool::new(false));

        let stale = {
            let mut guard = shared.lock().unwrap();
            guard.half_open_in_flight += 1;
            guard.generation
        };

        let transition = Arc::clone(&shared);
        let transition_done = Arc::clone(&transitioned);
        let transition_handle = thread::spawn(move || {
            let mut guard = transition.lock().unwrap();
            guard.transition_open();
            drop(guard);
            transition_done.store(true, Ordering::Release);
        });

        let completion = Arc::clone(&shared);
        let completion_ready = Arc::clone(&transitioned);
        let completion_handle = thread::spawn(move || {
            while !completion_ready.load(Ordering::Acquire) {
                thread::yield_now();
            }

            let mut guard = completion.lock().unwrap();
            if guard.generation == stale && guard.mode == Mode::HalfOpen {
                guard.half_open_in_flight = guard.half_open_in_flight.saturating_sub(1);
                guard.half_open_successes += 1;
                guard.transition_closed();
            }
        });

        transition_handle.join().unwrap();
        completion_handle.join().unwrap();

        let guard = shared.lock().unwrap();
        assert_eq!(guard.mode, Mode::Open);
        assert_eq!(guard.generation, stale.wrapping_add(1));
        assert_eq!(guard.half_open_in_flight, 0);
        assert_eq!(guard.half_open_successes, 0);
    });
}

#[test]
#[ignore = "exhaustive Loom model; run in the dedicated CI job"]
fn loom_models_breaker_half_open_probe_budget_and_release() {
    loom::model(|| {
        const PROBE_BUDGET: u32 = 1;

        let shared = Arc::new(Mutex::new(Shared::half_open()));
        let first = Arc::clone(&shared);
        let second = Arc::clone(&shared);

        let first_handle = thread::spawn(move || {
            let admitted = {
                let mut guard = first.lock().unwrap();
                if guard.mode == Mode::HalfOpen
                    && guard.half_open_in_flight < PROBE_BUDGET
                {
                    guard.half_open_in_flight += 1;
                    assert!(guard.half_open_in_flight <= PROBE_BUDGET);
                    true
                } else {
                    false
                }
            };

            if admitted {
                thread::yield_now();
                let mut guard = first.lock().unwrap();
                guard.half_open_in_flight = guard.half_open_in_flight.saturating_sub(1);
            }
        });

        let second_handle = thread::spawn(move || {
            let admitted = {
                let mut guard = second.lock().unwrap();
                if guard.mode == Mode::HalfOpen
                    && guard.half_open_in_flight < PROBE_BUDGET
                {
                    guard.half_open_in_flight += 1;
                    assert!(guard.half_open_in_flight <= PROBE_BUDGET);
                    true
                } else {
                    false
                }
            };

            if admitted {
                thread::yield_now();
                let mut guard = second.lock().unwrap();
                guard.half_open_in_flight = guard.half_open_in_flight.saturating_sub(1);
            }
        });

        first_handle.join().unwrap();
        second_handle.join().unwrap();

        let guard = shared.lock().unwrap();
        assert_eq!(guard.mode, Mode::HalfOpen);
        assert_eq!(guard.half_open_in_flight, 0);
        assert!(guard.half_open_successes <= PROBE_BUDGET);
    });
}
