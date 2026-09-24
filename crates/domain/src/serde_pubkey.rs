//! `solana-pubkey`'s own serde impl encodes a byte array; config files carry
//! base58 strings, so these helpers go through `FromStr`.

use serde::{Deserialize, Deserializer, de::Error};
use solana_pubkey::Pubkey;

pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Pubkey, D::Error> {
    let s = <&str>::deserialize(de)?;
    s.parse().map_err(|e| D::Error::custom(format!("{s}: {e}")))
}

pub mod vec {
    use super::{Deserialize, Deserializer, Error, Pubkey};

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<Pubkey>, D::Error> {
        Vec::<String>::deserialize(de)?
            .into_iter()
            .map(|s| s.parse().map_err(|e| D::Error::custom(format!("{s}: {e}"))))
            .collect()
    }
}
