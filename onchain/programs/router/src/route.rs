use pinocchio::error::ProgramError;
use pinocchio::{AccountView, Address, ProgramResult};
use router_core::RouterError;
use router_core::route_checks::{
    check_actual_in_band, check_hop_output, check_min_out, check_route_args,
};
use router_wire::Route;

use crate::error::custom;
use crate::{config, hop, token_account};

pub fn handle(program_id: &Address, accounts: &[AccountView], route: &Route) -> ProgramResult {
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
    let mut windows = remaining;
    if !user.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let user_key = user.address().as_array();
    for ata in [source_ata, destination_ata] {
        if &token_account::wallet_owner(ata)? != user_key {
            return Err(custom(RouterError::AtaOwnerMismatch));
        }
    }
    if config::load(config_account, program_id)?.paused {
        return Err(custom(RouterError::Paused));
    }
    let circular = source_ata.address() == destination_ata.address();
    check_route_args(route.in_amount(), route.min_out(), circular).map_err(custom)?;

    let mut amount_in = route.in_amount();
    let mut previous_out = source_ata.address();
    for &step in route.hops() {
        let len = router_core::adapters::window_len(step).map_err(custom)?;
        let (window, rest) = windows
            .split_at_checked(len)
            .ok_or(custom(RouterError::WindowOutOfBounds))?;
        windows = rest;

        let built = hop::build(step, window, amount_in, user_key)?;
        let in_ata = window
            .get(built.in_ata_index)
            .ok_or(custom(RouterError::BadWindow))?;
        let out_ata = window
            .get(built.out_ata_index)
            .ok_or(custom(RouterError::BadWindow))?;
        if in_ata.address() != previous_out {
            return Err(custom(RouterError::HopContinuityViolation));
        }

        let source_before = token_account::amount(in_ata)?;
        let out_before = token_account::amount(out_ata)?;
        hop::invoke(&built.ix, window)?;
        if token_account::is_closed(in_ata) {
            return Err(custom(RouterError::ActualInOutOfBand));
        }
        check_actual_in_band(source_before, token_account::amount(in_ata)?, amount_in)
            .map_err(custom)?;
        amount_in =
            check_hop_output(out_before, token_account::amount(out_ata)?).map_err(custom)?;
        previous_out = out_ata.address();
    }

    if !windows.is_empty() {
        return Err(custom(RouterError::BadWindow));
    }
    if previous_out != destination_ata.address() {
        return Err(custom(RouterError::HopContinuityViolation));
    }
    check_min_out(amount_in, route.min_out()).map_err(custom)
}
