use crate::DecodeError;

// Numbers follow `domain::DexKind`'s declaration order; a number exists only
// once the program has an adapter for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HopKind {
    RaydiumAmmV4 = 0,
    RaydiumClmm = 1,
    RaydiumCpmm = 2,
}

impl TryFrom<u8> for HopKind {
    type Error = DecodeError;

    fn try_from(kind: u8) -> Result<Self, DecodeError> {
        match kind {
            0 => Ok(Self::RaydiumAmmV4),
            1 => Ok(Self::RaydiumClmm),
            2 => Ok(Self::RaydiumCpmm),
            _ => Err(DecodeError::UnknownHopKind),
        }
    }
}

impl From<HopKind> for u8 {
    fn from(kind: HopKind) -> Self {
        kind as u8
    }
}
