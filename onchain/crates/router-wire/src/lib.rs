mod config;
mod hop_kind;
mod instruction;
mod route;

pub use config::{CONFIG_LEN, CONFIG_SEED, Config};
pub use hop_kind::HopKind;
pub use instruction::RouterInstruction;
pub use route::{Hop, MAX_HOPS, ROUTE_VERSION, Route};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    UnknownInstruction,
    Length,
    InvalidBool,
    UnsupportedVersion,
    HopCount,
    Discriminator,
    UnknownHopKind,
}

fn read_u64(data: &[u8], at: usize) -> Result<u64, DecodeError> {
    let bytes = data
        .get(at..at.checked_add(8).ok_or(DecodeError::Length)?)
        .ok_or(DecodeError::Length)?;
    let mut buf = [0u8; 8];
    buf.copy_from_slice(bytes);
    Ok(u64::from_le_bytes(buf))
}

fn read_key(data: &[u8], at: usize) -> Result<[u8; 32], DecodeError> {
    let bytes = data
        .get(at..at.checked_add(32).ok_or(DecodeError::Length)?)
        .ok_or(DecodeError::Length)?;
    let mut key = [0u8; 32];
    key.copy_from_slice(bytes);
    Ok(key)
}

fn read_bool(byte: u8) -> Result<bool, DecodeError> {
    match byte {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(DecodeError::InvalidBool),
    }
}

#[cfg(test)]
mod tests;
