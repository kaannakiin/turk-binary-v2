use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use domain::DexKind;
use quoter::QuoteError;

use crate::error::RouteError;
use crate::feed::PoolFeed;
use crate::reader::QuoteReader;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeTally {
    pub quoted: u64,
    pub refused: BTreeMap<&'static str, u64>,
}

#[derive(Debug, Clone, Default)]
pub struct ProbeReport {
    pub by_dex: BTreeMap<DexKind, ProbeTally>,
    pub quotes: u64,
    pub elapsed: Duration,
    pub slowest: Duration,
}

impl RouteError {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::UnknownPool(_) => "unknown_pool",
            Self::NotReady(_) => "not_ready",
            Self::NoClock => "no_clock",
            Self::Decode(_) => "decode",
            Self::DecodePanicked | Self::QuotePanicked => "panic",
            Self::Quote(error) => match error {
                QuoteError::Unsupported(_) => "unsupported",
                QuoteError::Incomplete(_) => "incomplete",
                QuoteError::Inconsistent(_) => "inconsistent",
                QuoteError::Disabled => "disabled",
                QuoteError::Liquidity => "liquidity",
                QuoteError::Math => "math",
                QuoteError::Arrays(_) => "arrays",
                QuoteError::TransferHook => "transfer_hook",
            },
        }
    }
}

impl<F: PoolFeed> QuoteReader<F> {
    /// Quotes `amount_in` both ways through every decoded pool, the way a
    /// route search would, and tallies the outcomes.
    #[must_use]
    pub fn probe(&self, amount_in: u64, max_arrays: u8) -> ProbeReport {
        let mut report = ProbeReport::default();
        let started = Instant::now();
        for (pool, dex) in self.table.pools() {
            let tally = report.by_dex.entry(dex).or_default();
            for a_to_b in [true, false] {
                let begun = Instant::now();
                let outcome = self.quote(&pool, amount_in, a_to_b, max_arrays);
                report.slowest = report.slowest.max(begun.elapsed());
                report.quotes += 1;
                match outcome {
                    Ok(_) => tally.quoted += 1,
                    Err(error) => *tally.refused.entry(error.label()).or_default() += 1,
                }
            }
        }
        report.elapsed = started.elapsed();
        report
    }
}
