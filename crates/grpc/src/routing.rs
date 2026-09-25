use domain::Pubkey;

use crate::events::{GroupKey, Placement, StreamId};

/// Rendezvous hashing: adding a shard moves only the groups that land on it,
/// and the mapping is stable across restarts.
pub(crate) fn stream_for(key: &GroupKey, placement: Placement, shards: u16) -> StreamId {
    match placement {
        Placement::Shared => StreamId::Shared,
        Placement::Pool => StreamId::Shard(
            (0..shards.max(1))
                .max_by_key(|shard| score(&key.0, *shard))
                .unwrap_or(0),
        ),
    }
}

fn score(key: &Pubkey, shard: u16) -> u64 {
    let bytes = key.to_bytes();
    let word = |i: usize| u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap_or([0; 8]));
    splitmix64(word(0) ^ word(8).rotate_left(17) ^ splitmix64(u64::from(shard)))
}

const fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(i: u8) -> GroupKey {
        let mut bytes = [0u8; 32];
        bytes[0] = i;
        bytes[9] = i.wrapping_mul(31);
        GroupKey(Pubkey::new_from_array(bytes))
    }

    #[test]
    fn a_pool_always_lands_on_the_same_shard() {
        let key = GroupKey(Pubkey::new_unique());
        assert_eq!(
            stream_for(&key, Placement::Pool, 12),
            stream_for(&key, Placement::Pool, 12)
        );
    }

    #[test]
    fn shared_groups_go_to_the_shared_stream() {
        assert_eq!(
            stream_for(&pool(1), Placement::Shared, 12),
            StreamId::Shared
        );
    }

    #[test]
    fn pools_spread_over_every_shard() {
        let used: std::collections::BTreeSet<StreamId> = (0..=255)
            .map(|i| stream_for(&pool(i), Placement::Pool, 4))
            .collect();
        assert_eq!(used.len(), 4);
    }

    #[test]
    fn adding_a_shard_moves_pools_only_onto_the_new_shard() {
        for i in 0..=255 {
            let before = stream_for(&pool(i), Placement::Pool, 4);
            let after = stream_for(&pool(i), Placement::Pool, 5);
            assert!(after == before || after == StreamId::Shard(4));
        }
    }
}
