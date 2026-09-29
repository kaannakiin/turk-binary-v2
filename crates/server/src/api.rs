use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use domain::Pubkey;
use route::PoolFeed;
use tower_http::timeout::RequestBodyTimeoutLayer;
use tx::{SwapInstructions, SwapRequest, TxError};

use crate::blockhash::BlockhashSlot;
use crate::error::ApiError;
use crate::executor::{Refused, SearchPool};
use crate::service::{QuoteService, QuoteSlot, ServiceError};
use crate::settings::{QuoteSettings, SwapSettings};
use crate::transport;
use crate::wire::{
    QuoteBody, QuoteResponse, SwapBody, SwapInstructionsResponse, SwapResponse, SwapSource,
    Swapping,
};

const MAX_BODY: usize = 16 * 1024;

struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub(crate) struct Api<F> {
    pub pool: Arc<SearchPool>,
    pub quotes: QuoteSlot<F>,
    pub settings: QuoteSettings,
    pub swap: SwapSettings,
    pub blockhashes: BlockhashSlot,
    pub max_clock_stall: Duration,
    pub read_timeout: Duration,
}

impl<F> Clone for Api<F> {
    fn clone(&self) -> Self {
        Self {
            pool: Arc::clone(&self.pool),
            quotes: self.quotes.clone(),
            settings: self.settings.clone(),
            swap: self.swap,
            blockhashes: self.blockhashes.clone(),
            max_clock_stall: self.max_clock_stall,
            read_timeout: self.read_timeout,
        }
    }
}

pub(crate) fn router<F: PoolFeed>(api: Api<F>) -> Router {
    let read_timeout = api.read_timeout;
    let router = Router::new()
        .route("/quote", post(quote::<F>))
        .route("/swap-instructions", post(swap_instructions::<F>))
        .route("/swap", post(swap::<F>))
        .fallback(|| async { ApiError::NOT_FOUND })
        .method_not_allowed_fallback(|| async { ApiError::METHOD_NOT_ALLOWED })
        .layer(DefaultBodyLimit::max(MAX_BODY))
        // A body that stops arriving would otherwise hold its connection
        // forever; the search timeout starts only once the body is read.
        .layer(RequestBodyTimeoutLayer::new(read_timeout))
        .with_state(api);
    transport::layered(router)
}

impl<F: PoolFeed> Api<F> {
    fn service(&self) -> Result<QuoteService<F>, ApiError> {
        self.quotes
            .service(self.settings.clone(), self.swap, self.max_clock_stall)
            .ok_or(ApiError::NOT_READY)
    }

    async fn on_search_thread<T: Send + 'static>(
        &self,
        cancellation: Arc<AtomicBool>,
        work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
    ) -> Result<T, ApiError> {
        let _cancel_on_drop = CancelOnDrop(cancellation);
        let pending = self.pool.submit(work).map_err(|refused| match refused {
            Refused::Full => ApiError::OVERLOADED,
            Refused::Closed => ApiError::SHUTTING_DOWN,
        })?;
        tokio::time::timeout(self.settings.timeout(), pending)
            .await
            .map_err(|_| ApiError::TIMEOUT)?
            .map_err(|_| ApiError::INTERNAL)?
    }
}

async fn quote<F: PoolFeed>(
    State(api): State<Api<F>>,
    body: Result<Json<QuoteBody>, JsonRejection>,
) -> Result<Json<QuoteResponse>, ApiError> {
    let Json(body) = body.map_err(|rejection| ApiError::invalid(rejection.body_text()))?;
    let quoting = body.into_request(api.swap.default_slippage_bps, &api.settings.unique_dex_ids)?;
    let service = api.service()?;
    let response = api
        .on_search_thread(service.cancellation(), move || {
            let routed = service.route(&quoting.request)?;
            let min_out = threshold(routed.amount_out, quoting.slippage_bps)?;
            Ok(QuoteResponse::new(&routed, min_out, quoting.slippage_bps))
        })
        .await?;
    Ok(Json(response))
}

async fn swap_instructions<F: PoolFeed>(
    State(api): State<Api<F>>,
    body: Result<Json<SwapBody>, JsonRejection>,
) -> Result<Json<SwapInstructionsResponse>, ApiError> {
    let plan = plan(&api, body).await?;
    Ok(Json(SwapInstructionsResponse::new(
        plan.quote,
        &plan.instructions,
        plan.priority_fee_lamports,
    )))
}

