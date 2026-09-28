use std::time::Duration;

use domain::Slot;
use market::PoolCounts;

use crate::ReadySettings;
use crate::health::{EngineSample, NotReady, Observed, assess};

const SETTINGS: ReadySettings = ReadySettings {
    startup_percent: 90,
    floor_percent: 50,
    max_clock_stall_ms: 1_000,
};

const FRESH: Option<Duration> = Some(Duration::from_millis(200));

fn serving(ready: usize, eligible: usize, clock: Option<Duration>, opened: bool) -> Observed {
    Observed::Serving(EngineSample {
        pools: PoolCounts {
            ready,
            eligible,
            total: eligible + 3,
        },
        clock: clock.map(|age| (Slot(100), age)),
        opened,
    })
}

#[test]
fn ready_reasons_follow_phase_clock_and_pool_share() {
    let cases: [(&str, Observed, &[NotReady]); 12] = [
        ("starting", Observed::Starting, &[NotReady::Starting]),
        ("draining", Observed::Draining, &[NotReady::Draining]),
        ("stopping", Observed::Stopping, &[NotReady::Draining]),
        ("startup at its share", serving(9, 10, FRESH, false), &[]),
        (
            "startup below its share",
            serving(8, 10, FRESH, false),
            &[NotReady::TooFewReadyPools],
        ),
        ("opened, at the floor", serving(5, 10, FRESH, true), &[]),
        (
            "opened, below the floor",
            serving(4, 10, FRESH, true),
            &[NotReady::TooFewReadyPools],
        ),
        (
            "no pool can become ready",
            serving(0, 0, FRESH, true),
            &[NotReady::TooFewReadyPools],
        ),
        (
            "stall exactly at the limit",
            serving(10, 10, Some(Duration::from_millis(1_000)), true),
            &[],
        ),
        (
            "stall past the limit",
            serving(10, 10, Some(Duration::from_millis(1_001)), true),
            &[NotReady::ClockStalled],
        ),
        (
            "no clock yet",
            serving(10, 10, None, false),
            &[NotReady::NoClock],
        ),
        (
            "every check failing",
            serving(1, 10, None, false),
            &[NotReady::NoClock, NotReady::TooFewReadyPools],
        ),
    ];
    for (name, observed, want) in cases {
        let report = assess(&observed, &SETTINGS);
        assert_eq!(report.reasons, want, "{name}");
        assert_eq!(report.ready, want.is_empty(), "{name}");
    }
}
