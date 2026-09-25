//! The pools below were captured together with every account their closure
//! names: all tick/bin arrays the bitmaps mark (CLMM, DLMM) and every
//! possible tick-array PDA (Whirlpool, 2522 at spacing 4, 362 existing).
//! The expected sets were derived by an independent Python port.

use std::collections::BTreeSet;

use domain::{DexKind, Pubkey};

use super::accounts::fixtures;
use crate::{
    Closure, Known, Need, NoAccounts, PoolAccount, Presence, Role, closure, discovery_filters,
    spec, structural_ranges,
};

const CLMM_POOL: Pubkey = Pubkey::from_str_const("3ucNos4NbumPLZNWztqGHNFFgkHeRMBQAVemeeomsUxv");
const DLMM_POOL: Pubkey = Pubkey::from_str_const("HTvjzsfX3yU6BUodCjZ5vZkUrAxMDTrBs3CJaq43ashR");
const WHIRLPOOL: Pubkey = Pubkey::from_str_const("Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE");
const ADAPTIVE_WHIRLPOOL: Pubkey =
    Pubkey::from_str_const("31KrYUDzgEQhEgr1JSNVfHAknWACcF97CtUaU8enKQsy");
// src: orca-so/whirlpools@408c945fef4c49ab70def4303377cfaf8f0f3c99 rust-sdk/client/src/generated/accounts/oracle.rs (ORACLE_DISCRIMINATOR)
const ORACLE_DISCRIMINATOR: [u8; 8] = [139, 194, 131, 179, 140, 179, 229, 244];

fn pool_data(pool: &Pubkey) -> &'static [u8] {
    fixtures().account(pool).unwrap().bytes()
}

fn derive(kind: DexKind, pool: Pubkey, data: &[u8]) -> Closure {
    let account = PoolAccount {
        address: pool,
        data,
        mints: None,
    };
    closure(kind, &account, fixtures()).unwrap()
}

fn array_keys(closure: &Closure) -> BTreeSet<Pubkey> {
    closure
        .deps
        .iter()
        .filter(|d| matches!(d.role, Role::TickArray { .. } | Role::BinArray { .. }))
        .map(|d| d.pubkey)
        .collect()
}

fn existing_arrays(kind: DexKind, pool: &Pubkey, disc: [u8; 8]) -> BTreeSet<Pubkey> {
    let owner = spec(kind).program_id;
    fixtures()
        .owned_by(&owner)
        .filter(|key| *key != pool)
        .filter(|key| {
            fixtures()
                .account(key)
                .and_then(|a| a.data.as_deref())
                .is_none_or(|data| data.get(..8) == Some(disc.as_slice()))
        })
        .copied()
        .collect()
}

fn role_of(closure: &Closure, key: &Pubkey) -> Role {
    closure.deps.iter().find(|d| &d.pubkey == key).unwrap().role
}

#[test]
fn clmm_closure_names_every_initialized_tick_array() {
    let closure = derive(DexKind::RaydiumClmm, CLMM_POOL, pool_data(&CLMM_POOL));
    let arrays = array_keys(&closure);
    assert_eq!(arrays.len(), 417);
    assert!(arrays.iter().all(|key| fixtures().account(key).is_some()));
}

#[test]
fn clmm_extension_arrays_decode_to_their_stored_start_index() {
    let closure = derive(DexKind::RaydiumClmm, CLMM_POOL, pool_data(&CLMM_POOL));
    for dep in closure
        .deps
        .iter()
        .filter(|d| matches!(d.role, Role::TickArray { .. }))
    {
        let Some(data) = fixtures().account(&dep.pubkey).unwrap().data.as_deref() else {
            continue;
        };
        let Role::TickArray { start } = dep.role else {
            unreachable!()
        };
        assert_eq!(&data[8..40], CLMM_POOL.as_ref());
        assert_eq!(i32::from_le_bytes(data[40..44].try_into().unwrap()), start);
    }
}

#[test]
fn clmm_closure_includes_extension_only_arrays_on_both_sides() {
    let closure = derive(DexKind::RaydiumClmm, CLMM_POOL, pool_data(&CLMM_POOL));
    let starts: BTreeSet<i32> = closure
        .deps
        .iter()
        .filter_map(|d| match d.role {
            Role::TickArray { start } => Some(start),
            _ => None,
        })
        .collect();
    assert!(
        [-443_640, -276_360, 46_020]
            .iter()
            .all(|s| starts.contains(s))
    );
}

#[test]
fn clmm_waits_for_an_unknown_extension() {
    let account = PoolAccount {
        address: CLMM_POOL,
        data: pool_data(&CLMM_POOL),
        mints: None,
    };
    let closure = closure(DexKind::RaydiumClmm, &account, &NoAccounts).unwrap();
    let extension = closure
        .deps
        .iter()
        .find(|d| d.role == Role::TickArrayBitmapExtension)
        .unwrap();
    assert_eq!(closure.awaiting, [extension.pubkey]);
}

