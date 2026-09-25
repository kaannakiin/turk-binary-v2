use std::collections::{BTreeMap, BTreeSet, HashMap};

use domain::chain::CLOCK_SYSVAR;
use domain::{AccountFilter, Commitment, Pubkey};
use yellowstone_grpc_proto::prelude::{
    CommitmentLevel, SubscribeRequest, SubscribeRequestFilterAccounts,
    SubscribeRequestFilterAccountsFilter, SubscribeRequestFilterAccountsFilterMemcmp,
    SubscribeRequestFilterBlocksMeta, SubscribeRequestFilterSlots, SubscribeRequestPing,
    subscribe_request_filter_accounts_filter::Filter,
    subscribe_request_filter_accounts_filter_memcmp::Data,
};
use yellowstone_grpc_proto::prost::Message;

use crate::events::{Group, GroupKey, LimitViolation};

const SLOTS_FILTER_KEY: &str = "slots";
const ACCOUNTS_PREFIX: &str = "a";
const FILTERS_PREFIX: &str = "f";
const BLOCKS_META_FILTER_KEY: &str = "blocks_meta";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Heartbeat {
    Slots,
    Clock,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) pubkeys_per_filter: usize,
    pub(crate) account_filters: Option<usize>,
    pub(crate) request_bytes: usize,
}

#[derive(Debug)]
pub(crate) struct Built {
    pub(crate) request: SubscribeRequest,
    pub(crate) bytes: usize,
}

/// Every send replaces the server's whole filter set, so each request
/// carries every group. The Clock sysvar is always listed: it updates every
/// slot, so it doubles as the stream's heartbeat and slot evidence, and the
/// account list is never empty (an empty list would match every account).
///
/// Filter names carry `seq`. Updates are tagged with the names they matched,
/// so the first update tagged with a new `seq` proves the server switched to
/// that request; the plugin sends no other acknowledgement. The request must
/// not carry `ping`: the plugin answers a request with `ping` set with a pong
/// and drops its filters.
pub(crate) fn build_request(
    groups: &BTreeMap<GroupKey, Group>,
    heartbeat: Heartbeat,
    commitment: Commitment,
    limits: &Limits,
    seq: u64,
    from_slot: Option<u64>,
) -> Result<Built, LimitViolation> {
    let mut pubkeys: BTreeSet<Pubkey> = groups
        .values()
        .flat_map(|g| g.pubkeys.iter().copied())
        .collect();
    pubkeys.insert(CLOCK_SYSVAR);
    let per_filter = limits.pubkeys_per_filter.max(1);
    let mut accounts: HashMap<String, SubscribeRequestFilterAccounts> = pubkeys
        .iter()
        .copied()
        .collect::<Vec<_>>()
        .chunks(per_filter)
        .enumerate()
        .map(|(i, chunk)| (format!("{ACCOUNTS_PREFIX}{seq}.{i}"), pubkeys_filter(chunk)))
        .collect();
    let filters: BTreeSet<&AccountFilter> =
        groups.values().flat_map(|g| g.filters.iter()).collect();
    accounts.extend(
        filters
            .into_iter()
            .enumerate()
            .map(|(i, filter)| (format!("{FILTERS_PREFIX}{seq}.{i}"), owner_filter(filter))),
    );
    if let Some(limit) = limits.account_filters
        && accounts.len() > limit
    {
        return Err(LimitViolation::Filters { limit });
    }
    let slots = match heartbeat {
        Heartbeat::Slots => HashMap::from([(
            SLOTS_FILTER_KEY.to_owned(),
            SubscribeRequestFilterSlots {
                // Fork tracking needs Confirmed/Finalized/Dead even while
                // streaming at Processed; `filter_by_commitment` would drop
                // them and Dead is only sent with interslot updates.
                filter_by_commitment: Some(false),
                interslot_updates: Some(true),
            },
        )]),
        Heartbeat::Clock => HashMap::new(),
    };
    let request = SubscribeRequest {
        accounts,
        slots,
        commitment: Some(commitment_level(commitment) as i32),
        from_slot,
        ..SubscribeRequest::default()
    };
    let bytes = request.encoded_len();
    if bytes > limits.request_bytes {
        return Err(LimitViolation::RequestBytes {
            bytes,
            limit: limits.request_bytes,
        });
    }
    Ok(Built { request, bytes })
}

/// The slot feed only carries confirmed block metadata: parents and
/// confirmations for fork tracking when the provider refuses `slots`.
#[expect(
    clippy::zero_sized_map_values,
    reason = "the proto names filters with a map"
)]
pub(crate) fn slot_feed_request() -> SubscribeRequest {
    SubscribeRequest {
        blocks_meta: HashMap::from([(
            BLOCKS_META_FILTER_KEY.to_owned(),
            SubscribeRequestFilterBlocksMeta::default(),
        )]),
        commitment: Some(CommitmentLevel::Confirmed as i32),
        ..SubscribeRequest::default()
    }
}

/// The plugin treats a request with `ping` as a keepalive only and leaves
/// the filters alone.
pub(crate) fn ping_request(id: i32) -> SubscribeRequest {
    SubscribeRequest {
        ping: Some(SubscribeRequestPing { id }),
        ..SubscribeRequest::default()
    }
}

