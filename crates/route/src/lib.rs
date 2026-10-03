mod decoder;
mod direct;
mod error;
mod feed;
mod probe;
mod reader;
mod search;
mod session;
mod settings;
mod stats;

pub use decoder::{Decoder, Decoding};
pub use direct::{Candidate, Direct};
pub use error::RouteError;
pub use feed::PoolFeed;
pub use probe::{ProbeReport, ProbeTally};
pub use reader::{Quote, QuoteReader, Revision};
pub use search::{Engine, Everything, Filter, Goal, Leg, Path, Query, Search};
pub use session::{Pinned, SearchSession, Verdict};
pub use settings::RouteSettings;
pub use stats::RouteStatsSnapshot;

mod chunked;
mod flow;
mod pricer;
mod relaxed;
pub use flow::{Allocation, Flow, FlowOptions, FlowSearch, Operation, SPLIT_CHUNKS};

/// Resolves an on-chain program identity through the DEX registry.
#[must_use]
pub fn dex_kind(program: &domain::Pubkey) -> Option<domain::DexKind> {
    dex::by_program(program).map(|spec| spec.kind)
}

#[cfg(test)]
mod tests;
