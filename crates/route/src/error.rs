use domain::Pubkey;
use market::Reason;
use quoter::{DecodeError, QuoteError};

#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    #[error("{0} is not in the universe")]
    UnknownPool(Pubkey),
    #[error("pool is not ready: {0:?}")]
    NotReady(Reason),
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
