use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

/// Spaces request starts evenly at `max_rps`, shared by every method, so a
/// burst of repairs cannot push a free-tier key over its quota.
pub(crate) struct RateLimiter {
    interval: Option<Duration>,
    next: Mutex<Option<Instant>>,
}

impl RateLimiter {
    pub(crate) fn new(max_rps: u32) -> Self {
        Self {
            interval: (max_rps > 0).then(|| Duration::from_secs(1) / max_rps),
            next: Mutex::new(None),
        }
    }

    pub(crate) async fn wait(&self) {
        let Some(interval) = self.interval else {
            return;
        };
        let start = {
            let mut next = self.next.lock().await;
            reserve(&mut next, Instant::now(), interval)
        };
        tokio::time::sleep_until(start).await;
    }
}

fn reserve(next: &mut Option<Instant>, now: Instant, interval: Duration) -> Instant {
    let start = next.map_or(now, |n| n.max(now));
    *next = Some(start + interval);
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: Duration = Duration::from_millis(125);

    #[test]
    fn back_to_back_requests_are_spaced_by_the_interval() {
        let now = Instant::now();
        let mut next = None;
        let starts: Vec<Duration> = (0..3)
            .map(|_| reserve(&mut next, now, INTERVAL) - now)
            .collect();
        assert_eq!(starts, [Duration::ZERO, INTERVAL, INTERVAL * 2]);
    }

    #[test]
    fn an_idle_limiter_does_not_bank_credit() {
        let now = Instant::now();
        let mut next = None;
        reserve(&mut next, now, INTERVAL);
        let later = now + Duration::from_secs(10);
        assert_eq!(reserve(&mut next, later, INTERVAL), later);
    }
}