async fn swap<F: PoolFeed>(
    State(api): State<Api<F>>,
    body: Result<Json<SwapBody>, JsonRejection>,
) -> Result<Json<SwapResponse>, ApiError> {
    let plan = plan(&api, body).await?;
    let blockhash = api
        .blockhashes
        .fresh(api.swap.max_blockhash_age())
        .ok_or(ApiError::NO_BLOCKHASH)?;
    let transaction = tx::unsigned_v1(
        &plan.instructions,
        &plan.user,
        blockhash.hash,
        plan.priority_fee_lamports,
    )
    .map_err(ApiError::from)?;
    Ok(Json(SwapResponse::new(
        plan.quote,
        &transaction,
        blockhash.last_valid_block_height,
        &plan.instructions,
        plan.priority_fee_lamports,
    )))
}

struct Plan {
    user: Pubkey,
    quote: QuoteResponse,
    instructions: SwapInstructions,
    priority_fee_lamports: u64,
}

async fn plan<F: PoolFeed>(
    api: &Api<F>,
    body: Result<Json<SwapBody>, JsonRejection>,
) -> Result<Plan, ApiError> {
    let Json(body) = body.map_err(|rejection| ApiError::invalid(rejection.body_text()))?;
    let swapping = body.into_swap(api.swap.default_slippage_bps, &api.settings.unique_dex_ids)?;
    let service = api.service()?;
    api.on_search_thread(service.cancellation(), move || {
        swap_plan(&service, swapping)
    })
    .await
}

