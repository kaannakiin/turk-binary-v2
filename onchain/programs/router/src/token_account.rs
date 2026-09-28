use pinocchio::AccountView;
use pinocchio::error::ProgramError;
use router_core::{HopAccountView, RouterError};

use crate::error::custom;

fn read<T>(
    view: &AccountView,
    field: impl FnOnce(&HopAccountView) -> Result<T, RouterError>,
) -> Result<T, ProgramError> {
    let data = view.try_borrow()?;
    field(&HopAccountView {
        key: view.address().as_array(),
        owner: view.owner().as_array(),
        is_signer: view.is_signer(),
        is_writable: view.is_writable(),
        data: &data,
    })
    .map_err(custom)
}

pub fn amount(view: &AccountView) -> Result<u64, ProgramError> {
    read(view, router_core::token_account::amount)
}

pub fn wallet_owner(view: &AccountView) -> Result<[u8; 32], ProgramError> {
    read(view, router_core::token_account::wallet_owner)
}

pub fn is_closed(view: &AccountView) -> bool {
    view.lamports() == 0 || view.is_data_empty()
}
