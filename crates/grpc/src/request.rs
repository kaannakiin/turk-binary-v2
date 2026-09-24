use std::collections::{BTreeMap, HashMap};

use domain::{AccountFilter, Commitment, Pubkey};
use yellowstone_grpc_proto::prelude::{
    CommitmentLevel, SubscribeRequest, SubscribeRequestFilterAccounts,
    SubscribeRequestFilterAccountsFilter, SubscribeRequestFilterAccountsFilterMemcmp,
    SubscribeRequestFilterSlots, subscribe_request_filter_accounts_filter::Filter,
    subscribe_request_filter_accounts_filter_memcmp::Data,
};

pub(crate) const SLOTS_FILTER_KEY: &str = "slots";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionTarget {
    Pubkeys(Vec<Pubkey>),
    Filter(AccountFilter),
}

/// Every send replaces the server-side filter set, so the request is always
/// rebuilt from the full set of live subscriptions. Pubkey lists longer than
/// `max_pubkeys_per_filter` become several filters (`key#0`, `key#1`, ...)
/// because providers cap the accounts allowed in one filter.
pub(crate) fn build_request(
    subscriptions: &BTreeMap<String, SubscriptionTarget>,
    commitment: Commitment,
    max_pubkeys_per_filter: usize,
) -> SubscribeRequest {
    let accounts: HashMap<_, _> = subscriptions
        .iter()
        .flat_map(|(key, target)| accounts_filters(key, target, max_pubkeys_per_filter.max(1)))
        .collect();
    let slots = HashMap::from([(
        SLOTS_FILTER_KEY.to_owned(),
        SubscribeRequestFilterSlots {
            // Fork tracking needs Confirmed/Finalized/Dead even while
            // streaming at Processed; `filter_by_commitment` would drop them
            // and Dead is only sent with interslot updates.
            filter_by_commitment: Some(false),
            interslot_updates: Some(true),
        },
    )]);
    SubscribeRequest {
        accounts,
        slots,
        commitment: Some(commitment_level(commitment) as i32),
        ..SubscribeRequest::default()
    }
}

fn accounts_filters(
    key: &str,
    target: &SubscriptionTarget,
    max_pubkeys: usize,
) -> Vec<(String, SubscribeRequestFilterAccounts)> {
    match target {
        SubscriptionTarget::Pubkeys(pubkeys) if pubkeys.len() <= max_pubkeys => {
            vec![(key.to_owned(), pubkeys_filter(pubkeys))]
        }
        SubscriptionTarget::Pubkeys(pubkeys) => pubkeys
            .chunks(max_pubkeys)
            .enumerate()
            .map(|(i, chunk)| (format!("{key}#{i}"), pubkeys_filter(chunk)))
            .collect(),
        SubscriptionTarget::Filter(filter) => vec![(key.to_owned(), owner_filter(filter))],
    }
}

fn pubkeys_filter(pubkeys: &[Pubkey]) -> SubscribeRequestFilterAccounts {
    SubscribeRequestFilterAccounts {
        account: pubkeys.iter().map(ToString::to_string).collect(),
        ..SubscribeRequestFilterAccounts::default()
    }
}

fn owner_filter(filter: &AccountFilter) -> SubscribeRequestFilterAccounts {
    SubscribeRequestFilterAccounts {
        owner: vec![filter.owner.to_string()],
        filters: filter
            .data_size
            .map(Filter::Datasize)
            .into_iter()
            .chain(filter.memcmp.iter().map(|m| {
                Filter::Memcmp(SubscribeRequestFilterAccountsFilterMemcmp {
                    offset: m.offset as u64,
                    data: Some(Data::Bytes(m.bytes.clone())),
                })
            }))
            .map(|f| SubscribeRequestFilterAccountsFilter { filter: Some(f) })
            .collect(),
        ..SubscribeRequestFilterAccounts::default()
    }
}

