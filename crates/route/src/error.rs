use domain::Pubkey;
use market::Reason;
use quoter::{DecodeError, QuoteError};

#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    #[error("route.route_threads must be at least 1")]
    Threads,
    #[error("starting a route thread: {0}")]
    Thread(#[source] std::io::Error),
    #[error("a route thread panicked")]
    ThreadPanicked,
    #[error("{0} is not in the universe")]
    UnknownPool(Pubkey),
    #[error("pool is not ready: {0:?}")]
    NotReady(Reason),
    #[error("the pool changed since it was last decoded")]
    Stale,
    #[error("no Clock sysvar yet")]
    NoClock,
    #[error("decoding the pool failed: {0}")]
    Decode(#[source] DecodeError),
    #[error("decoding the pool panicked")]
    DecodePanicked,
    #[error("quoting panicked")]
    QuotePanicked,
    #[error(transparent)]
    Quote(#[from] QuoteError),
}
