use std::num::NonZeroU64;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use domain::{DexKind, Pubkey, Slot};
use serde::{Deserialize, Serialize};
use tx::{Fees, SwapInstructions};

use crate::error::{ApiError, SearchBody};
use crate::service::{DexFilter, QuotedRoute, RouteRequest, Routed, RoutedLeg, SearchQuality};

/// Unknown fields are refused: a client sending a misspelled option must
/// not believe it was applied.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct QuoteBody {
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
    slippage_bps: Option<u16>,
}

impl QuoteBody {
    pub(crate) fn into_request(self, default_slippage_bps: u16) -> Result<Quoting, ApiError> {
        Ok(Quoting {
            request: RouteRequest {
                from: address("fromTokenAddress", &self.from_token_address)?,
                to: address("toTokenAddress", &self.to_token_address)?,
                amount: amount(&self.amount)?,
                cycle: self.enable_cyclic_arbitrage,
                max_hops: self.max_hops,
                dexes: DexFilter {
                    only: self.dexes,
                    except: self.exclude_dexes,
                },
            },
            slippage_bps: slippage(self.slippage_bps.unwrap_or(default_slippage_bps))?,
        })
    }
}

pub(crate) struct Quoting {
    pub request: RouteRequest,
    pub slippage_bps: u16,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SwapBody {
    user_public_key: String,
    #[serde(default = "yes")]
    wrap_and_unwrap_sol: bool,
    #[serde(default)]
    priority_fee_lamports: u64,
    quote_request: Option<QuoteBody>,
    quote_response: Option<QuotedBody>,
}

const fn yes() -> bool {
    true
}

pub(crate) enum SwapSource {
    Search(Quoting),
    Quoted(QuotedRoute, u16),
}

pub(crate) struct Swapping {
    pub user: Pubkey,
    pub wrap_sol: bool,
    pub priority_fee_lamports: u64,
    pub source: SwapSource,
}

impl SwapBody {
    pub(crate) fn into_swap(self, default_slippage_bps: u16) -> Result<Swapping, ApiError> {
        let source = match (self.quote_request, self.quote_response) {
            (Some(request), None) => {
                SwapSource::Search(request.into_request(default_slippage_bps)?)
            }
            (None, Some(quoted)) => {
                let slippage_bps = quoted.slippage_bps;
                SwapSource::Quoted(quoted.into_route()?, slippage_bps)
            }
            _ => {
                return Err(ApiError::invalid(
                    "send exactly one of quoteRequest and quoteResponse",
                ));
            }
        };
        Ok(Swapping {
            user: address("userPublicKey", &self.user_public_key)?,
            wrap_sol: self.wrap_and_unwrap_sol,
            priority_fee_lamports: self.priority_fee_lamports,
            source,
        })
    }
}

/// A `/quote` response sent back. Fields the route does not depend on
/// (`search`, `crossStream`) are ignored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuotedBody {
    from_token_address: String,
    to_token_address: String,
    from_token_amount: String,
    to_token_amount: String,
    other_amount_threshold: String,
    slippage_bps: u16,
    context_slot: u64,
    legs: Vec<QuotedLeg>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotedLeg {
    pool_address: String,
    dex: DexKind,
    from_token_address: String,
    to_token_address: String,
    from_token_amount: String,
    to_token_amount: String,
}

impl QuotedBody {
    fn into_route(self) -> Result<QuotedRoute, ApiError> {
        let legs = self
            .legs
            .into_iter()
            .map(|leg| {
                Ok(RoutedLeg {
                    pool: address("legs.poolAddress", &leg.pool_address)?,
                    dex: leg.dex,
                    from: address("legs.fromTokenAddress", &leg.from_token_address)?,
                    to: address("legs.toTokenAddress", &leg.to_token_address)?,
                    amount_in: amount(&leg.from_token_amount)?.get(),
                    amount_out: amount(&leg.to_token_amount)?.get(),
                })
            })
            .collect::<Result<_, ApiError>>()?;
        slippage(self.slippage_bps)?;
        Ok(QuotedRoute {
            routed: Routed {
                from: address("fromTokenAddress", &self.from_token_address)?,
                to: address("toTokenAddress", &self.to_token_address)?,
                amount_in: amount(&self.from_token_amount)?.get(),
                amount_out: amount(&self.to_token_amount)?.get(),
                slot: Slot(self.context_slot),
                cross_stream: false,
                search: SearchQuality {
                    pruned: false,
                    exhausted: false,
                    quotes: 0,
                },
                legs,
            },
            min_out: amount(&self.other_amount_threshold)?.get(),
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
    const MESSAGE: &str = "amounts must be positive integers of base units below 2^64, as strings";
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ApiError::invalid(MESSAGE));
    }
    text.parse().map_err(|_| ApiError::invalid(MESSAGE))
}

fn slippage(bps: u16) -> Result<u16, ApiError> {
    if bps > tx::MAX_SLIPPAGE_BPS {
        return Err(ApiError::invalid("slippageBps is at most 10000"));
    }
    Ok(bps)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuoteResponse {
    from_token_address: String,
    to_token_address: String,
    from_token_amount: String,
    to_token_amount: String,
    other_amount_threshold: String,
    slippage_bps: u16,
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

impl QuoteResponse {
    pub(crate) fn new(routed: Routed, min_out: u64, slippage_bps: u16) -> Self {
        Self {
            from_token_address: routed.from.to_string(),
            to_token_address: routed.to.to_string(),
            from_token_amount: routed.amount_in.to_string(),
            to_token_amount: routed.amount_out.to_string(),
            other_amount_threshold: min_out.to_string(),
            slippage_bps,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstructionBody {
    program_id: String,
    accounts: Vec<AccountBody>,
    data: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountBody {
    pubkey: String,
    is_signer: bool,
    is_writable: bool,
}

impl From<&tx::Instruction> for InstructionBody {
    fn from(instruction: &tx::Instruction) -> Self {
        Self {
            program_id: instruction.program_id.to_string(),
            accounts: instruction
                .accounts
                .iter()
                .map(|meta| AccountBody {
                    pubkey: meta.pubkey.to_string(),
                    is_signer: meta.is_signer,
                    is_writable: meta.is_writable,
                })
                .collect(),
            data: STANDARD.encode(&instruction.data),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SwapInstructionsResponse {
    quote: QuoteResponse,
    setup_instructions: Vec<InstructionBody>,
    swap_instruction: InstructionBody,
    cleanup_instructions: Vec<InstructionBody>,
    compute_unit_limit: u32,
    priority_fee_lamports: u64,
}

impl SwapInstructionsResponse {
    pub(crate) fn new(quote: QuoteResponse, instructions: &SwapInstructions, fees: Fees) -> Self {
        Self {
            quote,
            setup_instructions: instructions.setup.iter().map(Into::into).collect(),
            swap_instruction: (&instructions.swap).into(),
            cleanup_instructions: instructions.cleanup.iter().map(Into::into).collect(),
            compute_unit_limit: fees.compute_unit_limit,
            priority_fee_lamports: fees.priority_fee_lamports,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SwapResponse {
    quote: QuoteResponse,
    transaction: String,
    last_valid_block_height: u64,
    compute_unit_limit: u32,
    priority_fee_lamports: u64,
}

impl SwapResponse {
    pub(crate) fn new(
        quote: QuoteResponse,
        transaction: &[u8],
        last_valid_block_height: u64,
        fees: Fees,
    ) -> Self {
        Self {
            quote,
            transaction: STANDARD.encode(transaction),
            last_valid_block_height,
            compute_unit_limit: fees.compute_unit_limit,
            priority_fee_lamports: fees.priority_fee_lamports,
        }
    }
}
