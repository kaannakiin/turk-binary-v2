use domain::Pubkey;

/// Pubkeys are uniformly random, so their leading bytes spread accounts
/// evenly, and the mapping stays stable across restarts and reconnects.
pub(crate) fn shard_of_pubkey(pubkey: &Pubkey, shards: usize) -> usize {
    let bytes = pubkey.to_bytes();
    let prefix = u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    modulo(prefix, shards)
}

/// FNV-1a: `std`'s hasher is randomly seeded per process, which would move
/// filter subscriptions between shards on every restart.
pub(crate) fn shard_of_key(key: &str, shards: usize) -> usize {
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    modulo(hash, shards)
}

pub(crate) fn partition(pubkeys: &[Pubkey], shards: usize) -> Vec<Vec<Pubkey>> {
    let mut out = vec![Vec::new(); shards.max(1)];
    for pubkey in pubkeys {
        out[shard_of_pubkey(pubkey, shards)].push(*pubkey);
    }
    out
}

fn modulo(value: u64, shards: usize) -> usize {
    let shards = u64::try_from(shards.max(1)).unwrap_or(u64::MAX);
    usize::try_from(value % shards).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(n: usize) -> Vec<Pubkey> {
        (0..n).map(|_| Pubkey::new_unique()).collect()
    }

    #[test]
    fn partition_keeps_every_pubkey_exactly_once() {
        let input = keys(1_000);
        let mut flat: Vec<_> = partition(&input, 12).into_iter().flatten().collect();
        let mut expected = input.clone();
        flat.sort();
        expected.sort();
        assert_eq!(flat, expected);
    }

    #[test]
    fn partition_is_deterministic() {
        let input = keys(100);
        assert_eq!(partition(&input, 7), partition(&input, 7));
    }

    #[test]
    fn single_shard_takes_everything() {
        let input = keys(10);
        assert_eq!(partition(&input, 1), vec![input]);
    }

    #[test]
    fn zero_shards_is_treated_as_one() {
        assert_eq!(partition(&keys(3), 0).len(), 1);
    }

    #[test]
    fn key_shard_is_stable_and_in_range() {
        let shard = shard_of_key("pools", 12);
        assert!(shard < 12);
        assert_eq!(shard, shard_of_key("pools", 12));
    }

    #[test]
    fn random_pubkeys_spread_over_all_shards() {
        let sizes: Vec<_> = partition(&random_keys(12_000), 12)
            .iter()
            .map(Vec::len)
            .collect();
        assert!(sizes.iter().all(|&n| n > 800), "{sizes:?}");
    }

    fn random_keys(n: usize) -> Vec<Pubkey> {
        (0..n)
            .map(|_| Pubkey::new_from_array(rand::random()))
            .collect()
    }
}
