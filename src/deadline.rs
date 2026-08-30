use std::time::Duration;

/// Runtime-agnostic budget for one logical request.
///
/// The caller supplies elapsed time from a monotonic clock. The core never reads a clock and never
/// sleeps, which keeps deadline semantics deterministic and independent from an async runtime.
///
/// A bounded budget is an outer limit shared by attempts, retry sleeps, route failover, and adapter
/// timeout wrappers. Child work must clamp its own timeout to [`LogicalRequestBudget::remaining`].
///
/// ```
/// use std::time::Duration;
/// use softwheel_resilience::LogicalRequestBudget;
///
/// let budget = LogicalRequestBudget::bounded(Duration::from_millis(100));
/// assert_eq!(
///     budget.remaining(Duration::from_millis(25)),
///     Some(Duration::from_millis(75))
/// );
/// assert_eq!(
///     budget.clamp_child(Duration::from_millis(25), Duration::from_millis(90)),
///     Duration::from_millis(75)
/// );
/// assert!(budget.can_wait(Duration::from_millis(25), Duration::from_millis(50)));
/// assert!(!budget.can_wait(Duration::from_millis(25), Duration::from_millis(75)));
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalRequestBudget {
    limit: Option<Duration>,
}

impl LogicalRequestBudget {
    /// Create an unbounded logical-request budget.
    pub const fn unlimited() -> Self {
        Self { limit: None }
    }

    /// Create a bounded logical-request budget.
    ///
    /// A zero duration is valid and represents an already-exhausted request. Keeping this state
    /// representable makes boundary handling explicit instead of silently extending the deadline.
    pub const fn bounded(limit: Duration) -> Self {
        Self { limit: Some(limit) }
    }

    /// Return the configured outer limit, or `None` for an unbounded request.
    pub const fn limit(&self) -> Option<Duration> {
        self.limit
    }

    /// Return the remaining budget after caller-supplied elapsed time.
    ///
    /// Bounded budgets saturate at zero, so advancing elapsed time can never increase the result.
    /// `None` denotes an unbounded request.
    pub fn remaining(&self, elapsed: Duration) -> Option<Duration> {
        self.limit.map(|limit| limit.saturating_sub(elapsed))
    }

    /// Return whether the logical request has exhausted its outer budget.
    pub fn is_exhausted(&self, elapsed: Duration) -> bool {
        self.limit.is_some_and(|limit| elapsed >= limit)
    }

    /// Clamp a child timeout or deadline budget to the parent's remaining time.
    ///
    /// This is the core rule adapters use to ensure a physical attempt, failover step, shadow
    /// execution, or runtime timeout cannot outlive the logical request.
    pub fn clamp_child(&self, elapsed: Duration, requested: Duration) -> Duration {
        match self.remaining(elapsed) {
            Some(remaining) => requested.min(remaining),
            None => requested,
        }
    }

    /// Return whether a blocking delay leaves positive budget for work afterwards.
    ///
    /// Equality is rejected deliberately: sleeping for exactly the remaining duration reaches the
    /// outer deadline, so another physical attempt must not begin after that sleep.
    pub fn can_wait(&self, elapsed: Duration, delay: Duration) -> bool {
        match self.remaining(elapsed) {
            Some(remaining) => !remaining.is_zero() && delay < remaining,
            None => true,
        }
    }
}

impl Default for LogicalRequestBudget {
    fn default() -> Self {
        Self::unlimited()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_remaining_saturates_and_never_increases() {
        let budget = LogicalRequestBudget::bounded(Duration::from_millis(100));

        let samples = [0, 25, 99, 100, 125].map(|elapsed| {
            budget
                .remaining(Duration::from_millis(elapsed))
                .expect("bounded budget")
        });

        assert_eq!(
            samples,
            [
                Duration::from_millis(100),
                Duration::from_millis(75),
                Duration::from_millis(1),
                Duration::ZERO,
                Duration::ZERO,
            ]
        );
        assert!(samples.windows(2).all(|pair| pair[1] <= pair[0]));
    }

    #[test]
    fn zero_budget_is_explicitly_exhausted() {
        let budget = LogicalRequestBudget::bounded(Duration::ZERO);

        assert!(budget.is_exhausted(Duration::ZERO));
        assert_eq!(budget.remaining(Duration::ZERO), Some(Duration::ZERO));
        assert!(!budget.can_wait(Duration::ZERO, Duration::ZERO));
    }

    #[test]
    fn deadline_boundary_is_exhausted() {
        let budget = LogicalRequestBudget::bounded(Duration::from_millis(10));

        assert!(!budget.is_exhausted(Duration::from_millis(9)));
        assert!(budget.is_exhausted(Duration::from_millis(10)));
        assert!(budget.is_exhausted(Duration::from_millis(11)));
    }

    #[test]
    fn child_budget_never_exceeds_parent_remaining() {
        let budget = LogicalRequestBudget::bounded(Duration::from_millis(100));

        assert_eq!(
            budget.clamp_child(Duration::from_millis(60), Duration::from_millis(10)),
            Duration::from_millis(10)
        );
        assert_eq!(
            budget.clamp_child(Duration::from_millis(60), Duration::from_millis(80)),
            Duration::from_millis(40)
        );
        assert_eq!(
            budget.clamp_child(Duration::from_millis(120), Duration::from_millis(80)),
            Duration::ZERO
        );
    }

    #[test]
    fn wait_must_leave_positive_budget_for_following_work() {
        let budget = LogicalRequestBudget::bounded(Duration::from_millis(100));

        assert!(budget.can_wait(Duration::from_millis(60), Duration::from_millis(39)));
        assert!(!budget.can_wait(Duration::from_millis(60), Duration::from_millis(40)));
        assert!(!budget.can_wait(Duration::from_millis(100), Duration::ZERO));
    }

    #[test]
    fn unlimited_budget_never_exhausts_or_clamps_children() {
        let budget = LogicalRequestBudget::unlimited();
        let requested = Duration::from_secs(60);

        assert_eq!(budget.limit(), None);
        assert_eq!(budget.remaining(Duration::MAX), None);
        assert!(!budget.is_exhausted(Duration::MAX));
        assert_eq!(budget.clamp_child(Duration::MAX, requested), requested);
        assert!(budget.can_wait(Duration::MAX, Duration::MAX));
    }
}
