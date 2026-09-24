use std::time::Duration;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base_delay_ms: 100,
            max_delay_ms: 5_000,
        }
    }
}

impl RetryPolicy {
    #[must_use]
    pub fn ceiling(&self, attempt: u32) -> Duration {
        let exp = attempt.saturating_sub(1).min(32);
        let ms = self
            .base_delay_ms
            .saturating_mul(1u64 << exp)
            .min(self.max_delay_ms);
        Duration::from_millis(ms)
    }

    /// Half fixed, half random: keeps a floor so retries never hammer the
    /// endpoint, while the random half spreads out simultaneous reconnects.
    #[must_use]
    pub fn delay(&self, attempt: u32) -> Duration {
        let ceiling = u64::try_from(self.ceiling(attempt).as_millis()).unwrap_or(u64::MAX);
        let half = ceiling / 2;
        Duration::from_millis(half + rand::random_range(0..=ceiling - half))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: RetryPolicy = RetryPolicy {
        max_attempts: 10,
        base_delay_ms: 100,
        max_delay_ms: 1_000,
    };

    #[test]
    fn ceiling_doubles_per_attempt() {
        assert_eq!(POLICY.ceiling(3), Duration::from_millis(400));
    }

    #[test]
    fn ceiling_is_capped() {
        assert_eq!(POLICY.ceiling(30), Duration::from_millis(1_000));
    }

    #[test]
    fn delay_stays_within_half_and_full_ceiling() {
        for attempt in 1..=10 {
            let delay = POLICY.delay(attempt);
            let ceiling = POLICY.ceiling(attempt);
            assert!(delay >= ceiling / 2 && delay <= ceiling);
        }
    }
}
