use crate::DecodeError;

// Numbers follow `domain::DexKind`'s declaration order; a number exists only
// once the program has an adapter for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HopKind {
    RaydiumCpmm = 2,
}

impl TryFrom<u8> for HopKind {
    type Error = DecodeError;

    fn try_from(kind: u8) -> Result<Self, DecodeError> {
        match kind {
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
