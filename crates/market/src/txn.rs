use std::collections::HashMap;
use std::time::{Duration, Instant};

use domain::{AccountUpdate, Pubkey, Slot, TxnSignature};
use grpc::StreamId;

use crate::store::Source;

type GroupId = (Source, Slot, TxnSignature);

// Agave replays sibling forks in parallel, so the next slot on a stream does
// not yet prove an older slot finished.
const PROGRESS_SLOTS: u64 = 2;

struct Held {
    opened: Instant,
    updates: Vec<AccountUpdate>,
}

/// Holds a transaction's account writes until they are known to be complete,
/// so a pool never shows one vault after the swap and the other before it.
///
/// A group is complete once its status arrives, once a later write hits one
/// of its keys, or once its stream is `PROGRESS_SLOTS` past its slot. Agave
/// notifies every account write of a batch during commit, before the batch
/// releases its locks, so a conflicting transaction cannot write before them.
// src: anza-xyz/agave@825efd18292aff6ffcf9daa0f7612f21b3531a72 runtime/src/bank.rs (commit_transactions)
// src: anza-xyz/agave@825efd18292aff6ffcf9daa0f7612f21b3531a72 accounts-db/src/accounts.rs (_store_accounts)
// src: anza-xyz/agave@825efd18292aff6ffcf9daa0f7612f21b3531a72 runtime/src/transaction_batch.rs (Drop)
#[derive(Default)]
pub(crate) struct TxnBuffer {
    groups: HashMap<GroupId, Held>,
    holder: HashMap<Pubkey, GroupId>,
    held: usize,
}

pub(crate) type Released = Vec<(Source, Vec<AccountUpdate>)>;

pub(crate) struct Hold {
    /// The open group that last wrote the same key; complete, and applied
    /// before the new write.
    pub superseded: Option<(Source, Vec<AccountUpdate>)>,
    /// The write itself when no transaction status will close it.
    pub unheld: Option<AccountUpdate>,
}

impl TxnBuffer {
    pub(crate) fn hold(&mut self, source: Source, update: AccountUpdate, now: Instant) -> Hold {
        let own = update.txn.map(|signature| (source, update.slot, signature));
        let superseded = self
            .holder
            .get(&update.pubkey)
            .copied()
            .filter(|id| Some(*id) != own)
            .and_then(|id| Some((id.0, self.remove(&id)?)));
        let Some(id) = own else {
            return Hold {
                superseded,
                unheld: Some(update),
            };
        };
        self.holder.insert(update.pubkey, id);
        self.groups
            .entry(id)
            .or_insert_with(|| Held {
                opened: now,
                updates: Vec::new(),
            })
            .updates
            .push(update);
        self.held += 1;
        Hold {
            superseded,
            unheld: None,
        }
    }

    pub(crate) fn commit(
        &mut self,
        source: Source,
        slot: Slot,
        signature: TxnSignature,
    ) -> Vec<AccountUpdate> {
        self.remove(&(source, slot, signature)).unwrap_or_default()
    }

    pub(crate) fn advanced(&mut self, stream: StreamId, slot: Slot) -> Released {
        self.take(|id, _| id.0.stream == stream && id.1.0.saturating_add(PROGRESS_SLOTS) <= slot.0)
    }

    pub(crate) fn expired(&mut self, now: Instant, max_hold: Duration) -> Released {
        self.take(|_, held| now.duration_since(held.opened) >= max_hold)
    }

    pub(crate) fn all(&mut self) -> Released {
        self.take(|_, _| true)
    }

    pub(crate) fn discard(&mut self, stream: StreamId) {
        let ids: Vec<GroupId> = self
            .groups
            .keys()
            .filter(|id| id.0.stream == stream)
            .copied()
            .collect();
        for id in ids {
            self.remove(&id);
        }
    }

    pub(crate) fn holds(&self, key: &Pubkey, slot: Slot) -> bool {
        self.holder.get(key).is_some_and(|id| id.1 <= slot)
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
            .filter_map(|id| Some((id.0, id.1, self.remove(&id)?)))
            .collect();
        released
            .sort_by_key(|(_, slot, updates)| (*slot, updates.first().map(|u| u.write_version)));
        released
            .into_iter()
            .map(|(source, _, updates)| (source, updates))
            .collect()
    }

    fn remove(&mut self, id: &GroupId) -> Option<Vec<AccountUpdate>> {
        let held = self.groups.remove(id)?;
        for update in &held.updates {
            if self.holder.get(&update.pubkey) == Some(id) {
                self.holder.remove(&update.pubkey);
            }
        }
        self.held -= held.updates.len();
        Some(held.updates)
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
                    let hold = buffer.hold(SHARD, update, now);
                    hold.superseded
                        .map(|(_, updates)| updates)
                        .unwrap_or_default()
                        .into_iter()
                        .chain(hold.unheld)
                        .collect()
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
    fn a_later_write_to_a_held_key_releases_the_whole_older_group_first() {
        let mut buffer = TxnBuffer::default();
        let now = Instant::now();
        let vault = write(10, 1);
        let other_vault = write(10, 1);
        buffer.hold(SHARD, vault.clone(), now);
        buffer.hold(SHARD, other_vault.clone(), now);
        let hold = buffer.hold(
            SHARD,
            AccountUpdate {
                txn: Some(TxnSignature([2; 64])),
                ..vault.clone()
            },
            now,
        );
        let released: Vec<Pubkey> = hold
            .superseded
            .into_iter()
            .flat_map(|(_, updates)| updates)
            .map(|u| u.pubkey)
            .collect();
        assert_eq!(
            (released, hold.unheld.is_none(), buffer.held()),
            (vec![vault.pubkey, other_vault.pubkey], true, 1)
        );
    }

    #[test]
    fn a_group_is_released_once_its_own_stream_is_two_slots_past_it() {
        for (stream, slot, released) in [
            (StreamId::Shard(0), 11, false),
            (StreamId::Shard(1), 12, false),
            (StreamId::Shard(0), 12, true),
        ] {
            let mut buffer = TxnBuffer::default();
            buffer.hold(SHARD, write(10, 1), Instant::now());
            assert_eq!(
                held_slots(&buffer.advanced(stream, Slot(slot))) == [10],
                released,
                "{stream:?} at {slot}"
            );
        }
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
