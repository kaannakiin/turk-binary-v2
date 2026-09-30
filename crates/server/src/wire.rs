use std::num::NonZeroU64;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use domain::{DexKind, Pubkey, Slot};
use serde::{Deserialize, Serialize};
use tx::SwapInstructions;

use crate::error::{ApiError, SearchBody};
use crate::service::{DexFilter, QuotedRoute, RouteRequest, Routed, RoutedLeg, SearchQuality};

/// Unknown fields are refused: a client sending a misspelled option must
/// not believe it was applied.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)] // Mirrors independent HTTP routing controls.
pub(crate) struct QuoteBody {
    from_token_address: String,
    to_token_address: String,
    amount: String,
    user_wallet_address: Option<String>,
    #[serde(default)]
    enable_cyclic_arbitrage: bool,
    max_hops: Option<u8>,
    #[serde(default)]
    dex_ids: String,
    #[serde(default)]
    excluded_dex_ids: String,
    allowed_pools: Option<Vec<String>>,
    #[serde(default)]
    direct_route: bool,
    #[serde(default)]
    single_route_only: bool,
    #[serde(default)]
    single_pool_per_hop: bool,
    #[serde(default)]
    unique_dex_ids: String,
    #[serde(default = "yes")]
    enable_unique_dex: bool,
    slippage_percent: Option<String>,
}

