#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum RouterError {
    Paused = 6000,
    BadArgs = 6001,
    BadHopCount = 6002,
    UnsupportedWireVersion = 6003,
    AtaOwnerMismatch = 6004,
    WindowOutOfBounds = 6005,
    UnknownHopKind = 6006,
    BadWindow = 6007,
    NotATokenAccount = 6008,
    HopContinuityViolation = 6009,
    ActualInOutOfBand = 6010,
    ZeroHopOutput = 6011,
    BalanceRegression = 6012,
    SlippageExceeded = 6013,
    CircularRouteNotProfitable = 6014,
    NotAdmin = 6015,
    ZeroAdmin = 6016,
    NotUpgradeAuthority = 6017,
    BadProgramData = 6018,
}

impl From<RouterError> for u32 {
    fn from(error: RouterError) -> Self {
        error as u32
    }
}