#[test]
fn clmm_extension_is_optional_and_structural() {
    let closure = derive(DexKind::RaydiumClmm, CLMM_POOL, pool_data(&CLMM_POOL));
    let extension = closure
        .deps
        .iter()
        .find(|d| d.role == Role::TickArrayBitmapExtension)
        .unwrap();
    assert_eq!(
        (extension.presence, extension.structural),
        (Presence::Optional, true)
    );
}

#[test]
fn setting_a_clmm_bitmap_bit_adds_exactly_its_tick_array() {
    let original = pool_data(&CLMM_POOL);
    let before = array_keys(&derive(DexKind::RaydiumClmm, CLMM_POOL, original));
    let mut mutated = original.to_vec();
    let bit = (0..1024)
        .find(|bit| mutated[904 + bit / 8] & (1 << (bit % 8)) == 0)
        .unwrap();
    mutated[904 + bit / 8] |= 1 << (bit % 8);
    let after = array_keys(&derive(DexKind::RaydiumClmm, CLMM_POOL, &mutated));
    assert_eq!(after.difference(&before).count(), 1);
}

#[test]
fn dlmm_closure_names_every_bin_array_with_liquidity() {
    let closure = derive(DexKind::MeteoraDlmm, DLMM_POOL, pool_data(&DLMM_POOL));
    let arrays = array_keys(&closure);
    assert_eq!(arrays.len(), 183);
    assert!(arrays.iter().all(|key| fixtures().account(key).is_some()));
}

#[test]
fn dlmm_sample_bin_array_decodes_to_its_role() {
    let closure = derive(DexKind::MeteoraDlmm, DLMM_POOL, pool_data(&DLMM_POOL));
    let sample = closure
        .deps
        .iter()
        .find(|d| d.role == Role::BinArray { index: -307 })
        .unwrap();
    let data = fixtures().account(&sample.pubkey).unwrap().bytes();
    assert_eq!(
        (
            &data[24..56],
            i64::from_le_bytes(data[8..16].try_into().unwrap())
        ),
        (DLMM_POOL.as_ref(), -307)
    );
}

#[test]
fn dlmm_reserves_and_oracle_are_swap_only() {
    let closure = derive(DexKind::MeteoraDlmm, DLMM_POOL, pool_data(&DLMM_POOL));
    let swap_only: Vec<Role> = closure
        .deps
        .iter()
        .filter(|d| d.need == Need::Swap)
        .map(|d| d.role)
        .collect();
    assert_eq!(swap_only.len(), 3);
    assert!(swap_only.contains(&Role::Oracle));
}

#[test]
fn whirlpool_closure_covers_every_existing_tick_array_of_both_kinds() {
    let closure = derive(DexKind::OrcaWhirlpool, WHIRLPOOL, pool_data(&WHIRLPOOL));
    let arrays = array_keys(&closure);
    let existing: BTreeSet<Pubkey> = fixtures()
        .owned_by(&spec(DexKind::OrcaWhirlpool).program_id)
        .filter(|key| ![WHIRLPOOL, ADAPTIVE_WHIRLPOOL].contains(key))
        .filter(|key| {
            fixtures()
                .account(key)
                .and_then(|a| a.data.as_deref())
                .is_none_or(|data| !data.starts_with(&ORACLE_DISCRIMINATOR))
        })
        .copied()
        .collect();
    assert_eq!((arrays.len(), existing.len()), (2522, 362));
    assert!(existing.is_subset(&arrays));
}

#[test]
fn whirlpool_tick_arrays_are_optional() {
    let closure = derive(DexKind::OrcaWhirlpool, WHIRLPOOL, pool_data(&WHIRLPOOL));
    assert!(
        closure
            .deps
            .iter()
            .filter(|d| matches!(d.role, Role::TickArray { .. }))
            .all(|d| d.presence == Presence::Optional)
    );
}

#[test]
fn whirlpool_sample_arrays_store_their_role_start() {
    let closure = derive(DexKind::OrcaWhirlpool, WHIRLPOOL, pool_data(&WHIRLPOOL));
    for (key, data) in closure.deps.iter().filter_map(|d| {
        let data = fixtures().account(&d.pubkey)?.data.as_deref()?;
        matches!(d.role, Role::TickArray { .. }).then_some((d.pubkey, data))
    }) {
        let Role::TickArray { start } = role_of(&closure, &key) else {
            unreachable!()
        };
        assert_eq!(i32::from_le_bytes(data[8..12].try_into().unwrap()), start);
    }
}

