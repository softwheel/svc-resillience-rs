use std::num::NonZeroU32;

use softwheel_resilience::{
    PrimaryRetryBudget, RetryBudgetConfig, RetryBudgetDecision, ShadowRetryBudget,
};

fn config(capacity: u32, replenish_per_success: u32) -> RetryBudgetConfig {
    RetryBudgetConfig::new(
        NonZeroU32::new(capacity).unwrap(),
        NonZeroU32::new(replenish_per_success).unwrap(),
    )
}

#[test]
fn bounded_retry_budget_accounting_matches_reference_model() {
    for capacity in 1..=16_u32 {
        for replenish in 1..=8_u32 {
            let budget = PrimaryRetryBudget::new(config(capacity, replenish));
            let mut available = capacity;

            for step in 0..64_u32 {
                if step % 3 == 2 {
                    available = available.saturating_add(replenish).min(capacity);
                    assert_eq!(budget.record_success(), available);
                } else if available == 0 {
                    assert_eq!(budget.try_acquire_retry(), RetryBudgetDecision::Suppressed);
                } else {
                    available -= 1;
                    assert_eq!(
                        budget.try_acquire_retry(),
                        RetryBudgetDecision::Admitted {
                            remaining: available
                        }
                    );
                }

                assert_eq!(budget.available_retries(), available);
                assert!(available <= capacity);
            }
        }
    }
}

#[test]
fn replenishment_saturates_for_all_bounded_capacities_and_rates() {
    for capacity in 1..=32_u32 {
        for replenish in 1..=32_u32 {
            let budget = PrimaryRetryBudget::new(config(capacity, replenish));

            for _ in 0..capacity {
                assert!(matches!(
                    budget.try_acquire_retry(),
                    RetryBudgetDecision::Admitted { .. }
                ));
            }
            assert_eq!(budget.available_retries(), 0);

            let expected = replenish.min(capacity);
            assert_eq!(budget.record_success(), expected);
            assert_eq!(budget.available_retries(), expected);

            for _ in 0..capacity {
                budget.record_success();
            }
            assert_eq!(budget.available_retries(), capacity);
        }
    }
}

#[test]
fn cloned_handles_preserve_single_shared_budget_model() {
    for capacity in 1..=16_u32 {
        let budget = PrimaryRetryBudget::new(config(capacity, 1));
        let clone = budget.clone();

        for expected_remaining in (0..capacity).rev() {
            assert_eq!(
                clone.try_acquire_retry(),
                RetryBudgetDecision::Admitted {
                    remaining: expected_remaining,
                }
            );
            assert_eq!(budget.available_retries(), expected_remaining);
        }

        assert_eq!(budget.try_acquire_retry(), RetryBudgetDecision::Suppressed);
        assert_eq!(clone.record_success(), 1);
        assert_eq!(budget.available_retries(), 1);
    }
}

#[test]
fn primary_and_shadow_budget_state_remain_strictly_isolated() {
    for capacity in 1..=16_u32 {
        let primary = PrimaryRetryBudget::new(config(capacity, 1));
        let shadow = ShadowRetryBudget::new(config(capacity, 1));

        for expected_remaining in (0..capacity).rev() {
            assert_eq!(
                shadow.try_acquire_retry(),
                RetryBudgetDecision::Admitted {
                    remaining: expected_remaining,
                }
            );
            assert_eq!(primary.available_retries(), capacity);
        }

        assert_eq!(shadow.try_acquire_retry(), RetryBudgetDecision::Suppressed);
        assert_eq!(primary.available_retries(), capacity);

        shadow.record_success();
        assert_eq!(shadow.available_retries(), 1);
        assert_eq!(primary.available_retries(), capacity);

        assert_eq!(
            primary.try_acquire_retry(),
            RetryBudgetDecision::Admitted {
                remaining: capacity - 1,
            }
        );
        assert_eq!(shadow.available_retries(), 1);
    }
}
