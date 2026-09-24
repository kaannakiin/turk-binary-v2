use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, de::Error};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Commitment {
    #[default]
    Processed,
    Confirmed,
    Finalized,
}

impl Commitment {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Processed => "processed",
            Self::Confirmed => "confirmed",
            Self::Finalized => "finalized",
        }
    }
}

impl fmt::Display for Commitment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown commitment `{0}`, expected processed, confirmed or finalized")]
pub struct UnknownCommitment(String);

impl FromStr for Commitment {
    type Err = UnknownCommitment;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "processed" => Ok(Self::Processed),
            "confirmed" => Ok(Self::Confirmed),
            "finalized" => Ok(Self::Finalized),
            _ => Err(UnknownCommitment(s.to_owned())),
        }
    }
}

impl<'de> Deserialize<'de> for Commitment {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        <&str>::deserialize(de)?.parse().map_err(D::Error::custom)
    }
}
