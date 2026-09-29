use domain::Pubkey;
use market::Reason;
use quoter::{DecodeError, QuoteError, WindowError};

#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    #[error("invalid exact-in flow")]
    InvalidFlow,
    #[error("flow needs an unverified shared-account state transition")]
    StatefulFlowUnsupported,
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
    #[error(transparent)]
    Window(#[from] WindowError),
}
