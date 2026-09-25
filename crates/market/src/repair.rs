use std::collections::HashMap;
use std::time::Duration;

use domain::{Pubkey, Slot};
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Priority {
    Structural,
    Normal,
    Speculative,
}

#[derive(Debug, Clone, Copy)]
struct Want {
    min_slot: Slot,
    epoch: u64,
    priority: Priority,
    not_before: Instant,
    attempts: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct Ticket {
    pub id: u64,
    pub keys: Vec<(Pubkey, u64)>,
    pub min_slot: Slot,
}

#[derive(Debug, Clone, Copy)]
struct InFlight {
    ticket: u64,
    epoch: u64,
    min_slot: Slot,
    attempts: u32,
    priority: Priority,
}

/// Keys that need an RPC read, batched into tickets of at most `batch`
/// keys. A key is never read twice at once; a newer want for a key that is
/// in flight waits until the current read comes back.
#[derive(Debug, Default)]
pub(crate) struct RepairQueue {
    wanted: HashMap<Pubkey, Want>,
    inflight: HashMap<Pubkey, InFlight>,
    tickets_in_flight: usize,
    next_id: u64,
}

impl RepairQueue {
    pub(crate) fn want(&mut self, key: Pubkey, min_slot: Slot, epoch: u64, priority: Priority) {
        let now = Instant::now();
        self.wanted
            .entry(key)
            .and_modify(|w| {
                w.min_slot = w.min_slot.max(min_slot);
                w.epoch = w.epoch.max(epoch);
                w.priority = w.priority.min(priority);
            })
            .or_insert(Want {
                min_slot,
                epoch,
                priority,
                not_before: now,
                attempts: 0,
            });
    }

    pub(crate) fn cancel(&mut self, key: &Pubkey) {
        self.wanted.remove(key);
    }

    pub(crate) fn is_wanted(&self, key: &Pubkey) -> bool {
        self.wanted.contains_key(key) || self.inflight.contains_key(key)
    }

    pub(crate) fn backlog(&self) -> usize {
        self.wanted.len() + self.inflight.len()
    }

    pub(crate) const fn tickets_in_flight(&self) -> usize {
        self.tickets_in_flight
    }

    pub(crate) fn next_ticket(&mut self, batch: usize) -> Option<Ticket> {
        let now = Instant::now();
        let mut ready: Vec<(Pubkey, Want)> = self
            .wanted
            .iter()
            .filter(|(k, w)| w.not_before <= now && !self.inflight.contains_key(*k))
            .map(|(k, w)| (*k, *w))
            .collect();
        if ready.is_empty() {
            return None;
        }
        let order = |(k, w): &(Pubkey, Want)| (w.priority, w.min_slot, *k);
        let batch = batch.max(1);
        if ready.len() > batch {
            ready.select_nth_unstable_by_key(batch - 1, order);
            ready.truncate(batch);
        }
        ready.sort_by_key(order);
        self.next_id += 1;
        let id = self.next_id;
        let min_slot = ready
            .iter()
            .map(|(_, w)| w.min_slot)
            .max()
            .unwrap_or_default();
        for (key, want) in &ready {
            self.wanted.remove(key);
            self.inflight.insert(
                *key,
                InFlight {
                    ticket: id,
                    epoch: want.epoch,
                    min_slot: want.min_slot,
                    attempts: want.attempts,
                    priority: want.priority,
                },
            );
        }
        self.tickets_in_flight += 1;
        Some(Ticket {
            id,
            keys: ready.iter().map(|(k, w)| (*k, w.epoch)).collect(),
            min_slot,
        })
    }

    pub(crate) fn finish_ticket(&mut self) {
        self.tickets_in_flight = self.tickets_in_flight.saturating_sub(1);
    }

    /// Clears the key's in-flight read; returns the epoch it was read for.
    pub(crate) fn complete(&mut self, key: &Pubkey, ticket: u64) -> Option<u64> {
        match self.inflight.get(key) {
            Some(f) if f.ticket == ticket => self.inflight.remove(key).map(|f| f.epoch),
            _ => None,
        }
    }

    pub(crate) fn retry(&mut self, key: Pubkey, ticket: u64, delay: impl Fn(u32) -> Duration) {
        let Some(flight) = self
            .inflight
            .get(&key)
            .filter(|f| f.ticket == ticket)
            .copied()
        else {
            return;
        };
        self.inflight.remove(&key);
        let attempts = flight.attempts + 1;
        let retry = Want {
            min_slot: flight.min_slot,
            epoch: flight.epoch,
            priority: flight.priority,
            not_before: Instant::now() + delay(attempts),
            attempts,
        };
        self.wanted
            .entry(key)
            .and_modify(|w| {
                w.min_slot = w.min_slot.max(retry.min_slot);
                w.epoch = w.epoch.max(retry.epoch);
            })
            .or_insert(retry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(i: u8) -> Pubkey {
        Pubkey::new_from_array([i; 32])
    }

    #[test]
    fn wanted_keys_are_batched_up_to_the_limit() {
        let mut queue = RepairQueue::default();
        for i in 0..5 {
            queue.want(key(i), Slot(10), 0, Priority::Normal);
        }
        let ticket = queue.next_ticket(3).unwrap();
        assert_eq!(ticket.keys.len(), 3);
    }

    #[test]
    fn a_key_in_flight_is_not_read_again() {
        let mut queue = RepairQueue::default();
        queue.want(key(1), Slot(10), 0, Priority::Normal);
        queue.next_ticket(100).unwrap();
        queue.want(key(1), Slot(20), 1, Priority::Normal);
        assert!(queue.next_ticket(100).is_none());
    }

    #[test]
    fn a_newer_want_is_read_after_the_current_read_completes() {
        let mut queue = RepairQueue::default();
        queue.want(key(1), Slot(10), 0, Priority::Normal);
        let first = queue.next_ticket(100).unwrap();
        queue.want(key(1), Slot(20), 1, Priority::Normal);
        queue.complete(&key(1), first.id);
        let second = queue.next_ticket(100).unwrap();
        assert_eq!(
            (second.min_slot, second.keys),
            (Slot(20), vec![(key(1), 1)])
        );
    }

    #[test]
    fn structural_keys_are_read_first() {
        let mut queue = RepairQueue::default();
        queue.want(key(1), Slot(10), 0, Priority::Speculative);
        queue.want(key(2), Slot(10), 0, Priority::Structural);
        let ticket = queue.next_ticket(1).unwrap();
        assert_eq!(ticket.keys, vec![(key(2), 0)]);
    }

    #[test]
    fn a_ticket_asks_for_the_highest_barrier_of_its_keys() {
        let mut queue = RepairQueue::default();
        queue.want(key(1), Slot(10), 0, Priority::Normal);
        queue.want(key(2), Slot(40), 0, Priority::Normal);
        assert_eq!(queue.next_ticket(100).unwrap().min_slot, Slot(40));
    }

    #[test]
    fn a_failed_read_is_retried_after_its_delay() {
        let mut queue = RepairQueue::default();
        queue.want(key(1), Slot(10), 0, Priority::Normal);
        let ticket = queue.next_ticket(100).unwrap();
        queue.retry(key(1), ticket.id, |_| Duration::from_secs(60));
        assert!(queue.next_ticket(100).is_none() && queue.is_wanted(&key(1)));
    }
}
