use domain::chain::is_token_program;
use spl_token_2022_interface::extension::StateWithExtensions;
use spl_token_2022_interface::state::{Account, AccountState};

use super::captured;
use crate::token::any_token_account;

const ACCOUNT_LEN: usize = 165;
const ACCOUNT_TYPE: usize = 165;
const ACCOUNT_TYPE_ACCOUNT: u8 = 2;

// src: mainnet getProgramAccounts at slot 451609640: E1uVFJc5… (SPL Token) and 23WULbkE… (Token-2022)
// are frozen vaults of CPMM pools 5MM19hAj… and BYDajRqd….
#[test]
fn captured_token_accounts_read_frozen_as_the_interface_does() {
    let accounts: Vec<_> = captured()
        .into_iter()
        .filter(|(_, owner, data)| {
            is_token_program(owner)
                && (data.len() == ACCOUNT_LEN
                    || data.get(ACCOUNT_TYPE) == Some(&ACCOUNT_TYPE_ACCOUNT))
        })
        .collect();
    let mut frozen = 0;
    let mut thawed = 0;
    for (key, owner, data) in accounts {
        let local = any_token_account(&owner, &data).unwrap_or_else(|| panic!("{key}"));
        let canonical = StateWithExtensions::<Account>::unpack(&data).expect("interface decodes");
        let expected = canonical.base.state == AccountState::Frozen;
        assert_eq!(
            (local.frozen, local.amount),
            (expected, canonical.base.amount),
            "{key}"
        );
        if expected {
            frozen += 1;
        } else {
            thawed += 1;
        }
    }
    assert!(
        frozen >= 2 && thawed >= 1,
        "{frozen} frozen, {thawed} thawed"
    );
}
