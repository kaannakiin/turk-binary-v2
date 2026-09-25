#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("{0} pools exceed the {max} an edge id can address", max = u32::MAX >> 1)]
    TooManyPools(usize),
    #[error("{0} mints exceed what a mint id can address")]
    TooManyMints(usize),
}
