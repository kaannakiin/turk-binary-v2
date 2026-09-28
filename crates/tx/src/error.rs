use domain::DexKind;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TxError {
    #[error("the route has no hop")]
    EmptyRoute,
    #[error("the router runs at most {max} hops, the route has {hops}")]
    TooManyHops { hops: usize, max: usize },
    #[error("the router has no adapter for {0}")]
    Unsupported(DexKind),
    #[error("hop {hop} does not start with the mint the previous hop paid out")]
    Discontinuous { hop: usize },
    #[error("the transaction needs {count} accounts, a v1 transaction holds {max}")]
    TooManyAccounts { count: usize, max: usize },
    #[error("the transaction is {bytes} bytes, a v1 transaction holds {max}")]
    TooLarge { bytes: usize, max: usize },
    #[error("the transaction does not compile: {0}")]
    Compile(String),
}
