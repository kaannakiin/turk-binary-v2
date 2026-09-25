use domain::chain::TOKEN_2022_PROGRAM;
use spl_token_2022_interface::extension::transfer_fee::{self, TransferFeeConfig};
use spl_token_2022_interface::extension::transfer_hook::TransferHook;
use spl_token_2022_interface::extension::{
    AccountType, BaseStateWithExtensions, ExtensionType, StateWithExtensions,
};
use spl_token_2022_interface::state::Mint;

use super::captured;
use crate::error::MintDecodeError;
use crate::token22::{TransferFee, decode_mint};

const MINT_TLV_START: usize = 166;
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
        .filter(|(_, owner, _)| *owner == TOKEN_2022_PROGRAM)
        .collect();
    assert!(mints.len() >= 10, "Token-2022 mint fixtures");
    let mut fee_mints = 0;
    let mut hook_mints = 0;
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
    }
    assert!(
        fee_mints >= 5 && hook_mints >= 1,
        "fixtures cover fee and hook mints"
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
