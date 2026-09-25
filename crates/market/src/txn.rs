use std::collections::HashMap;
use std::time::{Duration, Instant};

use domain::{AccountUpdate, Slot, TxnSignature};
use grpc::StreamId;

use crate::store::Source;

type GroupId = (Source, Slot, TxnSignature);

struct Held {
    opened: Instant,
    updates: Vec<AccountUpdate>,
}

/// Holds a transaction's account writes until its status closes the group,
/// so a pool never shows one vault after the swap and the other before it.
#[derive(Default)]
pub(crate) struct TxnBuffer {
    groups: HashMap<GroupId, Held>,
    held: usize,
}

pub(crate) type Released = Vec<(Source, Vec<AccountUpdate>)>;

impl TxnBuffer {
    /// Hands `update` back when no transaction status will close it.
    pub(crate) fn hold(
        &mut self,
        source: Source,
        update: AccountUpdate,
        now: Instant,
    ) -> Option<AccountUpdate> {
        let Some(signature) = update.txn else {
            return Some(update);
        };
        self.groups
            .entry((source, update.slot, signature))
            .or_insert_with(|| Held {
                opened: now,
                updates: Vec::new(),
            })
            .updates
            .push(update);
        self.held += 1;
        None
    }

    pub(crate) fn commit(
        &mut self,
        source: Source,
        slot: Slot,
        signature: TxnSignature,
    ) -> Vec<AccountUpdate> {
        self.groups
            .remove(&(source, slot, signature))
            .map(|held| self.release(held))
            .unwrap_or_default()
    }

    pub(crate) fn expired(&mut self, now: Instant, wait: Duration) -> Released {
        self.take(|_, held| now.duration_since(held.opened) >= wait)
    }

    /// A group still open when its slot is confirmed would land below the
    /// confirmation; it goes out first.
    pub(crate) fn through(&mut self, slot: Slot) -> Released {
        self.take(|id, _| id.1 <= slot)
    }

    pub(crate) fn all(&mut self) -> Released {
        self.take(|_, _| true)
    }

    pub(crate) fn discard(&mut self, stream: StreamId) {
        let before = self.groups.len();
        self.groups.retain(|id, _| id.0.stream != stream);
        if self.groups.len() != before {
            self.held = self.groups.values().map(|h| h.updates.len()).sum();
        }
    }

    pub(crate) const fn held(&self) -> usize {
        self.held
    }

    fn take(&mut self, due: impl Fn(&GroupId, &Held) -> bool) -> Released {
        let ids: Vec<GroupId> = self
            .groups
            .iter()
            .filter(|(id, held)| due(id, held))
            .map(|(id, _)| *id)
            .collect();
        let mut released: Vec<(Source, Slot, Vec<AccountUpdate>)> = ids
            .into_iter()
            .filter_map(|id| {
                let held = self.groups.remove(&id)?;
                Some((id.0, id.1, self.release(held)))
            })
            .collect();
        released
            .sort_by_key(|(_, slot, updates)| (*slot, updates.first().map(|u| u.write_version)));
        released
            .into_iter()
            .map(|(source, _, updates)| (source, updates))
            .collect()
    }

