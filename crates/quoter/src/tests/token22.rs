use domain::chain::TOKEN_2022_PROGRAM;
use spl_token_2022_interface::extension::default_account_state::DefaultAccountState;
use spl_token_2022_interface::extension::non_transferable::NonTransferable;
use spl_token_2022_interface::extension::pausable::PausableConfig;
use spl_token_2022_interface::extension::transfer_fee::{self, TransferFeeConfig};
use spl_token_2022_interface::extension::transfer_hook::TransferHook;
use spl_token_2022_interface::extension::{
    AccountType, BaseStateWithExtensions, BaseStateWithExtensionsMut, ExtensionType,
    StateWithExtensions, StateWithExtensionsMut,
};
use spl_token_2022_interface::state::{AccountState, Mint};

use super::captured;
use crate::error::{MintDecodeError, QuoteError};
use crate::token22::{Restrictions, TransferFee, check_transfer, decode_mint};

const MINT_TLV_START: usize = 166;

fn is_mint(data: &[u8]) -> bool {
    let base = ExtensionType::try_calculate_account_len::<Mint>(&[]).expect("base length");
    data.len() == base || data.get(MINT_TLV_START - 1) == Some(&(AccountType::Mint as u8))
}
const TLV_HEADER: usize = 4;

/// Every extension type the interface knows, checked against its own
/// length and account type rather than a table restated here.
#[test]
fn every_mint_extension_length_matches_the_interface() {
    for kind in 1..=u16::MAX {
        let Ok(extension) = ExtensionType::try_from(kind) else {
            continue;
        };
        let mut data = vec![0u8; MINT_TLV_START];
        data[45] = 1;
        data[165] = 1;
        let local = match extension.get_account_type() {
            AccountType::Mint if extension != ExtensionType::TokenMetadata => {
                let len = ExtensionType::try_calculate_account_len::<Mint>(&[extension])
                    .expect("sized mint extension")
                    - MINT_TLV_START
                    - TLV_HEADER;
                data.extend_from_slice(&kind.to_le_bytes());
                data.extend_from_slice(&u16::try_from(len).expect("short").to_le_bytes());
                data.resize(data.len() + len, 0);
                decode_mint(&TOKEN_2022_PROGRAM, &data).map(|m| m.extensions)
            }
            AccountType::Account => {
                data.extend_from_slice(&kind.to_le_bytes());
                data.extend_from_slice(&0u16.to_le_bytes());
                assert_eq!(
                    decode_mint(&TOKEN_2022_PROGRAM, &data).map(|m| m.extensions),
                    Err(MintDecodeError::Extension),
                    "account-only extension {kind} on a mint"
                );
                continue;
            }
            _ => continue,
        };
        assert_eq!(local, Ok(vec![kind]), "extension {kind}");
    }
}

#[test]
fn captured_mints_decode_as_the_interface_decodes_them() {
    let mints: Vec<_> = captured()
        .into_iter()
        .filter(|(_, owner, data)| *owner == TOKEN_2022_PROGRAM && is_mint(data))
        .collect();
    assert!(mints.len() >= 10, "Token-2022 mint fixtures");
    let mut fee_mints = 0;
    let mut hook_mints = 0;
    let mut restricted = Restrictions::default();
    let mut running_pausable = 0;
    for (key, owner, data) in mints {
        let local = decode_mint(&owner, &data).unwrap_or_else(|e| panic!("{key}: {e}"));
        let canonical = StateWithExtensions::<Mint>::unpack(&data).expect("interface decodes");
        let extensions: Vec<u16> = canonical
            .get_extension_types()
            .expect("extension types")
            .into_iter()
            .map(u16::from)
            .collect();
        let freeze: Option<[u8; 32]> = canonical.base.freeze_authority.map(|a| a.to_bytes()).into();
        assert_eq!(
            (
                local.decimals,
                local.supply,
                local.freeze_authority.map(|a| a.to_bytes()),
                local.extensions.clone()
            ),
            (
                canonical.base.decimals,
                canonical.base.supply,
                freeze,
                extensions
            ),
            "{key}"
        );
        if let Ok(config) = canonical.get_extension::<TransferFeeConfig>() {
            fee_mints += 1;
            let schedule = local.transfer_fee.expect("fee schedule");
            for fee in [schedule.older, schedule.newer] {
                let epoch = fee.epoch;
                let expected = config.get_epoch_fee(epoch);
                assert_eq!(
                    (fee.epoch, fee.maximum_fee, fee.basis_points),
                    (
                        u64::from(expected.epoch),
                        u64::from(expected.maximum_fee),
                        u16::from(expected.transfer_fee_basis_points)
                    ),
                    "{key} epoch {epoch}"
                );
            }
        } else {
            assert_eq!(local.transfer_fee, None, "{key}");
        }
        if let Ok(hook) = canonical.get_extension::<TransferHook>() {
            hook_mints += 1;
            let program = hook.program_id.copied().map(|p| p.to_bytes());
            assert_eq!(
                local
                    .transfer_hook
                    .and_then(|h| h.program)
                    .map(|p| p.to_bytes()),
                program,
                "{key}"
            );
        }
        let pausable = canonical.get_extension::<PausableConfig>().ok();
        let expected = Restrictions {
            paused: pausable.is_some_and(|config| bool::from(config.paused)),
            non_transferable: canonical.get_extension::<NonTransferable>().is_ok(),
            default_frozen: canonical
                .get_extension::<DefaultAccountState>()
                .is_ok_and(|default| default.state == AccountState::Frozen as u8),
        };
        assert_eq!(local.restrictions, expected, "{key}");
        restricted.paused |= expected.paused;
        restricted.default_frozen |= expected.default_frozen;
        running_pausable += usize::from(pausable.is_some() && !expected.paused);
    }
    assert!(
        fee_mints >= 5 && hook_mints >= 1,
        "fixtures cover fee and hook mints"
    );
    // src: mainnet getAccountInfo jsonParsed at slot 451609545: Pre2Y4ga… and Pre5X98d… paused,
    // EMFTTUnt… and FJiust6A… defaultAccountState frozen, XsoCS1Tf… (SPYx) pausable and running.
    assert!(
        restricted.paused && restricted.default_frozen && running_pausable >= 1,
        "fixtures cover a paused mint, a running pausable one and a frozen default state"
    );
}

