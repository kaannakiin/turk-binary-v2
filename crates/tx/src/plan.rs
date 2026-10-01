use std::collections::BTreeSet;

use domain::chain::NATIVE_MINT;
use domain::{Pubkey, SwapWindow, TokenSide, WindowAccount};
use router_wire::{
    FlowRoute, FlowStep, Hop, MAX_FLOW_SLOTS, MAX_FLOW_STEPS, MAX_HOPS, Route, RouterInstruction,
};
use solana_instruction::{AccountMeta, Instruction};

use crate::TxError;
use crate::budget::{Limits, limits};
use crate::router::{ROUTER_PROGRAM, hop_kind, router_config};
use crate::token::{
    TokenAccounts, close_account, create_idempotent, sync_native, transfer_lamports,
};

pub const MAX_ACCOUNTS: usize = AccountLimit::MAX.get();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountLimit(usize);

impl AccountLimit {
    // src: SIMD-0385 (a v1 transaction carries at most 64 inline addresses); AGENTS.md → Transaction format
    pub const MAX: Self = Self(64);

    #[must_use]
    pub fn new(limit: u8) -> Option<Self> {
        let limit = usize::from(limit);
        (1..=Self::MAX.0).contains(&limit).then_some(Self(limit))
    }

    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SwapRequest<'a> {
    pub wallet: &'a TokenAccounts,
    pub hops: &'a [SwapWindow],
    pub amount_in: u64,
    pub min_out: u64,
    pub hop_min_outs: &'a [u64],
    pub wrap_sol: bool,
    pub max_accounts: AccountLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowAllocation {
    pub source: u8,
    pub destination: u8,
    pub numerator: u64,
    pub denominator: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct FlowSwapRequest<'a> {
    pub wallet: &'a TokenAccounts,
    pub slots: &'a [TokenSide],
    pub windows: &'a [SwapWindow],
    pub allocations: &'a [FlowAllocation],
    pub step_min_outs: &'a [u64],
    pub amount_in: u64,
    pub min_out: u64,
    pub wrap_sol: bool,
    pub max_accounts: AccountLimit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapInstructions {
    pub setup: Vec<Instruction>,
    pub swap: Instruction,
    pub cleanup: Vec<Instruction>,
    pub limits: Limits,
}

impl SwapInstructions {
    pub(crate) fn all(&self) -> impl Iterator<Item = &Instruction> {
        self.setup
            .iter()
            .chain(std::iter::once(&self.swap))
            .chain(&self.cleanup)
    }

    fn account_count(&self, fee_payer: &Pubkey) -> usize {
        let mut keys: BTreeSet<&Pubkey> = BTreeSet::from([fee_payer]);
        for instruction in self.all() {
            keys.insert(&instruction.program_id);
            keys.extend(instruction.accounts.iter().map(|meta| &meta.pubkey));
        }
        keys.len()
    }

    fn within(self, user: &Pubkey, limit: AccountLimit) -> Result<Self, TxError> {
        let count = self.account_count(user);
        if count > limit.get() {
            return Err(TxError::TooManyAccounts {
                count,
                max: limit.get(),
            });
        }
        crate::unsigned_v1(&self, user, [0; 32], u64::MAX)?;
        Ok(self)
    }
}

pub fn build(request: &SwapRequest) -> Result<SwapInstructions, TxError> {
    endpoints(request.hops)?;
    if request.hops.iter().any(|hop| {
        hop.optional_tail > (hop.tail & 0x7f) || usize::from(hop.optional_tail) > hop.accounts.len()
    }) {
        return Err(TxError::InvalidOptionalTail);
    }
    if request.hop_min_outs.len() != request.hops.len()
        || request.hop_min_outs.contains(&0)
        || request
            .hop_min_outs
            .last()
            .is_some_and(|&last| last < request.min_out)
    {
        return Err(TxError::InvalidHopThresholds);
    }
    match build_once(request, request.hops) {
        Err(TxError::TooManyAccounts { .. } | TxError::TooLarge { .. })
            if request.hops.iter().any(|hop| hop.optional_tail > 0) =>
        {
            let mut trimmed = request.hops.to_vec();
            for hop in &mut trimmed {
                if hop.optional_tail > 0 {
                    let remove = usize::from(hop.optional_tail);
                    hop.accounts.truncate(hop.accounts.len() - remove);
                    hop.tail -= hop.optional_tail;
                    hop.optional_tail = 0;
                }
            }
            build_once(request, &trimmed)
        }
        result => result,
    }
}

pub fn build_flow(request: &FlowSwapRequest) -> Result<SwapInstructions, TxError> {
    validate_flow_request(request, request.windows)?;
    match build_flow_once(request, request.windows) {
        Err(TxError::TooManyAccounts { .. } | TxError::TooLarge { .. })
            if request
                .windows
                .iter()
                .any(|window| window.optional_tail > 0) =>
        {
            let mut trimmed = request.windows.to_vec();
            for window in &mut trimmed {
                if window.optional_tail > 0 {
                    let remove = usize::from(window.optional_tail);
                    window.accounts.truncate(window.accounts.len() - remove);
                    window.tail -= window.optional_tail;
                    window.optional_tail = 0;
                }
            }
            build_flow_once(request, &trimmed)
        }
        result => result,
    }
}

fn validate_flow_request(request: &FlowSwapRequest, windows: &[SwapWindow]) -> Result<(), TxError> {
    if request.slots.len() < 2 {
        return Err(TxError::TooManyFlowSlots {
            slots: request.slots.len(),
            max: MAX_FLOW_SLOTS,
        });
    }
    if request.slots.len() > MAX_FLOW_SLOTS {
        return Err(TxError::TooManyFlowSlots {
            slots: request.slots.len(),
            max: MAX_FLOW_SLOTS,
        });
    }
    if windows.is_empty() {
        return Err(TxError::EmptyFlow);
    }
    if windows.len() > MAX_FLOW_STEPS
        || request.allocations.len() != windows.len()
        || request.step_min_outs.len() != windows.len()
    {
        if windows.len() > MAX_FLOW_STEPS {
            return Err(TxError::TooManyFlowSteps {
                steps: windows.len(),
                max: MAX_FLOW_STEPS,
            });
        }
        return Err(TxError::InvalidFlowShape);
    }
    for ((allocation, window), &min_out) in request
        .allocations
        .iter()
        .zip(windows)
        .zip(request.step_min_outs)
    {
        let source = request
            .slots
            .get(usize::from(allocation.source))
            .ok_or(TxError::InvalidFlowAllocation)?;
        let destination = request
            .slots
            .get(usize::from(allocation.destination))
            .ok_or(TxError::InvalidFlowAllocation)?;
        if allocation.source == allocation.destination
            || allocation.destination == 0
            || allocation.source == 1
            || allocation.denominator == 0
            || allocation.numerator == 0
            || allocation.numerator > allocation.denominator
        {
            return Err(TxError::InvalidFlowAllocation);
        }
        if window.source != *source || window.destination != *destination {
            return Err(TxError::FlowSlotMismatch);
        }
        if min_out == 0 {
            return Err(TxError::InvalidHopThresholds);
        }
        if window.optional_tail > (window.tail & 0x7f)
            || usize::from(window.optional_tail) > window.accounts.len()
        {
            return Err(TxError::InvalidOptionalTail);
        }
    }
    Ok(())
}

fn build_flow_once(
    request: &FlowSwapRequest,
    windows: &[SwapWindow],
) -> Result<SwapInstructions, TxError> {
    validate_flow_request(request, windows)?;
    if request.slots[0].mint == request.slots[1].mint && request.min_out <= request.amount_in {
        return Err(TxError::UnprofitableCycle {
            amount_in: request.amount_in,
            min_out: request.min_out,
        });
    }
    let wallet = request.wallet;
    let user = wallet.owner();
    let wraps_in = request.wrap_sol && request.slots[0].mint == NATIVE_MINT;
    let wraps_out = request.wrap_sol && request.slots[1].mint == NATIVE_MINT;

    let setup = setup_instructions(
        wallet,
        request.slots[0],
        request.slots.iter().copied(),
        request.amount_in,
        request.wrap_sol,
    );
    let cleanup = cleanup_instructions(
        wallet,
        [(wraps_in, request.slots[0]), (wraps_out, request.slots[1])],
    );

    let swap = flow_instruction(request, windows)?;
    let limits = limits(
        windows,
        setup.iter().chain(std::iter::once(&swap)).chain(&cleanup),
        user,
    )?;
    SwapInstructions {
        setup,
        swap,
        cleanup,
        limits,
    }
    .within(user, request.max_accounts)
}

fn build_once(request: &SwapRequest, hops: &[SwapWindow]) -> Result<SwapInstructions, TxError> {
    let (first, last) = endpoints(hops)?;
    // The router refuses a cycle whose threshold does not exceed its input before any swap
    // runs (router-core `check_route_args`); building one would hand out a transaction that
    // can only fail.
    if first.source == last.destination && request.min_out <= request.amount_in {
        return Err(TxError::UnprofitableCycle {
            amount_in: request.amount_in,
            min_out: request.min_out,
        });
    }
    let wallet = request.wallet;
    let user = wallet.owner();
    let wraps_in = request.wrap_sol && first.source.mint == NATIVE_MINT;
    let wraps_out = request.wrap_sol && last.destination.mint == NATIVE_MINT;

    let setup = setup_instructions(
        wallet,
        first.source,
        hops.iter().map(|hop| hop.destination),
        request.amount_in,
        request.wrap_sol,
    );
    let cleanup = cleanup_instructions(
        wallet,
        [(wraps_in, first.source), (wraps_out, last.destination)],
    );

    let swap = route_instruction(request, hops, first, last)?;
    let limits = limits(
        hops,
        setup.iter().chain(std::iter::once(&swap)).chain(&cleanup),
        user,
    )?;
    SwapInstructions {
        setup,
        swap,
        cleanup,
        limits,
    }
    .within(user, request.max_accounts)
}

fn endpoints(hops: &[SwapWindow]) -> Result<(&SwapWindow, &SwapWindow), TxError> {
    let (Some(first), Some(last)) = (hops.first(), hops.last()) else {
        return Err(TxError::EmptyRoute);
    };
    if hops.len() > MAX_HOPS {
        return Err(TxError::TooManyHops {
            hops: hops.len(),
            max: MAX_HOPS,
        });
    }
    if let Some(hop) = hops
        .windows(2)
        .position(|pair| pair[0].destination != pair[1].source)
    {
        return Err(TxError::Discontinuous { hop: hop + 1 });
    }
    Ok((first, last))
}

fn setup_instructions<I>(
    wallet: &TokenAccounts,
    root: TokenSide,
    destinations: I,
    amount_in: u64,
    wrap_sol: bool,
) -> Vec<Instruction>
where
    I: IntoIterator<Item = TokenSide>,
{
    let user = wallet.owner();
    let wraps_in = wrap_sol && root.mint == NATIVE_MINT;
    let mut setup = Vec::new();
    let mut created = BTreeSet::new();
    if wraps_in {
        let wsol = wallet.of(&root);
        setup.push(create_idempotent(user, wallet, &root));
        setup.push(transfer_lamports(user, &wsol, amount_in));
        setup.push(sync_native(&wsol));
        created.insert(root);
    }
    for side in destinations {
        if created.insert(side) {
            setup.push(create_idempotent(user, wallet, &side));
        }
    }
    setup
}

fn cleanup_instructions<const N: usize>(
    wallet: &TokenAccounts,
    wrapped: [(bool, TokenSide); N],
) -> Vec<Instruction> {
    let user = wallet.owner();
    wrapped
        .into_iter()
        .filter(|(is_wrapped, _)| *is_wrapped)
        .map(|(_, side)| wallet.of(&side))
        .collect::<BTreeSet<_>>()
        .iter()
        .map(|wsol| close_account(wsol, user, user))
        .collect()
}

fn route_instruction(
    request: &SwapRequest,
    hops: &[SwapWindow],
    first: &SwapWindow,
    last: &SwapWindow,
) -> Result<Instruction, TxError> {
    let wallet = request.wallet;
    let user = *wallet.owner();
    let mut accounts = vec![
        AccountMeta::new(user, true),
        AccountMeta::new(wallet.of(&first.source), false),
        AccountMeta::new(wallet.of(&last.destination), false),
        AccountMeta::new_readonly(router_config(), false),
    ];
    let mut plan = Vec::with_capacity(hops.len());
    for (hop, &min_out) in hops.iter().zip(request.hop_min_outs) {
        plan.push(Hop {
            kind: hop_kind(hop.kind)?.into(),
            hook_a: 0,
            hook_b: 0,
            tail: hop.tail,
            min_out,
        });
        let (source, destination) = (wallet.of(&hop.source), wallet.of(&hop.destination));
        accounts.push(AccountMeta::new_readonly(hop.program_id, false));
        accounts.extend(hop.accounts.iter().map(|account| match *account {
            WindowAccount::User => AccountMeta::new(user, true),
            WindowAccount::UserSource => AccountMeta::new(source, false),
            WindowAccount::UserDestination => AccountMeta::new(destination, false),
            WindowAccount::Fixed {
                key,
                writable: true,
            } => AccountMeta::new(key, false),
            WindowAccount::Fixed {
                key,
                writable: false,
            } => AccountMeta::new_readonly(key, false),
        }));
    }
    let route = Route::new(request.amount_in, request.min_out, &plan).map_err(|_| {
        TxError::TooManyHops {
            hops: plan.len(),
            max: MAX_HOPS,
        }
    })?;
    Ok(Instruction {
        program_id: ROUTER_PROGRAM,
        accounts,
        data: RouterInstruction::Route(route).encode(),
    })
}

fn flow_instruction(
    request: &FlowSwapRequest,
    windows: &[SwapWindow],
) -> Result<Instruction, TxError> {
    let wallet = request.wallet;
    let user = *wallet.owner();
    let mut accounts = vec![
        AccountMeta::new(user, true),
        AccountMeta::new(wallet.of(&request.slots[0]), false),
        AccountMeta::new(wallet.of(&request.slots[1]), false),
        AccountMeta::new_readonly(router_config(), false),
    ];
    accounts.extend(
        request
            .slots
            .iter()
            .map(|side| AccountMeta::new(wallet.of(side), false)),
    );

    let mut steps = Vec::with_capacity(windows.len());
    for ((allocation, window), &min_out) in request
        .allocations
        .iter()
        .zip(windows)
        .zip(request.step_min_outs)
    {
        let source = request
            .slots
            .get(usize::from(allocation.source))
            .ok_or(TxError::InvalidFlowAllocation)?;
        let destination = request
            .slots
            .get(usize::from(allocation.destination))
            .ok_or(TxError::InvalidFlowAllocation)?;
        steps.push(FlowStep {
            source_slot: allocation.source,
            destination_slot: allocation.destination,
            numerator: allocation.numerator,
            denominator: allocation.denominator,
            hop: Hop {
                kind: hop_kind(window.kind)?.into(),
                hook_a: 0,
                hook_b: 0,
                tail: window.tail,
                min_out,
            },
        });

        let source_ata = wallet.of(source);
        let destination_ata = wallet.of(destination);
        accounts.push(AccountMeta::new_readonly(window.program_id, false));
        accounts.extend(window.accounts.iter().map(|account| match *account {
            WindowAccount::User => AccountMeta::new(user, true),
            WindowAccount::UserSource => AccountMeta::new(source_ata, false),
            WindowAccount::UserDestination => AccountMeta::new(destination_ata, false),
            WindowAccount::Fixed {
                key,
                writable: true,
            } => AccountMeta::new(key, false),
            WindowAccount::Fixed {
                key,
                writable: false,
            } => AccountMeta::new_readonly(key, false),
        }));
    }
    let route = FlowRoute::new(
        request.amount_in,
        request.min_out,
        request.slots.len(),
        &steps,
    )
    .map_err(|_| TxError::InvalidFlowAllocation)?;
    Ok(Instruction {
        program_id: ROUTER_PROGRAM,
        accounts,
        data: RouterInstruction::Flow(route).encode(),
    })
}
