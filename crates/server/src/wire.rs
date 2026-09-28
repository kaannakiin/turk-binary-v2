use std::num::NonZeroU64;

use domain::{DexKind, Pubkey};
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, SearchBody};
use crate::service::{DexFilter, RouteRequest, Routed, SearchQuality};

/// Unknown fields are refused: a client sending `slippagePercent` must not
/// believe it was applied.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RouteBody {
    from_token_address: String,
    to_token_address: String,
    amount: String,
    #[serde(default)]
    enable_cyclic_arbitrage: bool,
    max_hops: Option<u8>,
    #[serde(default)]
    dexes: Vec<DexKind>,
    #[serde(default)]
    exclude_dexes: Vec<DexKind>,
}

impl RouteBody {
    pub(crate) fn into_request(self) -> Result<RouteRequest, ApiError> {
        Ok(RouteRequest {
            from: address("fromTokenAddress", &self.from_token_address)?,
            to: address("toTokenAddress", &self.to_token_address)?,
            amount: amount(&self.amount)?,
            cycle: self.enable_cyclic_arbitrage,
            max_hops: self.max_hops,
            dexes: DexFilter {
                only: self.dexes,
                except: self.exclude_dexes,
            },
        })
    }
}

fn address(field: &str, text: &str) -> Result<Pubkey, ApiError> {
    text.parse()
        .map_err(|_| ApiError::invalid(format!("{field} is not a base58 address")))
}

/// Base units as decimal digits only: `u64::from_str` would also take a
/// leading `+`.
fn amount(text: &str) -> Result<NonZeroU64, ApiError> {
    const MESSAGE: &str = "amount must be a positive integer of base units below 2^64, as a string";
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ApiError::invalid(MESSAGE));
    }
    text.parse().map_err(|_| ApiError::invalid(MESSAGE))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RouteResponse {
    from_token_address: String,
    to_token_address: String,
    from_token_amount: String,
    to_token_amount: String,
    context_slot: u64,
    cross_stream: bool,
    search: SearchBody,
    legs: Vec<LegBody>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LegBody {
    pool_address: String,
    dex: &'static str,
    from_token_address: String,
    to_token_address: String,
    from_token_amount: String,
    to_token_amount: String,
}

impl From<SearchQuality> for SearchBody {
    fn from(search: SearchQuality) -> Self {
        Self {
            pruned: search.pruned,
            exhausted: search.exhausted,
            quotes: search.quotes,
        }
    }
}

impl From<Routed> for RouteResponse {
    fn from(routed: Routed) -> Self {
        Self {
            from_token_address: routed.from.to_string(),
            to_token_address: routed.to.to_string(),
            from_token_amount: routed.amount_in.to_string(),
            to_token_amount: routed.amount_out.to_string(),
            context_slot: routed.slot.0,
            cross_stream: routed.cross_stream,
            search: routed.search.into(),
            legs: routed
                .legs
                .into_iter()
                .map(|leg| LegBody {
                    pool_address: leg.pool.to_string(),
                    dex: leg.dex.as_str(),
                    from_token_address: leg.from.to_string(),
                    to_token_address: leg.to.to_string(),
                    from_token_amount: leg.amount_in.to_string(),
                    to_token_amount: leg.amount_out.to_string(),
                })
                .collect(),
        }
    }
}
