use domain::chain::CLOCK_SYSVAR;
use domain::{ChainClock, DexKind, Pubkey, Slot};

use super::accounts::fixtures;
use super::fixture;
use crate::{
    Closure, Dependency, Need, NoAccounts, PoolAccount, Presence, Role, Side, closure,
    structural_ranges,
};

const POOL: Pubkey = Pubkey::from_str_const("11111111111111111111111111111112");
const PUMP_MINT: Pubkey = Pubkey::from_str_const("EyRriTY1S3vEcyyh79jhFUCqe4ecK81PKrhuSbfru6X7");
const WSOL: Pubkey = Pubkey::from_str_const("So11111111111111111111111111111111111111112");

const SIMPLE: [DexKind; 4] = [
    DexKind::RaydiumAmmV4,
    DexKind::RaydiumCpmm,
    DexKind::MeteoraDammV2,
    DexKind::PumpAmm,
];

fn derive(kind: DexKind, data: &[u8]) -> Closure {
    let pool = PoolAccount {
        address: POOL,
        data,
        mints: None,
    };
    closure(kind, &pool, &NoAccounts).unwrap()
}

fn bonding_curve(mints: Option<(Pubkey, Pubkey)>) -> Closure {
    let pool = PoolAccount {
        address: POOL,
        data: fixture(DexKind::PumpBondingCurve),
        mints,
    };
    closure(DexKind::PumpBondingCurve, &pool, &NoAccounts).unwrap()
}

fn role_key(closure: &Closure, role: Role) -> Option<Pubkey> {
    closure
        .deps
        .iter()
        .find(|d| d.role == role)
        .map(|d| d.pubkey)
}

fn non_pool_deps(closure: &Closure) -> impl Iterator<Item = &Dependency> {
    closure.deps.iter().filter(|d| d.role != Role::Pool)
}

#[test]
fn every_dependency_exists_on_chain_with_an_accepted_owner() {
    let mut closures: Vec<(DexKind, Closure)> = SIMPLE
        .map(|kind| (kind, derive(kind, fixture(kind))))
        .into();
    closures.push((
        DexKind::PumpBondingCurve,
        bonding_curve(Some((PUMP_MINT, WSOL))),
    ));
    for (kind, closure) in &closures {
        for dep in non_pool_deps(closure) {
            let account = fixtures()
                .account(&dep.pubkey)
                .unwrap_or_else(|| panic!("{kind} {:?} {} not on chain", dep.role, dep.pubkey));
            assert!(
                dep.owner.accepts(&account.owner),
                "{kind} {:?} owned by {}",
                dep.role,
                account.owner
            );
        }
    }
}

#[test]
fn every_vault_holds_the_pool_mint_on_its_side() {
    for kind in SIMPLE {
        let closure = derive(kind, fixture(kind));
        for side in [Side::A, Side::B] {
            let vault = role_key(&closure, Role::Vault(side)).unwrap();
            let mint = role_key(&closure, Role::Mint(side)).unwrap();
            let token_account = &fixtures().account(&vault).unwrap().bytes();
            assert_eq!(&token_account[..32], mint.as_ref(), "{kind} {side:?}");
        }
    }
}

#[test]
fn pump_config_pdas_match_live_addresses() {
    let curve = bonding_curve(Some((PUMP_MINT, WSOL)));
    let amm = derive(DexKind::PumpAmm, fixture(DexKind::PumpAmm));
    let found = [
        role_key(&curve, Role::PumpGlobal),
        role_key(&curve, Role::PumpFeeConfig),
        role_key(&amm, Role::PumpAmmGlobalConfig),
        role_key(&amm, Role::PumpFeeConfig),
    ];
    let live = [
        "4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf",
        "8Wf5TiAheLUqBrKXeYg2JtAFFMWtKdG2BSFgqUcPVwTt",
        "ADyA8hdefvWN2dbGGWFotbzWxrAvLW83WG6QCVXvJKqw",
        "5PHirr8joyTMp9JMm6nW7hNDVyEYdkzDqazxPD7RaTjx",
    ]
    .map(|key| Some(Pubkey::from_str_const(key)));
    assert_eq!(found, live);
}

#[test]
fn bonding_curve_without_known_mints_is_unverified() {
    assert!(!bonding_curve(None).verified);
}

#[test]
fn damm_v2_vaults_are_swap_only() {
    let closure = derive(DexKind::MeteoraDammV2, fixture(DexKind::MeteoraDammV2));
    let needs: Vec<Need> = closure
        .deps
        .iter()
        .filter(|d| matches!(d.role, Role::Vault(_)))
        .map(|d| d.need)
        .collect();
    assert_eq!(needs, [Need::Swap, Need::Swap]);
}

#[test]
fn only_amm_v4_quotes_without_the_clock() {
    for kind in SIMPLE {
        let has_clock = role_key(&derive(kind, fixture(kind)), Role::Clock) == Some(CLOCK_SYSVAR);
        assert_eq!(has_clock, kind != DexKind::RaydiumAmmV4, "{kind}");
    }
}

#[test]
fn simple_closures_are_complete_required_and_verified() {
    for kind in SIMPLE {
        let closure = derive(kind, fixture(kind));
        let all_required = closure
            .deps
            .iter()
            .all(|d| d.presence == Presence::Required);
        assert!(
            closure.verified && closure.is_complete() && all_required,
            "{kind}"
        );
    }
}

#[test]
fn bytes_outside_structural_ranges_do_not_change_the_closure() {
    for kind in SIMPLE {
        let original = fixture(kind);
        let ranges = structural_ranges(kind, &Role::Pool);
        let mut mutated = original.to_vec();
        for (i, byte) in mutated.iter_mut().enumerate().skip(8) {
            if !ranges.iter().any(|r| r.contains(&i)) {
                *byte ^= 0xff;
            }
        }
        assert_eq!(derive(kind, &mutated), derive(kind, original), "{kind}");
    }
}

#[test]
fn every_dependency_address_lies_in_a_structural_range() {
    for kind in SIMPLE {
        let original = fixture(kind);
        let ranges = structural_ranges(kind, &Role::Pool);
        let mut mutated = original.to_vec();
        for range in ranges {
            for byte in &mut mutated[range.clone()] {
                *byte ^= 0xff;
            }
        }
        let before = derive(kind, original);
        let after = derive(kind, &mutated);
        let pool_field_deps = |c: &Closure| -> Vec<Pubkey> {
            c.deps
                .iter()
                .filter(|d| matches!(d.role, Role::Vault(_) | Role::Mint(_) | Role::AmmConfig))
                .map(|d| d.pubkey)
                .collect()
        };
        let unchanged = pool_field_deps(&before)
            .iter()
            .zip(pool_field_deps(&after))
            .filter(|(a, b)| *a == b)
            .count();
        assert_eq!(unchanged, 0, "{kind}");
    }
}

#[test]
fn clock_fixture_decodes_at_its_capture_slot() {
    let clock = ChainClock::decode(fixtures().account(&CLOCK_SYSVAR).unwrap().bytes()).unwrap();
    assert_eq!(clock.slot, Slot(fixtures().slot(&CLOCK_SYSVAR).unwrap()));
}
