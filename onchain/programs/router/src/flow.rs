use pinocchio::error::ProgramError;
use pinocchio::{AccountView, Address, ProgramResult};
use router_core::RouterError;
use router_core::route_checks::{
    check_actual_in_band, check_hop_output, check_min_out, check_route_args,
};
use router_wire::{FlowRoute, MAX_FLOW_SLOTS};

use crate::error::custom;
use crate::{config, hop, token_account};

fn allocated_amount(available: u64, numerator: u64, denominator: u64) -> Result<u64, ProgramError> {
    if denominator == 0 || numerator == 0 || numerator > denominator {
        return Err(custom(RouterError::BadFlow));
    }
    if numerator == denominator {
        return Ok(available);
    }
    let amount = u128::from(available)
        .checked_mul(u128::from(numerator))
        .ok_or(custom(RouterError::FlowBalance))?
        .checked_div(u128::from(denominator))
        .ok_or(custom(RouterError::FlowBalance))?;
    u64::try_from(amount).map_err(|_| custom(RouterError::FlowBalance))
}

fn ensure_final_credits(balances: &[u64]) -> ProgramResult {
    if balances
        .iter()
        .enumerate()
        .any(|(slot, &balance)| slot != 1 && balance != 0)
    {
        return Err(custom(RouterError::FlowBalance));
    }
    Ok(())
}

pub fn handle(program_id: &Address, accounts: &[AccountView], route: &FlowRoute) -> ProgramResult {
    let [
        user,
        source_ata,
        destination_ata,
        config_account,
        remaining @ ..,
    ] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    let slot_count = route.slot_count();
    let (slots, mut windows) = remaining
        .split_at_checked(slot_count)
        .ok_or(custom(RouterError::WindowOutOfBounds))?;
    if slots.len() < 2
        || slots[0].address() != source_ata.address()
        || slots[1].address() != destination_ata.address()
    {
        return Err(custom(RouterError::FlowSlotAccountMismatch));
    }
    if !user.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let user_key = user.address().as_array();
    for account in slots {
        if &token_account::wallet_owner(account)? != user_key {
            return Err(custom(RouterError::AtaOwnerMismatch));
        }
    }
    if config::load(config_account, program_id)?.paused {
        return Err(custom(RouterError::Paused));
    }
    let circular = token_account::mint(source_ata)? == token_account::mint(destination_ata)?;
    check_route_args(route.in_amount(), route.min_out(), circular).map_err(custom)?;

    let mut balances = [0u64; MAX_FLOW_SLOTS];
    balances[0] = route.in_amount();
    for step in route.steps() {
        let source = usize::from(step.source_slot);
        let destination = usize::from(step.destination_slot);
        let available = balances
            .get(source)
            .copied()
            .ok_or(custom(RouterError::BadFlow))?;
        let amount_in = allocated_amount(available, step.numerator, step.denominator)?;
        if amount_in == 0 {
            return Err(custom(RouterError::BadFlow));
        }
        let len = router_core::adapters::window_len(step.hop).map_err(custom)?;
        let (window, rest) = windows
            .split_at_checked(len)
            .ok_or(custom(RouterError::WindowOutOfBounds))?;
        windows = rest;

        let source_account = slots.get(source).ok_or(custom(RouterError::BadFlow))?;
        let destination_account = slots.get(destination).ok_or(custom(RouterError::BadFlow))?;
        let built = hop::build(
            step.hop,
            window,
            amount_in,
            user_key,
            source_account.address().as_array(),
        )?;
        let in_ata = window
            .get(built.in_ata_index)
            .ok_or(custom(RouterError::BadWindow))?;
        let out_ata = window
            .get(built.out_ata_index)
            .ok_or(custom(RouterError::BadWindow))?;
        if in_ata.address() != source_account.address()
            || out_ata.address() != destination_account.address()
        {
            return Err(custom(RouterError::FlowSlotAccountMismatch));
        }

        let source_before = token_account::amount(in_ata)?;
        let output_before = token_account::amount(out_ata)?;
        hop::invoke(&built.ix, window)?;
        if token_account::is_closed(in_ata) {
            return Err(custom(RouterError::ActualInOutOfBand));
        }
        let actual_in =
            check_actual_in_band(source_before, token_account::amount(in_ata)?, amount_in)
                .map_err(custom)?;
        if actual_in != amount_in {
            return Err(custom(RouterError::ActualInOutOfBand));
        }
        let actual_out =
            check_hop_output(output_before, token_account::amount(out_ata)?).map_err(custom)?;
        check_min_out(actual_out, step.hop.min_out).map_err(custom)?;
        balances[source] = balances[source]
            .checked_sub(actual_in)
            .ok_or(custom(RouterError::FlowBalance))?;
        balances[destination] = balances[destination]
            .checked_add(actual_out)
            .ok_or(custom(RouterError::FlowBalance))?;
    }
    if !windows.is_empty() {
        return Err(custom(RouterError::BadWindow));
    }
    ensure_final_credits(&balances[..slot_count])?;
    check_min_out(balances[1], route.min_out()).map_err(custom)
}

#[cfg(test)]
mod tests {
    use super::allocated_amount;

    #[test]
    fn partial_flow_allocation_floors_and_full_allocation_consumes_remainder() {
        assert_eq!(allocated_amount(10, 1, 3).unwrap(), 3);
        assert_eq!(allocated_amount(10, 2, 3).unwrap(), 6);
        assert_eq!(allocated_amount(10, 1, 1).unwrap(), 10);
    }
}
