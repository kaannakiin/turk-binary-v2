use std::time::{Duration, SystemTime, UNIX_EPOCH};

use domain::{AccountUpdate, Pubkey, Slot, TxnSignature, WriteVersion};
use yellowstone_grpc_proto::prelude::SubscribeUpdateAccount;
use yellowstone_grpc_proto::prost_types::Timestamp;

/// `None` when the server stamp is in the future (clock skew) or invalid.
pub(crate) fn message_lag(created_at: &Timestamp, now: SystemTime) -> Option<Duration> {
    let created = UNIX_EPOCH
        + Duration::from_secs(u64::try_from(created_at.seconds).ok()?)
        + Duration::from_nanos(u64::try_from(created_at.nanos).ok()?);
    now.duration_since(created).ok()
}

pub(crate) fn account_update(update: SubscribeUpdateAccount) -> Option<AccountUpdate> {
    let info = update.account?;
    Some(AccountUpdate {
        pubkey: Pubkey::try_from(info.pubkey.as_slice()).ok()?,
        owner: Pubkey::try_from(info.owner.as_slice()).ok()?,
        lamports: info.lamports,
        data: info.data,
        slot: Slot(update.slot),
        write_version: WriteVersion(info.write_version),
        txn: info
            .txn_signature
            .and_then(|sig| <[u8; 64]>::try_from(sig.as_slice()).ok())
            .map(TxnSignature),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use yellowstone_grpc_proto::prelude::SubscribeUpdateAccountInfo;

    fn update(pubkey: Vec<u8>) -> SubscribeUpdateAccount {
        SubscribeUpdateAccount {
            account: Some(SubscribeUpdateAccountInfo {
                pubkey,
                owner: vec![2; 32],
                lamports: 5,
                data: vec![1, 2, 3].into(),
                write_version: 9,
                ..SubscribeUpdateAccountInfo::default()
            }),
            slot: 42,
            is_startup: false,
        }
    }

    #[test]
    fn converts_all_fields() {
        let got = account_update(update(vec![1; 32])).unwrap();
        assert_eq!(got.pubkey, Pubkey::new_from_array([1; 32]));
        assert_eq!(got.owner, Pubkey::new_from_array([2; 32]));
        assert_eq!(got.slot, Slot(42));
        assert_eq!(got.write_version, WriteVersion(9));
        assert_eq!(&got.data[..], &[1, 2, 3]);
    }

    fn stamp(seconds: i64) -> Timestamp {
        Timestamp { seconds, nanos: 0 }
    }

    #[test]
    fn lag_is_time_since_creation() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        assert_eq!(message_lag(&stamp(990), now), Some(Duration::from_secs(10)));
    }

    #[test]
    fn future_stamp_has_no_lag() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        assert_eq!(message_lag(&stamp(1_005), now), None);
    }

    #[test]
    fn negative_stamp_is_ignored() {
        assert_eq!(message_lag(&stamp(-1), SystemTime::now()), None);
    }

    #[test]
    fn rejects_malformed_pubkey() {
        assert!(account_update(update(vec![1; 31])).is_none());
    }
}
