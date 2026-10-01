use domain::DexKind;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TxError {
    #[error("the route has no hop")]
    EmptyRoute,
    #[error("the flow has no operation")]
    EmptyFlow,
    #[error("the router runs at most {max} hops, the route has {hops}")]
    TooManyHops { hops: usize, max: usize },
    #[error("the flow has {steps} operations, a flow holds at most {max}")]
    TooManyFlowSteps { steps: usize, max: usize },
    #[error("the flow has {slots} slots, a flow holds at most {max}")]
    TooManyFlowSlots { slots: usize, max: usize },
    #[error("the router has no adapter for {0}")]
    Unsupported(DexKind),
    #[error("a cycle must require more than it spends: threshold {min_out}, input {amount_in}")]
    UnprofitableCycle { amount_in: u64, min_out: u64 },
    #[error("hop {hop} does not start with the mint the previous hop paid out")]
    Discontinuous { hop: usize },
    #[error(
        "the route needs one positive minimum output per hop, with the last meeting the route minimum"
    )]
    InvalidHopThresholds,
    #[error("the flow operations, windows, and thresholds must have equal lengths")]
    InvalidFlowShape,
    #[error("a flow operation does not match its slot token sides")]
    FlowSlotMismatch,
    #[error("a flow allocation is invalid")]
    InvalidFlowAllocation,
    #[error("a swap window has an invalid optional account tail")]
    InvalidOptionalTail,
    #[error("the transaction needs {count} accounts, at most {max} are allowed")]
    TooManyAccounts { count: usize, max: usize },
    #[error("the transaction is {bytes} bytes, a v1 transaction holds {max}")]
    TooLarge { bytes: usize, max: usize },
    #[error("no loaded-data budget is known for program {0}")]
    UnknownProgram(domain::Pubkey),
    #[error("the transaction would load {bytes} bytes of accounts, past the 64 MiB limit")]
    TooMuchData { bytes: u64 },
    #[error("DLMM needs {arrays} bin arrays; router compute was measured only through three")]
    UnmeasuredDlmmArrays { arrays: u8 },
    #[error("the route needs {units} compute units, beyond the v1 limit of {max}")]
    TooMuchCompute { units: u32, max: u32 },
    #[error("the transaction does not compile: {0}")]
    Compile(String),
}