impl QuoteBody {
    pub(crate) fn into_request(
        self,
        default_slippage_bps: u16,
        default_unique_dex_ids: &[String],
    ) -> Result<Quoting, ApiError> {
        if let Some(user) = self.user_wallet_address.as_deref() {
            let _ = address("userWalletAddress", user)?;
        }
        let only = dex_ids("dexIds", &self.dex_ids)?;
        let except = dex_ids("excludedDexIds", &self.excluded_dex_ids)?;
        let requested_unique = dex_ids("uniqueDexIds", &self.unique_dex_ids)?;
        let unique = if self.enable_unique_dex && self.enable_cyclic_arbitrage {
            if requested_unique.is_empty() {
                configured_dex_ids(default_unique_dex_ids)?
            } else {
                requested_unique
            }
        } else {
            Vec::new()
        };
        if self.direct_route && self.enable_cyclic_arbitrage {
            return Err(ApiError::invalid(
                "directRoute cannot be combined with enableCyclicArbitrage",
            ));
        }
        let max_hops = self.direct_route.then_some(1).or(self.max_hops);
        Ok(Quoting {
            request: RouteRequest {
                from: address("fromTokenAddress", &self.from_token_address)?,
                to: address("toTokenAddress", &self.to_token_address)?,
                amount: amount(&self.amount)?,
                cycle: self.enable_cyclic_arbitrage,
                max_hops,
                dexes: DexFilter {
                    only,
                    except,
                    allowed_pools: allowed_pools(self.allowed_pools)?,
                    unique,
                },
                direct_route: self.direct_route,
                single_route_only: self.single_route_only,
                single_pool_per_hop: self.single_pool_per_hop,
            },
            slippage_bps: self
                .slippage_percent
                .as_deref()
                .map(slippage_percent)
                .transpose()?
                .unwrap_or(slippage_bps(default_slippage_bps)?),
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
    user_wallet_address: String,
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
    pub(crate) fn into_swap(
        self,
        default_slippage_bps: u16,
        default_unique_dex_ids: &[String],
    ) -> Result<Swapping, ApiError> {
        let source = match (self.quote_request, self.quote_response) {
            (Some(request), None) => SwapSource::Search(
                request.into_request(default_slippage_bps, default_unique_dex_ids)?,
            ),
            (None, Some(quoted)) => {
                let slippage_bps = slippage_percent(&quoted.slippage_percent)?;
                SwapSource::Quoted(quoted.into_route()?, slippage_bps)
            }
            _ => {
                return Err(ApiError::invalid(
                    "send exactly one of quoteRequest and quoteResponse",
                ));
            }
        };
        Ok(Swapping {
            user: address("userWalletAddress", &self.user_wallet_address)?,
            wrap_sol: self.wrap_and_unwrap_sol,
            priority_fee_lamports: self.priority_fee_lamports,
            source,
        })
    }
}

/// A `/quote` response sent back. `search` and `crossStream` describe the
/// search that priced it and are echoed as sent.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuotedBody {
    from_token_address: String,
    to_token_address: String,
    from_token_amount: String,
    to_token_amount: String,
    other_amount_threshold: String,
    slippage_percent: String,
    context_slot: u64,
    cross_stream: Option<bool>,
    search: Option<SearchBody>,
    slots: Vec<String>,
    operations: Vec<QuotedLeg>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotedLeg {
    source_slot: u8,
    destination_slot: u8,
    input_share: InputShare,
    dependencies: Vec<usize>,
    pool_address: String,
    dex: DexKind,
    from_token_address: String,
    to_token_address: String,
    from_token_amount: String,
    to_token_amount: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct InputShare {
    numerator: String,
    denominator: String,
}

impl QuotedBody {
    fn into_route(self) -> Result<QuotedRoute, ApiError> {
        for (index, operation) in self.operations.iter().enumerate() {
            let dependencies: Vec<_> = self.operations[..index]
                .iter()
                .enumerate()
                .filter_map(|(producer, previous)| {
                    (previous.destination_slot == operation.source_slot).then_some(producer)
                })
                .collect();
            if dependencies != operation.dependencies {
                return Err(ApiError::invalid(
                    "operations.dependencies does not match the input slot producers",
                ));
            }
        }
        let legs = self
            .operations
            .into_iter()
            .map(|leg| {
                Ok(RoutedLeg {
                    allocation: route::Allocation {
                        source: leg.source_slot,
                        destination: leg.destination_slot,
                        numerator: amount(&leg.input_share.numerator)?.get(),
                        denominator: amount(&leg.input_share.denominator)?.get(),
                    },
                    pool: address("operations.poolAddress", &leg.pool_address)?,
                    dex: leg.dex,
                    from: address("operations.fromTokenAddress", &leg.from_token_address)?,
                    to: address("operations.toTokenAddress", &leg.to_token_address)?,
                    amount_in: amount(&leg.from_token_amount)?.get(),
                    amount_out: amount(&leg.to_token_amount)?.get(),
                })
            })
            .collect::<Result<_, ApiError>>()?;
        let _ = slippage_percent(&self.slippage_percent)?;
        Ok(QuotedRoute {
            routed: Routed {
                from: address("fromTokenAddress", &self.from_token_address)?,
                to: address("toTokenAddress", &self.to_token_address)?,
                amount_in: amount(&self.from_token_amount)?.get(),
                amount_out: amount(&self.to_token_amount)?.get(),
                slot: Slot(self.context_slot),
                cross_stream: self.cross_stream,
                search: self.search.map(|search| SearchQuality {
                    pruned: search.pruned,
                    exhausted: search.exhausted,
                    timed_out: search.timed_out,
                    quotes: search.quotes,
                }),
                slots: self
                    .slots
                    .iter()
                    .map(|mint| address("slots", mint))
                    .collect::<Result<_, _>>()?,
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

fn dex_ids(field: &str, text: &str) -> Result<Vec<DexKind>, ApiError> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    resolve_dex_ids(field, text.split(','))
}

fn configured_dex_ids(ids: &[String]) -> Result<Vec<DexKind>, ApiError> {
    resolve_dex_ids(
        "server.quote.unique_dex_ids",
        ids.iter().map(String::as_str),
    )
}

fn resolve_dex_ids<'a>(
    field: &str,
    ids: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<DexKind>, ApiError> {
    let mut result = Vec::new();
    for id in ids {
        let id = id.trim();
        if id.is_empty() {
            return Err(ApiError::invalid(format!(
                "{field} contains an empty program id"
            )));
        }
        let program = address(field, id)?;
        let kind = route::dex_kind(&program).ok_or_else(|| {
            ApiError::invalid(format!("{field} contains an unknown DEX program id {id}"))
        })?;
        if !result.contains(&kind) {
            result.push(kind);
        }
    }
    Ok(result)
}

fn allowed_pools(value: Option<Vec<String>>) -> Result<Option<Vec<Pubkey>>, ApiError> {
    value
        .map(|pools| {
            let mut result = Vec::with_capacity(pools.len());
            for (index, pool) in pools.into_iter().enumerate() {
                let pool = address(&format!("allowedPools[{index}]"), &pool)?;
                if !result.contains(&pool) {
                    result.push(pool);
                }
            }
            Ok(result)
        })
        .transpose()
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

fn slippage_bps(bps: u16) -> Result<u16, ApiError> {
    if bps > tx::MAX_SLIPPAGE_BPS {
        return Err(ApiError::invalid("slippagePercent must be less than 100"));
    }
    Ok(bps)
}

fn slippage_percent(text: &str) -> Result<u16, ApiError> {
    const MESSAGE: &str = "slippagePercent must be a decimal percentage from 0 up to (but excluding) 100 with at most two decimal places";
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty()
        || (text.contains('.') && fraction.is_empty())
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > 2
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(ApiError::invalid(MESSAGE));
    }
    let whole = whole
        .parse::<u16>()
        .map_err(|_| ApiError::invalid(MESSAGE))?;
    if whole >= 100 {
        return Err(ApiError::invalid(MESSAGE));
    }
    let fraction = match fraction.len() {
        0 => 0,
        1 => {
            fraction
                .parse::<u16>()
                .map_err(|_| ApiError::invalid(MESSAGE))?
                * 10
        }
        2 => fraction
            .parse::<u16>()
            .map_err(|_| ApiError::invalid(MESSAGE))?,
        _ => return Err(ApiError::invalid(MESSAGE)),
    };
    whole
        .checked_mul(100)
        .and_then(|value| value.checked_add(fraction))
        .ok_or_else(|| ApiError::invalid(MESSAGE))
}

fn format_slippage_percent(bps: u16) -> String {
    let whole = bps / 100;
    let fraction = bps % 100;
    match fraction {
        0 => whole.to_string(),
        n if n % 10 == 0 => format!("{whole}.{}", n / 10),
        n => format!("{whole}.{n:02}"),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuoteResponse {
    from_token_address: String,
    to_token_address: String,
    from_token_amount: String,
    to_token_amount: String,
    other_amount_threshold: String,
    slippage_percent: String,
    context_slot: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cross_stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    search: Option<SearchBody>,
    slots: Vec<String>,
    operations: Vec<LegBody>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LegBody {
    source_slot: u8,
    destination_slot: u8,
    input_share: InputShare,
    dependencies: Vec<usize>,
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
            timed_out: search.timed_out,
            quotes: search.quotes,
        }
    }
}

impl QuoteResponse {
    pub(crate) fn new(routed: &Routed, min_out: u64, slippage_bps: u16) -> Self {
        Self {
            from_token_address: routed.from.to_string(),
            to_token_address: routed.to.to_string(),
            from_token_amount: routed.amount_in.to_string(),
            to_token_amount: routed.amount_out.to_string(),
            other_amount_threshold: min_out.to_string(),
            slippage_percent: format_slippage_percent(slippage_bps),
            context_slot: routed.slot.0,
            cross_stream: routed.cross_stream,
            search: routed.search.map(Into::into),
            slots: routed.slots.iter().map(ToString::to_string).collect(),
            operations: routed
                .legs
                .iter()
                .enumerate()
                .map(|(index, leg)| LegBody {
                    source_slot: leg.allocation.source,
                    destination_slot: leg.allocation.destination,
                    input_share: InputShare {
                        numerator: leg.allocation.numerator.to_string(),
                        denominator: leg.allocation.denominator.to_string(),
                    },
                    dependencies: routed.legs[..index]
                        .iter()
                        .enumerate()
                        .filter_map(|(producer, previous)| {
                            (previous.allocation.destination == leg.allocation.source)
                                .then_some(producer)
                        })
                        .collect(),
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
    loaded_accounts_data_size_limit: u32,
    priority_fee_lamports: u64,
}

impl SwapInstructionsResponse {
    pub(crate) fn new(
        quote: QuoteResponse,
        instructions: &SwapInstructions,
        priority_fee_lamports: u64,
    ) -> Self {
        Self {
            quote,
            setup_instructions: instructions.setup.iter().map(Into::into).collect(),
            swap_instruction: (&instructions.swap).into(),
            cleanup_instructions: instructions.cleanup.iter().map(Into::into).collect(),
            compute_unit_limit: instructions.limits.compute_units,
            loaded_accounts_data_size_limit: instructions.limits.loaded_accounts_data_bytes,
            priority_fee_lamports,
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
    loaded_accounts_data_size_limit: u32,
    priority_fee_lamports: u64,
}

impl SwapResponse {
    pub(crate) fn new(
        quote: QuoteResponse,
        transaction: &[u8],
        last_valid_block_height: u64,
        instructions: &SwapInstructions,
        priority_fee_lamports: u64,
    ) -> Self {
        Self {
            quote,
            transaction: STANDARD.encode(transaction),
            last_valid_block_height,
            compute_unit_limit: instructions.limits.compute_units,
            loaded_accounts_data_size_limit: instructions.limits.loaded_accounts_data_bytes,
            priority_fee_lamports,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{QuoteBody, slippage_percent};

    const WSOL: &str = "So11111111111111111111111111111111111111112";
    const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    const RAYDIUM_AMM_V4: &str = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8";
    const POOL: &str = "S2MiN5qmiRS8HBQMXcdJUhLwrBgX9P3naDuo4GkQ63t";

    fn request() -> Value {
        json!({
            "fromTokenAddress": WSOL,
            "toTokenAddress": USDC,
            "amount": "1000000",
        })
    }

    #[test]
    fn slippage_percent_converts_without_float_rounding() {
        assert_eq!(slippage_percent("0").expect("zero slippage"), 0);
        assert_eq!(slippage_percent("0.5").expect("half percent"), 50);
        assert_eq!(slippage_percent("12.34").expect("decimal slippage"), 1234);
        for invalid in ["", ".5", "1.", "0.001", "100", "100.00", "-1", "+1"] {
            assert!(slippage_percent(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn request_constraints_normalize_to_route_fields() {
        let mut value = request();
        value["slippagePercent"] = json!("0.5");
        value["dexIds"] = json!(RAYDIUM_AMM_V4);
        value["excludedDexIds"] = json!(RAYDIUM_AMM_V4);
        value["allowedPools"] = json!([POOL, POOL]);
        value["directRoute"] = json!(true);
        value["singleRouteOnly"] = json!(true);
        value["singlePoolPerHop"] = json!(true);
        let body: QuoteBody = serde_json::from_value(value).expect("request deserializes");
        let quoting = body.into_request(50, &[]).expect("request normalizes");
        assert_eq!(quoting.slippage_bps, 50);
        assert_eq!(quoting.request.max_hops, Some(1));
        assert!(quoting.request.direct_route);
        assert!(quoting.request.single_route_only);
        assert!(quoting.request.single_pool_per_hop);
        assert_eq!(quoting.request.dexes.only.len(), 1);
        assert_eq!(quoting.request.dexes.except.len(), 1);
        assert_eq!(
            quoting
                .request
                .dexes
                .allowed_pools
                .as_ref()
                .expect("allowlist"),
            &[POOL.parse().expect("pool address"),]
        );
    }

    #[test]
    fn empty_pool_allowlist_is_distinct_from_omitted_allowlist() {
        let omitted: QuoteBody = serde_json::from_value(request()).expect("request deserializes");
        assert!(
            omitted
                .into_request(50, &[])
                .expect("request normalizes")
                .request
                .dexes
                .allowed_pools
                .is_none()
        );

        let mut value = request();
        value["allowedPools"] = json!([]);
        let empty: QuoteBody = serde_json::from_value(value).expect("request deserializes");
        assert_eq!(
            empty
                .into_request(50, &[])
                .expect("request normalizes")
                .request
                .dexes
                .allowed_pools,
            Some(Vec::new())
        );
    }

    #[test]
    fn unique_dex_ids_are_validated_even_when_disabled() {
        let mut value = request();
        value["uniqueDexIds"] = json!("not-a-program-id");
        value["enableUniqueDex"] = json!(false);
        let body: QuoteBody = serde_json::from_value(value).expect("request deserializes");
        assert!(body.into_request(50, &[]).is_err());

        let mut value = request();
        value["directRoute"] = json!(true);
        value["enableCyclicArbitrage"] = json!(true);
        let body: QuoteBody = serde_json::from_value(value).expect("request deserializes");
        assert!(body.into_request(50, &[]).is_err());
    }
}
