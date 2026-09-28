use domain::Pubkey;
use solana_hash::Hash;
use solana_message::VersionedMessage;
use solana_message::v1::{self, TransactionConfig};
use solana_signature::Signature;
use solana_transaction::versioned::VersionedTransaction;

use crate::{SwapInstructions, TxError};

// src: SIMD-0296 (a v1 transaction is at most 4096 bytes); AGENTS.md → Transaction format
pub const MAX_TRANSACTION_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fees {
    pub compute_unit_limit: u32,
    pub priority_fee_lamports: u64,
}

pub fn unsigned_v1(
    instructions: &SwapInstructions,
    fee_payer: &Pubkey,
    recent_blockhash: [u8; 32],
    fees: Fees,
) -> Result<Vec<u8>, TxError> {
    let all: Vec<_> = instructions
        .setup
        .iter()
        .chain(std::iter::once(&instructions.swap))
        .chain(&instructions.cleanup)
        .cloned()
        .collect();
    let config = TransactionConfig::empty()
        .with_compute_unit_limit(fees.compute_unit_limit)
        .with_priority_fee(fees.priority_fee_lamports);
    let message = v1::Message::try_compile_with_config(
        fee_payer,
        &all,
        Hash::new_from_array(recent_blockhash),
        config,
    )
    .map_err(|error| TxError::Compile(error.to_string()))?;
    let signatures =
        vec![Signature::default(); usize::from(message.header.num_required_signatures)];
    let transaction = VersionedTransaction {
        signatures,
        message: VersionedMessage::V1(message),
    };
    let bytes =
        wincode::serialize(&transaction).map_err(|error| TxError::Compile(error.to_string()))?;
    if bytes.len() > MAX_TRANSACTION_BYTES {
        return Err(TxError::TooLarge {
            bytes: bytes.len(),
            max: MAX_TRANSACTION_BYTES,
        });
    }
    Ok(bytes)
}
