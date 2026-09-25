use crate::kind::PoolKind;
use solana_pubkey::Pubkey;

pub const RAYDIUM_AMM_V4_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8");

pub const WHIRLPOOL_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc");

pub const RAYDIUM_CLMM_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK");

pub const RAYDIUM_CPMM_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C");

pub const METEORA_DLMM_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo");

pub const METEORA_DAMM_V2_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG");

pub const METEORA_DAMM_V1_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB");

pub const METEORA_DYNAMIC_VAULT_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("24Uqj9JCLxUeoC3hGfh5W3s9FM9uCHDS2SG3LYwBpyTi");

pub const PUMP_SWAP_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA");

pub const PUMP_SWAP_GLOBAL_CONFIG: Pubkey =
    Pubkey::from_str_const("ADyA8hdefvWN2dbGGWFotbzWxrAvLW83WG6QCVXvJKqw");

pub const PUMP_SWAP_FEE_PROGRAM: Pubkey =
    Pubkey::from_str_const("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ");

pub const PUMP_SWAP_PROTOCOL_FEE_RECIPIENT: Pubkey =
    Pubkey::from_str_const("62qc2CNXwrYqQScmEdiZFFAnJR262PxWEuNQtxfafNgV");

/// `GlobalConfig.reserved_fee_recipient` (offset 385), the set a mayhem pool's
/// swap must name instead of `protocol_fee_recipients`; the two sets are
/// disjoint and the program rejects the wrong one with `InvalidProtocolFeeRecipient`.
/// Measured 2026-08-13 over real mainnet swaps: 8 of 8 non-mayhem pools named a
/// protocol recipient, 8 of 8 mayhem pools named a reserved one.
pub const PUMP_SWAP_RESERVED_FEE_RECIPIENT: Pubkey =
    Pubkey::from_str_const("GesfTA3X2arioaHp8bbKdjG9vJtskViWACZoYvxp4twS");

pub const PUMP_SWAP_PFEE_FEE_RECIPIENT: Pubkey =
    Pubkey::from_str_const("A7hAgCzFw14fejgCp387JUJRMNyz4j89JKnhtKU8piqW");

pub const MEMO_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");

pub const ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

pub const TOKEN_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

pub const TOKEN_2022_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");

pub const BUILDABLE_KIND_COUNT: usize = 10;

/// One row per buildable WIRE kind. DLMM and PumpSwap each carry two kinds under one program, so
/// `program_id_to_kind` (detection: "which venue is this instruction") answers with the FIRST row
/// for a program — the canonical detect-time kind — while `pool_kind_to_program_id` is total over
/// every buildable kind.
pub const PROGRAM_ID_TO_KIND: [(Pubkey, PoolKind); BUILDABLE_KIND_COUNT] = [
    (RAYDIUM_AMM_V4_PROGRAM_ID, PoolKind::RaydiumAmmV4),
    (WHIRLPOOL_PROGRAM_ID, PoolKind::Whirlpool),
    (RAYDIUM_CLMM_PROGRAM_ID, PoolKind::RaydiumClmm),
    (RAYDIUM_CPMM_PROGRAM_ID, PoolKind::RaydiumCpmm),
    (METEORA_DLMM_PROGRAM_ID, PoolKind::MeteoraDlmmSwap),
    (METEORA_DLMM_PROGRAM_ID, PoolKind::MeteoraDlmmSwap2),
    (METEORA_DAMM_V2_PROGRAM_ID, PoolKind::MeteoraDammV2),
    (PUMP_SWAP_PROGRAM_ID, PoolKind::PumpSwapSell),
    (PUMP_SWAP_PROGRAM_ID, PoolKind::PumpSwapBuy),
    (METEORA_DAMM_V1_PROGRAM_ID, PoolKind::MeteoraDammV1),
];

pub fn program_id_to_kind(program_id: &Pubkey) -> Option<PoolKind> {
    PROGRAM_ID_TO_KIND
        .iter()
        .find(|(id, _)| id == program_id)
        .map(|(_, kind)| *kind)
}

pub fn pool_kind_to_program_id(kind: PoolKind) -> Option<Pubkey> {
    PROGRAM_ID_TO_KIND
        .iter()
        .find(|(_, k)| *k == kind)
        .map(|(id, _)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_map_covers_every_buildable_kind() {
        for (id, kind) in PROGRAM_ID_TO_KIND.iter() {
            assert_eq!(pool_kind_to_program_id(*kind), Some(*id));
        }
        for kind in PoolKind::BUILDABLE {
            assert!(pool_kind_to_program_id(kind).is_some(), "{kind:?} missing");
        }
    }

    #[test]
    fn detection_answers_the_canonical_kind_per_program() {
        assert_eq!(
            program_id_to_kind(&METEORA_DLMM_PROGRAM_ID),
            Some(PoolKind::MeteoraDlmmSwap)
        );
        assert_eq!(
            program_id_to_kind(&PUMP_SWAP_PROGRAM_ID),
            Some(PoolKind::PumpSwapSell)
        );
    }

    #[test]
    fn kinds_are_distinct_across_the_table() {
        for (i, (_, kind_a)) in PROGRAM_ID_TO_KIND.iter().enumerate() {
            for (j, (_, kind_b)) in PROGRAM_ID_TO_KIND.iter().enumerate().skip(i + 1) {
                assert_ne!(kind_a, kind_b, "duplicate kind at {i}/{j}");
            }
        }
    }

    #[test]
    fn out_of_scope_program_is_none() {
        assert_eq!(program_id_to_kind(&TOKEN_PROGRAM_ID), None);
        assert_eq!(pool_kind_to_program_id(PoolKind::Bisonfi), None);
    }
}