    fn release(&mut self, held: Held) -> Vec<AccountUpdate> {
        self.held -= held.updates.len();
        held.updates
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bytes::Bytes;
    use domain::{Pubkey, WriteVersion};

    use super::*;

    /// Recorded from mainnet with `turk-binary txn-probe --record`: one
    /// pool stream's account writes, transaction statuses and full
    /// transactions, in arrival order.
    const STREAM: &str = include_str!("tests/fixtures/streams/raydium_amm_v4.tsv");
    const TOKEN_ACCOUNT_LEN: usize = 165;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    fn signature(text: &str) -> Option<TxnSignature> {
        (text != "-").then(|| TxnSignature(hex(text).try_into().unwrap()))
    }

    fn token_amount(data: &[u8]) -> Option<u64> {
        (data.len() == TOKEN_ACCOUNT_LEN)
            .then(|| u64::from_le_bytes(data[64..72].try_into().unwrap()))
    }

    #[test]
    fn every_released_group_leaves_the_vaults_at_one_transactions_post_balances() {
        let mut post: HashMap<TxnSignature, HashMap<Pubkey, u64>> = HashMap::new();
        for line in STREAM.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            if f[0] == "txn" {
                let balances = f[3]
                    .split(',')
                    .filter(|b| !b.is_empty())
                    .map(|b| {
                        let (key, amount) = b.split_once('=').unwrap();
                        (Pubkey::from_str(key).unwrap(), amount.parse().unwrap())
                    })
                    .collect();
                post.insert(signature(f[2]).unwrap(), balances);
            }
        }
        let mut buffer = TxnBuffer::default();
        let mut vaults: HashMap<Pubkey, u64> = HashMap::new();
        let mut checked = 0;
        let now = Instant::now();
        for line in STREAM.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            let released = match f[0] {
                "acct" => {
                    let update = AccountUpdate {
                        slot: Slot(f[1].parse().unwrap()),
                        write_version: WriteVersion(f[2].parse().unwrap()),
                        txn: signature(f[3]),
                        pubkey: Pubkey::from_str(f[4]).unwrap(),
                        owner: Pubkey::from_str(f[5]).unwrap(),
                        lamports: f[6].parse().unwrap(),
                        data: Bytes::from(hex(f[7])),
                    };
                    buffer.hold(SHARD, update, now).into_iter().collect()
                }
                "status" => {
                    buffer.commit(SHARD, Slot(f[1].parse().unwrap()), signature(f[2]).unwrap())
                }
                _ => continue,
            };
            for update in &released {
                if let Some(amount) = token_amount(&update.data) {
                    vaults.insert(update.pubkey, amount);
                }
            }
            let Some(balances) = released
                .first()
                .and_then(|u| u.txn)
                .and_then(|sig| post.get(&sig))
            else {
                continue;
            };
            for (vault, amount) in &vaults {
                if let Some(expected) = balances.get(vault) {
                    assert_eq!(amount, expected, "vault {vault} after {}", f[2]);
                    checked += 1;
                }
            }
        }
        assert!(checked > 100, "only {checked} vault balances checked");
    }

    const SHARD: Source = Source {
        stream: StreamId::Shard(0),
        generation: 1,
    };

    fn write(slot: u64, txn: u8) -> AccountUpdate {
        AccountUpdate {
            pubkey: Pubkey::new_unique(),
            owner: Pubkey::new_unique(),
            lamports: 1,
            data: Bytes::new(),
            slot: Slot(slot),
            write_version: WriteVersion(1),
            txn: Some(TxnSignature([txn; 64])),
        }
    }

    fn held_slots(released: &Released) -> Vec<u64> {
        released
            .iter()
            .flat_map(|(_, updates)| updates.iter().map(|u| u.slot.0))
            .collect()
    }

    #[test]
    fn an_open_group_is_released_before_its_slot_is_confirmed() {
        let mut buffer = TxnBuffer::default();
        let now = Instant::now();
        buffer.hold(SHARD, write(10, 1), now);
        buffer.hold(SHARD, write(11, 2), now);
        assert_eq!(held_slots(&buffer.through(Slot(10))), [10]);
    }

    #[test]
    fn a_group_whose_status_never_comes_is_released_after_the_wait() {
        let mut buffer = TxnBuffer::default();
        let opened = Instant::now();
        buffer.hold(SHARD, write(10, 1), opened);
        let wait = Duration::from_millis(400);
        assert_eq!(held_slots(&buffer.expired(opened + wait, wait)), [10]);
    }

    #[test]
    fn a_dropped_stream_discards_only_its_own_groups() {
        let mut buffer = TxnBuffer::default();
        let other = Source {
            stream: StreamId::Shard(1),
            ..SHARD
        };
        let now = Instant::now();
        buffer.hold(SHARD, write(10, 1), now);
        buffer.hold(other, write(10, 2), now);
        buffer.discard(StreamId::Shard(0));
        assert_eq!(
            buffer.all().iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            [other]
        );
    }
}
