use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

const SLOWEST: Duration = Duration::from_secs(1);
const RECOVERY_STEPS: u32 = 1_024;

/// Spaces request starts, shared by every method, so a burst of repairs
/// cannot push a free-tier key over its quota. A slot is taken only when the
/// caller is actually awake to use it: callers that wake late after a stall
/// cannot start together.
///
/// `max_rps` is a ceiling, not the provider's quota: each 429 widens the
/// spacing by a quarter and each success narrows it by a sliver, so the pace
/// settles just under what the provider actually allows.
pub(crate) struct RateLimiter {
    fastest: Option<Duration>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    interval: Option<Duration>,
    next: Option<Instant>,
    paused_until: Option<Instant>,
}

impl State {
    fn try_take(&mut self, now: Instant) -> Result<(), Instant> {
        if let Some(at) = self.next.max(self.paused_until).filter(|at| *at > now) {
            return Err(at);
        }
        self.next = self.interval.map(|i| now + i);
        Ok(())
    }

    fn throttle(&mut self, until: Instant) {
        self.paused_until = self.paused_until.max(Some(until));
        self.interval = self.interval.map(|i| (i * 5 / 4).min(SLOWEST));
    }

    fn recover(&mut self, fastest: Option<Duration>) {
        if let (Some(interval), Some(fastest)) = (self.interval, fastest) {
            self.interval = Some(
                interval
                    .saturating_sub(interval / RECOVERY_STEPS)
                    .max(fastest),
            );
        }
    }
}

impl RateLimiter {
    pub(crate) fn new(max_rps: u32) -> Self {
        let fastest = (max_rps > 0).then(|| Duration::from_secs(1) / max_rps);
        Self {
            fastest,
            state: Mutex::new(State {
                interval: fastest,
                ..State::default()
            }),
        }
    }

    pub(crate) async fn wait(&self) {
        loop {
            let wake = match self.lock().try_take(Instant::now()) {
                Ok(()) => return,
                Err(at) => at,
            };
            tokio::time::sleep_until(wake).await;
        }
    }

    pub(crate) fn throttle(&self, pause: Duration) {
        let mut state = self.lock();
        state.throttle(Instant::now() + pause);
        tracing::debug!(interval = ?state.interval, ?pause, "rpc rate limited, slowing down");
    }

    pub(crate) fn succeeded(&self) {
        self.lock().recover(self.fastest);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: Duration = Duration::from_millis(125);

    fn paced() -> State {
        State {
            interval: Some(INTERVAL),
            ..State::default()
        }
    }

    #[test]
    fn a_start_is_spaced_from_the_previous_one_even_after_a_stall() {
        let now = Instant::now();
        let mut state = paced();
        state.try_take(now).unwrap();
        let late = now + INTERVAL * 10;
        let starts = [state.try_take(late), state.try_take(late)];
        assert_eq!(starts, [Ok(()), Err(late + INTERVAL)]);
    }

    #[test]
    fn an_idle_limiter_does_not_bank_credit() {
        let now = Instant::now();
        let mut state = paced();
        state.try_take(now).unwrap();
        let later = now + Duration::from_secs(10);
        state.try_take(later).unwrap();
        assert_eq!(state.try_take(later), Err(later + INTERVAL));
    }

    #[test]
    fn a_pause_holds_every_start_until_it_ends_even_without_pacing() {
        let now = Instant::now();
        let mut state = State::default();
        let until = now + Duration::from_secs(2);
        state.throttle(until);
        assert_eq!(
            (state.try_take(now), state.try_take(until)),
            (Err(until), Ok(()))
        );
    }

    #[test]
    fn a_429_widens_the_spacing_and_successes_narrow_it_back_to_the_ceiling_only() {
        let mut state = paced();
        state.throttle(Instant::now());
        let slowed = state.interval;
        for _ in 0..10 * RECOVERY_STEPS {
            state.recover(Some(INTERVAL));
        }
        assert_eq!(
            (slowed, state.interval),
            (Some(INTERVAL * 5 / 4), Some(INTERVAL))
        );
    }
}
