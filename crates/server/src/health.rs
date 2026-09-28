use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use domain::Slot;
use market::{MarketReader, PoolCounts};
use serde::Serialize;
use tokio::sync::watch;

use crate::settings::ReadySettings;

enum Phase {
    Starting,
    Serving(MarketReader),
    Draining,
    Stopping,
}

#[derive(Clone)]
pub struct Health {
    inner: Arc<Inner>,
}

struct Inner {
    phase: watch::Sender<Phase>,
    opened: AtomicBool,
    settings: ReadySettings,
}

impl Health {
    #[must_use]
    pub fn new(settings: ReadySettings) -> Self {
        Self {
            inner: Arc::new(Inner {
                phase: watch::Sender::new(Phase::Starting),
                opened: AtomicBool::new(false),
                settings,
            }),
        }
    }

    pub fn serving(&self, reader: MarketReader) {
        self.inner.phase.send_replace(Phase::Serving(reader));
    }

    /// `/ready` answers 503 from now on; the servers keep serving until
    /// [`Health::stop`], so a load balancer can move traffic away first.
    pub fn drain(&self) {
        self.inner.phase.send_replace(Phase::Draining);
    }

    pub fn stop(&self) {
        self.inner.phase.send_replace(Phase::Stopping);
    }

    pub(crate) async fn stopping(&self) {
        let mut phase = self.inner.phase.subscribe();
        // The sender lives in `inner`, so this only returns on `Stopping`.
        let _ = phase.wait_for(|p| matches!(p, Phase::Stopping)).await;
    }

    pub(crate) fn report(&self) -> ReadyReport {
        let settings = &self.inner.settings;
        let reader = match &*self.inner.phase.borrow() {
            Phase::Starting => return assess(&Observed::Starting, settings),
            Phase::Draining => return assess(&Observed::Draining, settings),
            Phase::Stopping => return assess(&Observed::Stopping, settings),
            Phase::Serving(reader) => reader.clone(),
        };
        let clock = reader.clock().zip(reader.clock_advanced_at());
        let sample = EngineSample {
            pools: reader.pool_counts(),
            clock: clock.map(|(clock, at)| (clock.slot, at.elapsed())),
            opened: self.inner.opened.load(Ordering::Acquire),
        };
        let report = assess(&Observed::Serving(sample), settings);
        if report.ready {
            self.inner.opened.store(true, Ordering::Release);
        }
        report
    }
}

pub(crate) enum Observed {
    Starting,
    Serving(EngineSample),
    Draining,
    Stopping,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EngineSample {
    pub pools: PoolCounts,
    /// The newest slot and how long ago it arrived.
    pub clock: Option<(Slot, Duration)>,
    /// Ready once already: the pool share only has to stay above the floor.
    pub opened: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReadyReport {
    pub ready: bool,
    pub phase: Stage,
    pub reasons: Vec<NotReady>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Stage {
    Starting,
    Serving,
    Draining,
    Stopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum NotReady {
    Starting,
    Draining,
    NoClock,
    ClockStalled,
    TooFewReadyPools,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EngineReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot_age_ms: Option<u64>,
    pub ready_pools: usize,
    pub eligible_pools: usize,
    pub total_pools: usize,
}

pub(crate) fn assess(observed: &Observed, settings: &ReadySettings) -> ReadyReport {
    let (phase, reasons, engine) = match observed {
        Observed::Starting => (Stage::Starting, vec![NotReady::Starting], None),
        Observed::Draining => (Stage::Draining, vec![NotReady::Draining], None),
        Observed::Stopping => (Stage::Stopping, vec![NotReady::Draining], None),
        Observed::Serving(sample) => {
            let mut reasons = Vec::new();
            match sample.clock {
                None => reasons.push(NotReady::NoClock),
                Some((_, age)) if age > settings.max_clock_stall() => {
                    reasons.push(NotReady::ClockStalled);
                }
                Some(_) => {}
            }
            let percent = if sample.opened {
                settings.floor_percent
            } else {
                settings.startup_percent
            };
            if !enough_pools(sample.pools, percent) {
                reasons.push(NotReady::TooFewReadyPools);
            }
            let engine = EngineReport {
                slot: sample.clock.map(|(slot, _)| slot.0),
                slot_age_ms: sample
                    .clock
                    .map(|(_, age)| u64::try_from(age.as_millis()).unwrap_or(u64::MAX)),
                ready_pools: sample.pools.ready,
                eligible_pools: sample.pools.eligible,
                total_pools: sample.pools.total,
            };
            (Stage::Serving, reasons, Some(engine))
        }
    };
    ReadyReport {
        ready: reasons.is_empty(),
        phase,
        reasons,
        engine,
    }
}

fn enough_pools(pools: PoolCounts, percent: u8) -> bool {
    pools.ready > 0
        && pools.ready.saturating_mul(100) >= pools.eligible.saturating_mul(usize::from(percent))
}