fn swap_plan<F: PoolFeed>(service: &QuoteService<F>, swapping: Swapping) -> Result<Plan, ApiError> {
    let (priced, min_out, slippage_bps) = match swapping.source {
        SwapSource::Search(quoting) => {
            let priced =
                service.route_to_swap(&quoting.request, swapping.user, swapping.wrap_sol)?;
            let min_out = threshold(priced.routed.amount_out, quoting.slippage_bps)?;
            (priced, min_out, quoting.slippage_bps)
        }
        SwapSource::Quoted(quoted, slippage_bps) => {
            let priced = service.quoted(&quoted)?;
            (priced, quoted.min_out, slippage_bps)
        }
    };
    let terminal_operations = priced
        .routed
        .legs
        .iter()
        .filter(|leg| leg.allocation.destination == 1)
        .count();
    let hop_min_outs = priced
        .windows
        .iter()
        .zip(&priced.routed.legs)
        .map(|(_, leg)| {
            let net = threshold(leg.amount_out, slippage_bps)?;
            let net = if terminal_operations == 1 && leg.allocation.destination == 1 {
                net.max(min_out)
            } else {
                net
            };
            Ok::<u64, ApiError>(net)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let linear = priced.windows.len() <= tx::MAX_HOPS
        && priced
            .routed
            .legs
            .iter()
            .all(|leg| leg.allocation.numerator == leg.allocation.denominator)
        && priced
            .routed
            .legs
            .windows(2)
            .all(|pair| pair[0].allocation.destination == pair[1].allocation.source);
    let instructions = if linear {
        tx::build(&SwapRequest {
            user: swapping.user,
            hops: &priced.windows,
            amount_in: priced.routed.amount_in,
            min_out,
            hop_min_outs: &hop_min_outs,
            wrap_sol: swapping.wrap_sol,
        })?
    } else {
        let slots = flow_slots(&priced)?;
        let allocations: Vec<_> = priced
            .routed
            .legs
            .iter()
            .map(|leg| tx::FlowAllocation {
                source: leg.allocation.source,
                destination: leg.allocation.destination,
                numerator: leg.allocation.numerator,
                denominator: leg.allocation.denominator,
            })
            .collect();
        tx::build_flow(&tx::FlowSwapRequest {
            user: swapping.user,
            slots: &slots,
            windows: &priced.windows,
            allocations: &allocations,
            step_min_outs: &hop_min_outs,
            amount_in: priced.routed.amount_in,
            min_out,
            wrap_sol: swapping.wrap_sol,
        })?
    };
    Ok(Plan {
        user: swapping.user,
        quote: QuoteResponse::new(&priced.routed, min_out, slippage_bps),
        instructions,
        priority_fee_lamports: swapping.priority_fee_lamports,
    })
}

fn flow_slots(priced: &crate::service::Priced) -> Result<Vec<domain::TokenSide>, ApiError> {
    let mut slots = vec![None; priced.routed.slots.len()];
    for (leg, window) in priced.routed.legs.iter().zip(&priced.windows) {
        for (index, side) in [
            (leg.allocation.source, window.source),
            (leg.allocation.destination, window.destination),
        ] {
            let index = usize::from(index);
            let slot = slots
                .get_mut(index)
                .ok_or_else(|| ApiError::invalid("invalid flow slot"))?;
            if priced.routed.slots[index] != side.mint || slot.is_some_and(|old| old != side) {
                return Err(ApiError::invalid(
                    "flow slot token identity is inconsistent",
                ));
            }
            *slot = Some(side);
        }
    }
    slots
        .into_iter()
        .map(|slot| slot.ok_or_else(|| ApiError::invalid("unused flow slot")))
        .collect()
}

fn threshold(amount_out: u64, slippage_bps: u16) -> Result<u64, ApiError> {
    tx::min_out(amount_out, slippage_bps)
        .filter(|&min_out| min_out > 0)
        .ok_or_else(|| ApiError::invalid("slippagePercent leaves no output to require"))
}

impl From<TxError> for ApiError {
    fn from(error: TxError) -> Self {
        let message = error.to_string();
        let code = match error {
            TxError::TooManyAccounts { .. } => "TOO_MANY_ACCOUNTS",
            TxError::TooLarge { .. } => "TRANSACTION_TOO_LARGE",
            TxError::TooMuchData { .. } => "TOO_MUCH_ACCOUNT_DATA",
            TxError::TooMuchCompute { .. } => "TOO_MUCH_COMPUTE",
            TxError::UnmeasuredDlmmArrays { .. } => "UNMEASURED_DLMM_WINDOW",
            TxError::UnprofitableCycle { .. } => "UNPROFITABLE_CYCLE",
            TxError::Unsupported(_) | TxError::UnknownProgram(_) => "UNSUPPORTED_VENUE",
            TxError::EmptyRoute
            | TxError::TooManyHops { .. }
            | TxError::InvalidHopThresholds
            | TxError::InvalidOptionalTail
            | TxError::Discontinuous { .. }
            | TxError::EmptyFlow
            | TxError::TooManyFlowSteps { .. }
            | TxError::TooManyFlowSlots { .. }
            | TxError::InvalidFlowShape
            | TxError::FlowSlotMismatch
            | TxError::InvalidFlowAllocation
            | TxError::Compile(_) => "CANNOT_BUILD",
        };
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, message)
    }
}

impl From<ServiceError> for ApiError {
    fn from(error: ServiceError) -> Self {
        let message = error.to_string();
        match error {
            ServiceError::UnknownMint(_) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "UNKNOWN_MINT", message)
            }
            ServiceError::Invalid(_) | ServiceError::MaxHops { .. } => Self::invalid(message),
            ServiceError::NotReady => Self::NOT_READY,
            ServiceError::StaleData { .. } => {
                Self::new(StatusCode::SERVICE_UNAVAILABLE, "STALE_DATA", message)
            }
            ServiceError::NoRoute(search) => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "NO_ROUTE",
                if search.timed_out {
                    "search deadline or cancellation ended exploration before a route was found"
                } else {
                    no_route(search.pruned, search.exhausted)
                },
            )
            .with_search(search.into()),
            ServiceError::RouteChanged(changed) => {
                tracing::debug!(%changed, "route changed while priced again");
                Self::new(StatusCode::SERVICE_UNAVAILABLE, "ROUTE_CHANGED", message)
            }
            ServiceError::QuoteExpired { .. } => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "QUOTE_EXPIRED", message)
            }
            ServiceError::QuoteMismatch(_) => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "QUOTE_MISMATCH", message)
            }
            ServiceError::NoWindow { .. } => {
                Self::new(StatusCode::UNPROCESSABLE_ENTITY, "CANNOT_SWAP", message)
            }
        }
    }
}

fn no_route(pruned: bool, exhausted: bool) -> &'static str {
    match (pruned, exhausted) {
        (false, false) => "no path connects the mints within maxHops over the admitted pools",
        (_, true) => "the quote budget ran out before a path was found; one may still exist",
        (true, false) => {
            "no path among the pools kept per pair; one may exist through a pool dropped"
        }
    }
}
