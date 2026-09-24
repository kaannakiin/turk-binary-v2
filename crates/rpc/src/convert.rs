use bytes::Bytes;
use domain::{AccountFilter, AccountUpdate, Commitment, Pubkey, Slot, WriteVersion};
use solana_account_decoder_client_types::UiAccount;
use solana_commitment_config::CommitmentConfig;
use solana_rpc_client_api::filter::{Memcmp, RpcFilterType};

use crate::RpcError;

pub(crate) const fn commitment_config(commitment: Commitment) -> CommitmentConfig {
    match commitment {
        Commitment::Processed => CommitmentConfig::processed(),
        Commitment::Confirmed => CommitmentConfig::confirmed(),
        Commitment::Finalized => CommitmentConfig::finalized(),
    }
}

pub(crate) fn rpc_filters(filter: &AccountFilter) -> Vec<RpcFilterType> {
    filter
        .data_size
        .map(RpcFilterType::DataSize)
        .into_iter()
        .chain(
            filter
                .memcmp
                .iter()
                .map(|m| RpcFilterType::Memcmp(Memcmp::new_raw_bytes(m.offset, m.bytes.clone()))),
        )
        .collect()
}

pub(crate) fn account_update(
    method: &'static str,
    pubkey: Pubkey,
    account: &UiAccount,
    slot: Slot,
) -> Result<AccountUpdate, RpcError> {
    let decode_err = |reason| RpcError::Decode {
        method,
        pubkey: pubkey.to_string(),
        reason,
    };
    let owner = account
        .owner
        .parse()
        .map_err(|_| decode_err("owner is not a valid pubkey"))?;
    let data = account
        .data
        .decode()
        .ok_or_else(|| decode_err("data is not in a binary encoding"))?;
    Ok(AccountUpdate {
        pubkey,
        owner,
        lamports: account.lamports,
        data: Bytes::from(data),
        slot,
        write_version: WriteVersion::SNAPSHOT,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_size_comes_first_then_memcmp_in_order() {
        let filter = AccountFilter::owned_by(Pubkey::new_from_array([1; 32]))
            .with_data_size(752)
            .with_memcmp(0, [1, 2])
            .with_memcmp(8, [3]);
        let expected = vec![
            RpcFilterType::DataSize(752),
            RpcFilterType::Memcmp(Memcmp::new_raw_bytes(0, vec![1, 2])),
            RpcFilterType::Memcmp(Memcmp::new_raw_bytes(8, vec![3])),
        ];
        assert_eq!(rpc_filters(&filter), expected);
    }

    #[test]
    fn owner_only_filter_produces_no_rpc_filters() {
        let filter = AccountFilter::owned_by(Pubkey::new_from_array([1; 32]));
        assert!(rpc_filters(&filter).is_empty());
    }
}