/// No non-transferable mint was among the 3030 pool-listed mints scanned at
/// slot 451609545, so the interface builds one.
#[test]
fn a_non_transferable_mint_refuses_both_directions() {
    let len = ExtensionType::try_calculate_account_len::<Mint>(&[ExtensionType::NonTransferable])
        .expect("mint length");
    let mut data = vec![0u8; len];
    let mut state =
        StateWithExtensionsMut::<Mint>::unpack_uninitialized(&mut data).expect("uninitialized");
    state
        .init_extension::<NonTransferable>(true)
        .expect("extension");
    state.base.decimals = 6;
    state.base.is_initialized = true;
    state.pack_base();
    state.init_account_type().expect("account type");

    let fixed = decode_mint(&TOKEN_2022_PROGRAM, &data).expect("decodes");
    let other = captured()
        .into_iter()
        .find(|(_, owner, data)| *owner == TOKEN_2022_PROGRAM && is_mint(data))
        .and_then(|(_, owner, data)| decode_mint(&owner, &data).ok())
        .expect("a transferable mint");

    assert!(fixed.restrictions.non_transferable);
    assert_eq!(
        check_transfer(&fixed, &other),
        Err(QuoteError::NonTransferable)
    );
    assert_eq!(
        check_transfer(&other, &fixed),
        Err(QuoteError::NonTransferable)
    );
}

/// Rounding edges: one unit either side of where the ceiling steps, the
/// maximum-fee cap, and the top of `u64`.
#[test]
fn transfer_fees_match_the_interface_at_rounding_edges() {
    let schedules: Vec<_> = captured()
        .into_iter()
        .filter_map(|(key, owner, data)| {
            let local = decode_mint(&owner, &data).ok()?.transfer_fee?;
            let canonical = StateWithExtensions::<Mint>::unpack(&data)
                .ok()?
                .get_extension::<TransferFeeConfig>()
                .ok()
                .copied()?;
            Some((key, local, canonical))
        })
        .collect();
    assert!(!schedules.is_empty());
    for (key, local, canonical) in schedules {
        for fee in [local.older, local.newer] {
            for amount in edges(&fee) {
                let expected = canonical.calculate_epoch_fee(fee.epoch, amount);
                assert_eq!(fee.calculate_fee(amount), expected, "{key} amount {amount}");
            }
        }
    }
    let bps_edges = transfer_fee::TransferFee {
        epoch: 0.into(),
        maximum_fee: u64::MAX.into(),
        transfer_fee_basis_points: 1.into(),
    };
    let local = TransferFee {
        epoch: 0,
        maximum_fee: u64::MAX,
        basis_points: 1,
    };
    for amount in [
        0,
        1,
        9_999,
        10_000,
        10_001,
        19_999,
        20_000,
        20_001,
        u64::MAX,
    ] {
        assert_eq!(local.calculate_fee(amount), bps_edges.calculate_fee(amount));
    }
}

fn edges(fee: &TransferFee) -> Vec<u64> {
    let mut amounts = vec![0, 1, 2, u64::MAX - 1, u64::MAX];
    let bps = u64::from(fee.basis_points);
    if bps > 0 {
        let step = 10_000 / bps.max(1);
        amounts.extend([step.saturating_sub(1), step, step + 1]);
        let cap = fee.maximum_fee.saturating_mul(10_000) / bps;
        amounts.extend([cap.saturating_sub(1), cap, cap.saturating_add(1)]);
    }
    amounts
}

/// Quotes read the fee through `Mint::fee_at`; on mints whose older and
/// newer fees differ, it has to switch where the interface does.
#[test]
fn a_fee_change_takes_effect_at_the_epoch_the_interface_switches() {
    let mut changing = 0;
    for (key, owner, data) in captured() {
        let Ok(local) = decode_mint(&owner, &data) else {
            continue;
        };
        let Some(schedule) = local.transfer_fee else {
            continue;
        };
        let rate = |fee: TransferFee| (fee.maximum_fee, fee.basis_points);
        if rate(schedule.older) == rate(schedule.newer) {
            continue;
        }
        changing += 1;
        let canonical = *StateWithExtensions::<Mint>::unpack(&data)
            .expect("interface decodes")
            .get_extension::<TransferFeeConfig>()
            .expect("fee config");
        let switch = schedule.newer.epoch;
        for epoch in [switch.saturating_sub(1), switch, switch + 1] {
            let fee = local.fee_at(epoch).expect("a fee mint");
            for amount in [1, 999, 1_000_000_000, u64::MAX] {
                assert_eq!(
                    fee.calculate_fee(amount),
                    canonical.calculate_epoch_fee(epoch, amount),
                    "{key} epoch {epoch} amount {amount}"
                );
            }
        }
    }
    assert!(changing >= 2, "fixtures hold mints whose fee changes");
}
