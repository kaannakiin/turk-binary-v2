use std::collections::{BTreeMap, HashSet};

use domain::Slot;

/// Parent links for slots above the last confirmed one, fed by one shard's
/// slot stream.
#[derive(Debug, Default)]
pub(crate) struct SlotTree {
    parents: BTreeMap<Slot, Slot>,
    confirmed: Option<Slot>,
    first_seen: Option<Slot>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Resolution {
    pub confirmed: Slot,
    pub previous: Option<Slot>,
    canonical: HashSet<Slot>,
    /// The parent walk stopped at this slot without reaching `previous`;
    /// anything below it cannot be classified.
    pub unresolved_below: Option<Slot>,
    pub first_seen: Option<Slot>,
}

impl Resolution {
    /// Unclassifiable slots count as canonical, which is exactly what the
    /// store did before fork tracking, so a missing parent never loses data.
    pub fn is_canonical(&self, slot: Slot) -> bool {
        self.canonical.contains(&slot) || self.unresolved_below.is_some_and(|u| slot < u)
    }

    pub fn canonical_slots(&self) -> impl Iterator<Item = Slot> + '_ {
        self.canonical.iter().copied()
    }

    /// Promoted without a fork check because the parent chain has a hole.
    pub fn is_unchecked(&self, slot: Slot) -> bool {
        self.has_gap() && self.unresolved_below.is_some_and(|u| slot < u)
    }

    /// Slots created before the subscription started never had their parent
    /// sent to us, so a hole below the first parent we saw is expected.
    pub fn has_gap(&self) -> bool {
        self.previous.is_some()
            && self
                .unresolved_below
                .zip(self.first_seen)
                .is_some_and(|(missing, first)| missing > first)
    }
}

impl SlotTree {
    /// `live` marks updates of the chain the subscription follows. On
    /// connect the server also resends older slots (finalized, up to about
    /// 32 back); their parents are usable but they do not start the chain.
    pub fn record_parent(&mut self, slot: Slot, parent: Slot, live: bool) {
        if live {
            self.first_seen.get_or_insert(slot);
        }
        if self.confirmed.is_none_or(|c| slot > c) {
            self.parents.insert(slot, parent);
        }
    }

    /// After a drop the chain resumes at the next session's first live slot:
    /// a replayed session continues it, an unreplayed one leaves a hole that
    /// the stream's `Gap` already re-reads.
    pub fn restart(&mut self) {
        self.first_seen = None;
    }

    /// `None` when `slot` is not newer than the last confirmed slot: its
    /// ancestors were already resolved by a later confirmation.
    pub fn confirm(&mut self, slot: Slot) -> Option<Resolution> {
        let previous = self.confirmed;
        if previous.is_some_and(|c| slot <= c) {
            return None;
        }
        let mut canonical = HashSet::new();
        let mut cursor = slot;
        let unresolved_below = loop {
            canonical.insert(cursor);
            match self.parents.get(&cursor) {
                Some(&parent) if previous.is_none_or(|c| parent > c) => cursor = parent,
                Some(_) => break None,
                None => break Some(cursor),
            }
        };
        self.confirmed = Some(slot);
        self.parents = self.parents.split_off(&Slot(slot.0 + 1));
        Some(Resolution {
            confirmed: slot,
            previous,
            canonical,
            unresolved_below,
            first_seen: self.first_seen,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_excludes_sibling_fork() {
        let mut tree = SlotTree::default();
        tree.confirm(Slot(100));
        tree.record_parent(Slot(101), Slot(100), true);
        tree.record_parent(Slot(102), Slot(100), true);
        tree.record_parent(Slot(103), Slot(101), true);
        let res = tree.confirm(Slot(103)).unwrap();
        assert_eq!(
            [101, 102, 103].map(|s| res.is_canonical(Slot(s))),
            [true, false, true]
        );
    }

    #[test]
    fn resent_finalized_slots_before_the_live_chain_are_not_a_gap() {
        let mut tree = SlotTree::default();
        tree.record_parent(Slot(91), Slot(90), false);
        tree.confirm(Slot(91));
        tree.record_parent(Slot(122), Slot(121), true);
        tree.record_parent(Slot(123), Slot(122), true);
        assert!(!tree.confirm(Slot(122)).unwrap().has_gap());
    }

    #[test]
    fn missing_parent_inside_the_live_chain_is_a_gap() {
        let mut tree = SlotTree::default();
        tree.record_parent(Slot(100), Slot(99), true);
        tree.confirm(Slot(100));
        tree.record_parent(Slot(101), Slot(100), true);
        tree.record_parent(Slot(103), Slot(102), true);
        assert!(tree.confirm(Slot(103)).unwrap().has_gap());
    }

    #[test]
    fn a_restarted_chain_starts_at_its_next_live_slot() {
        let mut tree = SlotTree::default();
        tree.record_parent(Slot(100), Slot(99), true);
        tree.confirm(Slot(100));
        tree.restart();
        tree.record_parent(Slot(200), Slot(199), true);
        tree.record_parent(Slot(201), Slot(200), true);
        assert!(!tree.confirm(Slot(201)).unwrap().has_gap());
    }
}