#[test]
fn whirlpool_discovery_filters_match_both_array_kinds() {
    let filters = discovery_filters(DexKind::OrcaWhirlpool, &WHIRLPOOL);
    let program = spec(DexKind::OrcaWhirlpool).program_id;
    let fixed = existing_arrays(
        DexKind::OrcaWhirlpool,
        &WHIRLPOOL,
        [0x45, 0x61, 0xbd, 0xbe, 0x6e, 0x07, 0x42, 0xbb],
    );
    let dynamic = existing_arrays(
        DexKind::OrcaWhirlpool,
        &WHIRLPOOL,
        [0x11, 0xd8, 0xf6, 0x8e, 0xe1, 0xc7, 0xda, 0x38],
    );
    let matched = |set: &BTreeSet<Pubkey>| -> usize {
        set.iter()
            .filter_map(|k| fixtures().account(k)?.data.as_deref())
            .filter(|data| filters.iter().any(|f| f.matches(&program, data)))
            .count()
    };
    let kept = |set: &BTreeSet<Pubkey>| -> usize {
        set.iter()
            .filter(|k| fixtures().account(k).is_some_and(|a| a.data.is_some()))
            .count()
    };
    assert_eq!(
        (matched(&fixed), matched(&dynamic)),
        (kept(&fixed), kept(&dynamic))
    );
    assert!(kept(&fixed) > 0 && kept(&dynamic) > 0);
}

#[test]
fn whirlpool_static_fee_pool_does_not_quote_on_the_oracle() {
    let closure = derive(DexKind::OrcaWhirlpool, WHIRLPOOL, pool_data(&WHIRLPOOL));
    let oracle = closure
        .deps
        .iter()
        .find(|d| d.role == Role::Oracle)
        .unwrap();
    assert_eq!(
        (oracle.presence, oracle.need),
        (Presence::Optional, Need::Swap)
    );
}

#[test]
fn whirlpool_adaptive_fee_pool_requires_its_live_oracle() {
    let closure = derive(
        DexKind::OrcaWhirlpool,
        ADAPTIVE_WHIRLPOOL,
        pool_data(&ADAPTIVE_WHIRLPOOL),
    );
    let oracle = closure
        .deps
        .iter()
        .find(|d| d.role == Role::Oracle)
        .unwrap();
    let account = fixtures().account(&oracle.pubkey).unwrap();
    let data = account.bytes();
    assert_eq!(
        (
            oracle.presence,
            oracle.need,
            oracle.owner.accepts(&account.owner),
            &data[..8],
            &data[8..40],
        ),
        (
            Presence::Required,
            Need::Quote,
            true,
            ORACLE_DISCRIMINATOR.as_slice(),
            ADAPTIVE_WHIRLPOOL.as_ref(),
        )
    );
}

#[test]
fn every_required_dependency_of_the_complex_pools_exists_with_an_accepted_owner() {
    let pools = [
        (DexKind::RaydiumClmm, CLMM_POOL),
        (DexKind::MeteoraDlmm, DLMM_POOL),
        (DexKind::OrcaWhirlpool, WHIRLPOOL),
    ];
    for (kind, pool) in pools {
        let closure = derive(kind, pool, pool_data(&pool));
        for dep in closure
            .deps
            .iter()
            .filter(|d| d.presence == Presence::Required)
        {
            let account = fixtures()
                .account(&dep.pubkey)
                .unwrap_or_else(|| panic!("{kind} {:?} missing", dep.role));
            assert!(dep.owner.accepts(&account.owner), "{kind} {:?}", dep.role);
        }
    }
}

#[test]
fn every_optional_dependency_that_exists_has_an_accepted_owner() {
    let closure = derive(DexKind::OrcaWhirlpool, WHIRLPOOL, pool_data(&WHIRLPOOL));
    for dep in closure
        .deps
        .iter()
        .filter(|d| d.presence == Presence::Optional)
    {
        if let Some(account) = fixtures().account(&dep.pubkey) {
            assert!(dep.owner.accepts(&account.owner), "{:?}", dep.role);
        }
    }
}

#[test]
fn bytes_outside_structural_ranges_do_not_change_complex_closures() {
    let damm_v1 = super::damm_v1::CASES
        .iter()
        .map(|(pool, _)| (DexKind::MeteoraDammV1, Pubkey::from_str_const(pool)));
    for (kind, pool) in [
        (DexKind::RaydiumClmm, CLMM_POOL),
        (DexKind::MeteoraDlmm, DLMM_POOL),
        (DexKind::OrcaWhirlpool, WHIRLPOOL),
    ]
    .into_iter()
    .chain(damm_v1)
    {
        let original = pool_data(&pool);
        let ranges = structural_ranges(kind, &Role::Pool);
        let mut mutated = original.to_vec();
        for (i, byte) in mutated.iter_mut().enumerate().skip(8) {
            if !ranges.iter().any(|r| r.contains(&i)) {
                *byte ^= 0xff;
            }
        }
        assert_eq!(
            derive(kind, pool, &mutated),
            derive(kind, pool, original),
            "{kind}"
        );
    }
}

#[test]
fn unknown_array_bytes_are_reported_as_unknown_not_absent() {
    let unkept = array_keys(&derive(
        DexKind::MeteoraDlmm,
        DLMM_POOL,
        pool_data(&DLMM_POOL),
    ))
    .into_iter()
    .find(|k| fixtures().account(k).is_some_and(|a| a.data.is_none()))
    .unwrap();
    assert_eq!(crate::AccountView::get(fixtures(), &unkept), Known::Unknown);
}