const fn commitment_level(commitment: Commitment) -> CommitmentLevel {
    match commitment {
        Commitment::Processed => CommitmentLevel::Processed,
        Commitment::Confirmed => CommitmentLevel::Confirmed,
        Commitment::Finalized => CommitmentLevel::Finalized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: Pubkey = Pubkey::new_from_array([3; 32]);
    const KEY: Pubkey = Pubkey::new_from_array([4; 32]);
    const LIMIT: usize = 100;

    fn subs(entries: &[(&str, SubscriptionTarget)]) -> BTreeMap<String, SubscriptionTarget> {
        entries
            .iter()
            .map(|(k, t)| ((*k).to_owned(), t.clone()))
            .collect()
    }

    #[test]
    fn pubkey_target_lists_accounts_only() {
        let req = build_request(
            &subs(&[("pools", SubscriptionTarget::Pubkeys(vec![KEY]))]),
            Commitment::Confirmed,
            LIMIT,
        );
        let f = &req.accounts["pools"];
        assert_eq!(f.account, vec![KEY.to_string()]);
        assert!(f.owner.is_empty() && f.filters.is_empty());
    }

    #[test]
    fn filter_target_maps_owner_size_and_memcmp() {
        let filter = AccountFilter::owned_by(OWNER)
            .with_data_size(752)
            .with_memcmp(0, [9, 9]);
        let req = build_request(
            &subs(&[("dex", SubscriptionTarget::Filter(filter))]),
            Commitment::Processed,
            LIMIT,
        );
        let f = &req.accounts["dex"];
        assert_eq!(f.owner, vec![OWNER.to_string()]);
        assert_eq!(
            f.filters,
            vec![
                SubscribeRequestFilterAccountsFilter {
                    filter: Some(Filter::Datasize(752))
                },
                SubscribeRequestFilterAccountsFilter {
                    filter: Some(Filter::Memcmp(SubscribeRequestFilterAccountsFilterMemcmp {
                        offset: 0,
                        data: Some(Data::Bytes(vec![9, 9])),
                    }))
                },
            ]
        );
    }

    #[test]
    fn request_always_carries_slot_filter_and_commitment() {
        let req = build_request(&BTreeMap::new(), Commitment::Finalized, LIMIT);
        let slots = &req.slots[SLOTS_FILTER_KEY];
        assert_eq!(
            (slots.filter_by_commitment, slots.interslot_updates),
            (Some(false), Some(true))
        );
        assert_eq!(req.commitment, Some(CommitmentLevel::Finalized as i32));
    }

    #[test]
    fn every_subscription_becomes_one_named_filter() {
        let req = build_request(
            &subs(&[
                ("a", SubscriptionTarget::Pubkeys(vec![KEY])),
                (
                    "b",
                    SubscriptionTarget::Filter(AccountFilter::owned_by(OWNER)),
                ),
            ]),
            Commitment::Confirmed,
            LIMIT,
        );
        assert_eq!(req.accounts.len(), 2);
    }

    #[test]
    fn long_pubkey_list_is_split_into_numbered_filters() {
        let pubkeys: Vec<_> = (0..250).map(|_| Pubkey::new_unique()).collect();
        let req = build_request(
            &subs(&[("pools", SubscriptionTarget::Pubkeys(pubkeys))]),
            Commitment::Processed,
            LIMIT,
        );
        let mut sizes: Vec<_> = req
            .accounts
            .iter()
            .map(|(k, f)| (k.clone(), f.account.len()))
            .collect();
        sizes.sort();
        assert_eq!(
            sizes,
            vec![
                ("pools#0".to_owned(), 100),
                ("pools#1".to_owned(), 100),
                ("pools#2".to_owned(), 50),
            ]
        );
    }

    #[test]
    fn list_at_limit_keeps_plain_key() {
        let pubkeys: Vec<_> = (0..LIMIT).map(|_| Pubkey::new_unique()).collect();
        let req = build_request(
            &subs(&[("pools", SubscriptionTarget::Pubkeys(pubkeys))]),
            Commitment::Processed,
            LIMIT,
        );
        assert!(req.accounts.contains_key("pools"));
    }
}
