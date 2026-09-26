use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

/// Spaces request starts at `max_rps`, shared by every method, so a burst of
/// repairs cannot push a free-tier key over its quota. A slot is taken only
/// when the caller is actually awake to use it: callers that wake late after
/// a stall cannot start together.
pub(crate) struct RateLimiter {
    interval: Option<Duration>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    next: Option<Instant>,
    paused_until: Option<Instant>,
}

impl State {
    fn try_take(&mut self, now: Instant, interval: Option<Duration>) -> Result<(), Instant> {
        if let Some(at) = self.next.max(self.paused_until).filter(|at| *at > now) {
            return Err(at);
        }
        self.next = interval.map(|i| now + i);
        Ok(())
    }

    fn pause(&mut self, until: Instant) {
        self.paused_until = self.paused_until.max(Some(until));
    }
}

impl RateLimiter {
    pub(crate) fn new(max_rps: u32) -> Self {
        Self {
            interval: (max_rps > 0).then(|| Duration::from_secs(1) / max_rps),
            state: Mutex::default(),
        }
    }

    pub(crate) async fn wait(&self) {
        loop {
            let wake = {
                let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                match state.try_take(Instant::now(), self.interval) {
                    Ok(()) => return,
                    Err(at) => at,
                }
            };
            tokio::time::sleep_until(wake).await;
        }
    }

    pub(crate) fn pause_for(&self, delay: Duration) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pause(Instant::now() + delay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: Duration = Duration::from_millis(125);

    #[test]
    fn a_start_is_spaced_from_the_previous_one_even_after_a_stall() {
        let now = Instant::now();
        let mut state = State::default();
        state.try_take(now, Some(INTERVAL)).unwrap();
        let late = now + INTERVAL * 10;
        let starts = [
            state.try_take(late, Some(INTERVAL)),
            state.try_take(late, Some(INTERVAL)),
        ];
        assert_eq!(starts, [Ok(()), Err(late + INTERVAL)]);
    }

    #[test]
    fn an_idle_limiter_does_not_bank_credit() {
        let now = Instant::now();
        let mut state = State::default();
        state.try_take(now, Some(INTERVAL)).unwrap();
        let later = now + Duration::from_secs(10);
        state.try_take(later, Some(INTERVAL)).unwrap();
        assert_eq!(state.try_take(later, Some(INTERVAL)), Err(later + INTERVAL));
    }

    #[test]
    fn a_pause_holds_every_start_until_it_ends_even_without_pacing() {
        let now = Instant::now();
        let mut state = State::default();
        let until = now + Duration::from_secs(2);
        state.pause(until);
        assert_eq!(
            (state.try_take(now, None), state.try_take(until, None)),
            (Err(until), Ok(()))
        );
    }
}