pub(crate) fn seq_of(name: &str) -> Option<u64> {
    let rest = name
        .strip_prefix(ACCOUNTS_PREFIX)
        .or_else(|| name.strip_prefix(FILTERS_PREFIX))?;
    rest.split_once('.')?.0.parse().ok()
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
    const LIMITS: Limits = Limits {
        pubkeys_per_filter: 100,
        account_filters: None,
        request_bytes: 4_000_000,
    };

    fn groups(entries: Vec<Group>) -> BTreeMap<GroupKey, Group> {
        entries
            .into_iter()
            .map(|g| (GroupKey(Pubkey::new_unique()), g))
            .collect()
    }

    fn keys(n: usize) -> Group {
        Group {
            pubkeys: (0..n).map(|_| Pubkey::new_unique()).collect(),
            filters: Vec::new(),
        }
    }

    fn build(groups: &BTreeMap<GroupKey, Group>, limits: &Limits) -> Result<Built, LimitViolation> {
        build_request(
            groups,
            Heartbeat::Slots,
            Commitment::Processed,
            limits,
            1,
            None,
        )
    }

    #[test]
    fn an_empty_request_still_lists_the_clock() {
        let built = build(&BTreeMap::new(), &LIMITS).unwrap();
        let listed: Vec<&String> = built
            .request
            .accounts
            .values()
            .flat_map(|f| &f.account)
            .collect();
        assert_eq!(listed, [&CLOCK_SYSVAR.to_string()]);
    }

    #[test]
    fn keys_shared_by_groups_are_listed_once() {
        let shared = Pubkey::new_unique();
        let group = || Group {
            pubkeys: BTreeSet::from([shared]),
            filters: Vec::new(),
        };
        let built = build(&groups(vec![group(), group()]), &LIMITS).unwrap();
        let listed = built
            .request
            .accounts
            .values()
            .map(|f| f.account.len())
            .sum::<usize>();
        assert_eq!(listed, 2);
    }

    #[test]
    fn keys_are_split_into_filters_of_at_most_the_limit() {
        let built = build(&groups(vec![keys(150), keys(99)]), &LIMITS).unwrap();
        let mut sizes: Vec<usize> = built
            .request
            .accounts
            .values()
            .map(|f| f.account.len())
            .collect();
        sizes.sort_unstable();
        assert_eq!(sizes, [50, 100, 100]);
    }

    #[test]
    fn a_filter_maps_owner_size_and_every_memcmp() {
        let filter = AccountFilter::owned_by(OWNER)
            .with_data_size(752)
            .with_memcmp(0, [9, 9]);
        let group = Group {
            pubkeys: BTreeSet::new(),
            filters: vec![filter],
        };
        let built = build(&groups(vec![group]), &LIMITS).unwrap();
        let f = &built.request.accounts["f1.0"];
        assert_eq!((f.owner.len(), f.filters.len(), f.account.len()), (1, 2, 0));
    }

    #[test]
    fn too_many_filters_is_a_limit_violation() {
        let limits = Limits {
            account_filters: Some(2),
            ..LIMITS
        };
        let err = build(&groups(vec![keys(250)]), &limits).unwrap_err();
        assert_eq!(err, LimitViolation::Filters { limit: 2 });
    }

    #[test]
    fn an_oversized_request_is_a_limit_violation() {
        let limits = Limits {
            request_bytes: 1_000,
            ..LIMITS
        };
        assert!(matches!(
            build(&groups(vec![keys(100)]), &limits),
            Err(LimitViolation::RequestBytes { .. })
        ));
    }

    #[test]
    fn clock_heartbeat_mode_sends_no_slots_filter() {
        let built = build_request(
            &BTreeMap::new(),
            Heartbeat::Clock,
            Commitment::Processed,
            &LIMITS,
            1,
            None,
        )
        .unwrap();
        assert!(built.request.slots.is_empty());
    }

    #[test]
    fn slots_mode_asks_for_every_status() {
        let built = build(&BTreeMap::new(), &LIMITS).unwrap();
        let slots = &built.request.slots[SLOTS_FILTER_KEY];
        assert_eq!(
            (slots.filter_by_commitment, slots.interslot_updates),
            (Some(false), Some(true))
        );
    }

    #[test]
    fn filter_requests_never_carry_a_ping() {
        let built = build_request(
            &BTreeMap::new(),
            Heartbeat::Slots,
            Commitment::Processed,
            &LIMITS,
            7,
            Some(99),
        )
        .unwrap();
        assert_eq!(
            (built.request.ping, built.request.from_slot),
            (None, Some(99))
        );
    }

    #[test]
    fn every_filter_name_carries_the_request_sequence() {
        let group = Group {
            pubkeys: BTreeSet::new(),
            filters: vec![AccountFilter::owned_by(OWNER)],
        };
        let built = build_request(
            &groups(vec![group, keys(5)]),
            Heartbeat::Slots,
            Commitment::Processed,
            &LIMITS,
            42,
            None,
        )
        .unwrap();
        assert!(
            built
                .request
                .accounts
                .keys()
                .all(|name| seq_of(name) == Some(42))
        );
    }

    #[test]
    fn names_from_other_filters_have_no_sequence() {
        assert_eq!(
            (seq_of("slots"), seq_of("a7"), seq_of("ax.1")),
            (None, None, None)
        );
    }

    #[test]
    fn a_ping_request_carries_nothing_but_the_ping() {
        let request = ping_request(3);
        assert!(request.accounts.is_empty() && request.slots.is_empty() && request.ping.is_some());
    }

    #[test]
    fn slot_feed_asks_for_confirmed_block_meta_only() {
        let request = slot_feed_request();
        assert_eq!(
            (
                request.blocks_meta.len(),
                request.accounts.len(),
                request.commitment
            ),
            (1, 0, Some(CommitmentLevel::Confirmed as i32))
        );
    }
}
