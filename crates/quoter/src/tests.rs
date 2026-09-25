#[cfg(feature = "damm-v1")]
mod damm_v1;
#[cfg(feature = "damm-v2")]
mod damm_v2;
#[cfg(feature = "dlmm")]
mod dlmm;
#[cfg(feature = "pumpswap")]
mod pumpswap;
#[cfg(feature = "raydium-amm-v4")]
mod raydium_amm_v4;
#[cfg(feature = "raydium-clmm")]
mod raydium_clmm;
#[cfg(feature = "raydium-cpmm")]
mod raydium_cpmm;
mod sim;
mod svm;
mod token22;
#[cfg(feature = "whirlpool")]
mod whirlpool;

use std::io::Read as _;
use std::path::PathBuf;

use domain::Pubkey;
use flate2::read::GzDecoder;

pub(crate) fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/tests/fixtures")
}

pub(crate) fn sim_fixture(file: &str) -> String {
    let compressed = std::fs::File::open(fixtures().join("sim").join(file)).expect("fixture");
    let mut text = String::new();
    GzDecoder::new(compressed)
        .read_to_string(&mut text)
        .expect("gzip fixture");
    text
}

/// Mainnet accounts captured with `scripts/capture_accounts.py`. An account
/// that did not exist has no owner and no data.
fn captured() -> Vec<(Pubkey, Pubkey, Vec<u8>)> {
    let dir = fixtures().join("accounts");
    let manifest = std::fs::read_to_string(dir.join("accounts.tsv")).expect("manifest");
    manifest
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let key: Pubkey = fields.next()?.parse().ok()?;
            let owner = fields.next()?;
            if owner == "-" {
                return Some((key, Pubkey::default(), Vec::new()));
            }
            let data = std::fs::read(dir.join(format!("{key}.bin"))).expect("captured bytes");
            Some((key, owner.parse().ok()?, data))
        })
        .collect()
}
