use crate::{DecodeError, read_bool, read_key};

pub const CONFIG_SEED: &[u8] = b"config";
pub const CONFIG_LEN: usize = 36;

const DISCRIMINATOR: u8 = 1;
const VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub admin: [u8; 32],
    pub paused: bool,
    pub bump: u8,
}

impl Config {
    #[must_use]
    pub fn encode(&self) -> [u8; CONFIG_LEN] {
        let mut out = [0u8; CONFIG_LEN];
        out[0] = DISCRIMINATOR;
        out[1] = VERSION;
        out[2..34].copy_from_slice(&self.admin);
        out[34] = u8::from(self.paused);
        out[35] = self.bump;
        out
    }

    pub fn decode(data: &[u8]) -> Result<Self, DecodeError> {
        if data.len() != CONFIG_LEN {
            return Err(DecodeError::Length);
        }
        if data[0] != DISCRIMINATOR {
            return Err(DecodeError::Discriminator);
        }
        if data[1] != VERSION {
            return Err(DecodeError::UnsupportedVersion);
        }
        Ok(Self {
            admin: read_key(data, 2)?,
            paused: read_bool(data[34])?,
            bump: data[35],
        })
    }
}
