//! Mainnet pools whose mint or vault Token-2022 would not move, built from
//! captured accounts the way the route threads build them. Arrays and oracles
//! are not captured: the refusal comes before a quote needs them.

use dex::{AccountView, Known, PoolAccount, Role};
use domain::{ChainClock, DexKind, Pubkey, Slot};

use super::captured;
use crate::{AccountRef, QuoteError, QuoteInput, VenueState};

struct Captured(Vec<(Pubkey, Pubkey, Vec<u8>)>);

impl AccountView for Captured {
    fn get(&self, key: &Pubkey) -> Known<'_> {
        self.0
            .iter()
            .find(|(k, _, _)| k == key)
            .map_or(Known::Unknown, |(_, _, data)| Known::Present(data))
    }
}

fn quote(dex: DexKind, pool: &str, a_to_b: bool) -> Result<u64, QuoteError> {
    let accounts = Captured(captured());
    let address = Pubkey::from_str_const(pool);
    let Known::Present(data) = accounts.get(&address) else {
        panic!("{pool} is captured");
    };
    let closure = dex::closure(
        dex,
        &PoolAccount {
            address,
            data,
            mints: None,
        },
        &accounts,
    )
    .expect("closure");
    let mut state = VenueState::new(dex);
    for dep in closure.deps.iter().filter(|d| d.role != Role::Clock) {
        let Some((key, owner, data)) = accounts.0.iter().find(|(k, _, _)| *k == dep.pubkey) else {
            continue;
        };
        state
            .apply(&AccountRef {
                key: *key,
                role: dep.role,
                owner: *owner,
                lamports: 1,
                data,
            })
            .expect("applies");
    }
    let clock = ChainClock {
        slot: Slot(451_611_899),
        epoch_start_timestamp: 0,
        epoch: 1_045,
        leader_schedule_epoch: 1_046,
        unix_timestamp: 1_790_670_000,
    };
    state
        .quote(&QuoteInput {
            amount_in: 1_000_000,
            a_to_b,
            clock: &clock,
            max_arrays: 8,
        })
        .map(|out| out.amount_out)
}

// src: mainnet getAccountInfo jsonParsed at slot 451611899: vault E1uVFJc5… of 5MM19hAj… is frozen.
#[test]
fn a_pool_with_a_frozen_vault_refuses_both_directions() {
    for a_to_b in [true, false] {
        assert_eq!(
            quote(
                DexKind::RaydiumCpmm,
                "5MM19hAjcisF4BQHZMy2HtpNHraUk2YkUeTk5ZP7RNun",
                a_to_b
            ),
            Err(QuoteError::VaultFrozen),
            "a_to_b {a_to_b}"
        );
    }
}

// src: mainnet getAccountInfo jsonParsed at slot 451611899: Pre2Y4ga…, side A of CLMM pool Bn3ity8q…,
// has pausableConfig.paused = true.
#[test]
fn a_pool_with_a_paused_mint_refuses_both_directions() {
    for a_to_b in [true, false] {
        assert_eq!(
            quote(
                DexKind::RaydiumClmm,
                "Bn3ity8qd9XhBTMaPcxaDHeEoDkUBoKWYbMzidJPeBk7",
                a_to_b
            ),
            Err(QuoteError::MintPaused),
            "a_to_b {a_to_b}"
        );
    }
}

// src: mainnet getAccountInfo jsonParsed at slot 451611899: EMFTTUnt…, side A of Whirlpool 2ddJzwky…,
// has defaultAccountState frozen.
#[test]
fn a_mint_that_freezes_new_accounts_is_refused_only_as_the_output() {
    let pool = "2ddJzwkyNBiB3FEC14XjrQDzjfWqx74LbdavCCk19CHi";
    assert_eq!(
        quote(DexKind::OrcaWhirlpool, pool, false),
        Err(QuoteError::FrozenByDefault)
    );
    assert_ne!(
        quote(DexKind::OrcaWhirlpool, pool, true),
        Err(QuoteError::FrozenByDefault)
    );
}
