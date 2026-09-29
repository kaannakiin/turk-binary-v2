use std::collections::BTreeSet;

use domain::chain::NATIVE_MINT;
use domain::{Pubkey, SwapWindow, TokenSide, WindowAccount};
use router_wire::{Hop, MAX_HOPS, Route, RouterInstruction};
use solana_instruction::{AccountMeta, Instruction};

use crate::TxError;
use crate::budget::{Limits, limits};
use crate::router::{ROUTER_PROGRAM, hop_kind, router_config};
use crate::token::{
    associated_token_address, close_account, create_idempotent, sync_native, transfer_lamports,
};

// src: SIMD-0385 (a v1 transaction carries at most 64 inline addresses); AGENTS.md → Transaction format
pub const MAX_ACCOUNTS: usize = 64;

#[derive(Debug, Clone, Copy)]
pub struct SwapRequest<'a> {
    pub user: Pubkey,
    pub hops: &'a [SwapWindow],
    pub amount_in: u64,
    pub min_out: u64,
    pub hop_min_outs: &'a [u64],
    pub wrap_sol: bool,
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
    let user = &request.user;
    let wraps_in = request.wrap_sol && first.source.mint == NATIVE_MINT;
    let wraps_out = request.wrap_sol && last.destination.mint == NATIVE_MINT;

    let mut setup = Vec::new();
    let mut created = BTreeSet::new();
    if wraps_in {
        let wsol = ata(user, &first.source);
        setup.push(create_idempotent(
            user,
            user,
            &NATIVE_MINT,
            &first.source.token_program,
        ));
        setup.push(transfer_lamports(user, &wsol, request.amount_in));
        setup.push(sync_native(&wsol));
        created.insert(first.source);
    }
    for side in hops.iter().map(|hop| hop.destination) {
        if created.insert(side) {
            setup.push(create_idempotent(
                user,
                user,
                &side.mint,
                &side.token_program,
            ));
        }
    }

    let cleanup = [(wraps_in, &first.source), (wraps_out, &last.destination)]
        .into_iter()
        .filter(|&(wraps, _)| wraps)
        .map(|(_, side)| ata(user, side))
        .collect::<BTreeSet<_>>()
        .iter()
        .map(|wsol| close_account(wsol, user, user))
        .collect();

    let swap = route_instruction(request, hops, first, last)?;
    let limits = limits(
        hops,
        setup.iter().chain(std::iter::once(&swap)).chain(&cleanup),
        user,
    )?;
    let instructions = SwapInstructions {
        setup,
        swap,
        cleanup,
        limits,
    };
    let count = instructions.account_count(user);
    if count > MAX_ACCOUNTS {
        return Err(TxError::TooManyAccounts {
            count,
            max: MAX_ACCOUNTS,
        });
    }
    crate::unsigned_v1(&instructions, user, [0; 32], u64::MAX)?;
    Ok(instructions)
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

fn ata(user: &Pubkey, side: &TokenSide) -> Pubkey {
    associated_token_address(user, &side.mint, &side.token_program)
}

fn route_instruction(
    request: &SwapRequest,
    hops: &[SwapWindow],
    first: &SwapWindow,
    last: &SwapWindow,
) -> Result<Instruction, TxError> {
    let user = request.user;
    let mut accounts = vec![
        AccountMeta::new(user, true),
        AccountMeta::new(ata(&user, &first.source), false),
        AccountMeta::new(ata(&user, &last.destination), false),
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
        let (source, destination) = (ata(&user, &hop.source), ata(&user, &hop.destination));
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
